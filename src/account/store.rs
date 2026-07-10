use anyhow::{Context, bail};
use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use super::{
    ApiKeyMap, ApiKeyRecord, KeyIdentity, KeyStatus, Role, UserRecord, UserStatus,
    entity::{api_key, user},
};

/// Persistence for accounts and API keys. A trait so the admin API can be
/// exercised in tests without PostgreSQL.
#[async_trait]
pub(crate) trait AccountStore: Send + Sync {
    async fn find_user_by_username(&self, username: &str) -> anyhow::Result<Option<UserRecord>>;
    async fn get_user(&self, id: Uuid) -> anyhow::Result<Option<UserRecord>>;
    async fn list_users(&self) -> anyhow::Result<Vec<UserRecord>>;
    /// Creates a user; a duplicate username yields [`AccountError::Conflict`].
    async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: Role,
    ) -> anyhow::Result<UserRecord>;
    async fn set_user_status(&self, id: Uuid, status: UserStatus) -> anyhow::Result<bool>;
    async fn set_password_hash(&self, id: Uuid, password_hash: &str) -> anyhow::Result<bool>;

    /// Keys owned by `owner`, or every key when `owner` is `None`.
    async fn list_keys(&self, owner: Option<Uuid>) -> anyhow::Result<Vec<ApiKeyRecord>>;
    async fn get_key(&self, id: Uuid) -> anyhow::Result<Option<ApiKeyRecord>>;
    async fn create_key(
        &self,
        owner: Uuid,
        name: &str,
        key_hash_hex: &str,
        prefix: &str,
        allowed_models: Option<&[String]>,
        spend_cap_usd: Option<rust_decimal::Decimal>,
    ) -> anyhow::Result<ApiKeyRecord>;
    async fn revoke_key(&self, id: Uuid) -> anyhow::Result<bool>;
    /// Replaces a key's allowed-models list (`None` = unrestricted) and spend
    /// cap (`None` = uncapped).
    async fn set_key_limits(
        &self,
        id: Uuid,
        allowed_models: Option<&[String]>,
        spend_cap_usd: Option<rust_decimal::Decimal>,
    ) -> anyhow::Result<bool>;

    /// The hot-path auth map: every active key of every active user.
    async fn active_key_map(&self) -> anyhow::Result<ApiKeyMap>;
}

/// Marker error for unique-constraint conflicts (duplicate username).
#[derive(Debug)]
pub(crate) struct Conflict(pub String);

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Conflict {}

fn user_record(model: user::Model) -> anyhow::Result<UserRecord> {
    Ok(UserRecord {
        id: model.id,
        role: Role::parse(&model.role)
            .with_context(|| format!("user '{}' has unknown role '{}'", model.username, model.role))?,
        status: match model.status.as_str() {
            "active" => UserStatus::Active,
            _ => UserStatus::Disabled,
        },
        username: model.username,
        password_hash: model.password_hash,
        created_at: model.created_at,
    })
}

fn key_record(model: api_key::Model) -> ApiKeyRecord {
    ApiKeyRecord {
        id: model.id,
        user_id: model.user_id,
        prefix: model.prefix,
        name: model.name,
        status: match model.status.as_str() {
            "active" => KeyStatus::Active,
            _ => KeyStatus::Revoked,
        },
        created_at: model.created_at,
        last_used_at: model.last_used_at,
        allowed_models: model.allowed_models.and_then(parse_allowed_models),
        spend_cap_usd: model.spend_cap_usd,
    }
}

/// Parses the JSONB `allowed_models` column into a list of names. A JSON `null`
/// or a non-array value means "unrestricted" (`None`).
fn parse_allowed_models(value: serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::Array(items) => Some(
            items
                .into_iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => None,
    }
}

#[derive(Clone)]
pub(crate) struct SeaOrmAccountStore {
    db: DatabaseConnection,
}

impl SeaOrmAccountStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AccountStore for SeaOrmAccountStore {
    async fn find_user_by_username(&self, username: &str) -> anyhow::Result<Option<UserRecord>> {
        user::Entity::find()
            .filter(user::Column::Username.eq(username))
            .one(&self.db)
            .await
            .context("failed to query user by username")?
            .map(user_record)
            .transpose()
    }

    async fn get_user(&self, id: Uuid) -> anyhow::Result<Option<UserRecord>> {
        user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .context("failed to query user")?
            .map(user_record)
            .transpose()
    }

    async fn list_users(&self) -> anyhow::Result<Vec<UserRecord>> {
        user::Entity::find()
            .order_by_asc(user::Column::CreatedAt)
            .all(&self.db)
            .await
            .context("failed to list users")?
            .into_iter()
            .map(user_record)
            .collect()
    }

    async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: Role,
    ) -> anyhow::Result<UserRecord> {
        if self.find_user_by_username(username).await?.is_some() {
            bail!(Conflict(format!("username '{username}' already exists")));
        }
        let model = user::ActiveModel {
            id: Set(Uuid::new_v4()),
            username: Set(username.to_owned()),
            password_hash: Set(password_hash.to_owned()),
            role: Set(role.as_str().to_owned()),
            status: Set(UserStatus::Active.as_str().to_owned()),
            created_at: Set(Utc::now()),
        };
        let inserted = model.insert(&self.db).await.map_err(|error| {
            // Races on the unique index surface as a conflict, not a 500.
            if error.to_string().contains("duplicate key") {
                anyhow::Error::new(Conflict(format!("username '{username}' already exists")))
            } else {
                anyhow::Error::new(error).context("failed to create user")
            }
        })?;
        user_record(inserted)
    }

    async fn set_user_status(&self, id: Uuid, status: UserStatus) -> anyhow::Result<bool> {
        let Some(existing) = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .context("failed to query user")?
        else {
            return Ok(false);
        };
        let mut model: user::ActiveModel = existing.into();
        model.status = Set(status.as_str().to_owned());
        model.update(&self.db).await.context("failed to update user status")?;
        Ok(true)
    }

    async fn set_password_hash(&self, id: Uuid, password_hash: &str) -> anyhow::Result<bool> {
        let Some(existing) = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .context("failed to query user")?
        else {
            return Ok(false);
        };
        let mut model: user::ActiveModel = existing.into();
        model.password_hash = Set(password_hash.to_owned());
        model.update(&self.db).await.context("failed to update password")?;
        Ok(true)
    }

    async fn list_keys(&self, owner: Option<Uuid>) -> anyhow::Result<Vec<ApiKeyRecord>> {
        let mut query = api_key::Entity::find().order_by_asc(api_key::Column::CreatedAt);
        if let Some(owner) = owner {
            query = query.filter(api_key::Column::UserId.eq(owner));
        }
        Ok(query
            .all(&self.db)
            .await
            .context("failed to list api keys")?
            .into_iter()
            .map(key_record)
            .collect())
    }

    async fn get_key(&self, id: Uuid) -> anyhow::Result<Option<ApiKeyRecord>> {
        Ok(api_key::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .context("failed to query api key")?
            .map(key_record))
    }

    async fn create_key(
        &self,
        owner: Uuid,
        name: &str,
        key_hash_hex: &str,
        prefix: &str,
        allowed_models: Option<&[String]>,
        spend_cap_usd: Option<rust_decimal::Decimal>,
    ) -> anyhow::Result<ApiKeyRecord> {
        let model = api_key::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(owner),
            key_hash: Set(key_hash_hex.to_owned()),
            prefix: Set(prefix.to_owned()),
            name: Set(name.to_owned()),
            status: Set(KeyStatus::Active.as_str().to_owned()),
            created_at: Set(Utc::now()),
            last_used_at: Set(None),
            allowed_models: Set(allowed_models_to_json(allowed_models)),
            spend_cap_usd: Set(spend_cap_usd),
        };
        let inserted = model.insert(&self.db).await.context("failed to create api key")?;
        Ok(key_record(inserted))
    }

    async fn revoke_key(&self, id: Uuid) -> anyhow::Result<bool> {
        let Some(existing) = api_key::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .context("failed to query api key")?
        else {
            return Ok(false);
        };
        let mut model: api_key::ActiveModel = existing.into();
        model.status = Set(KeyStatus::Revoked.as_str().to_owned());
        model.update(&self.db).await.context("failed to revoke api key")?;
        Ok(true)
    }

    async fn set_key_limits(
        &self,
        id: Uuid,
        allowed_models: Option<&[String]>,
        spend_cap_usd: Option<rust_decimal::Decimal>,
    ) -> anyhow::Result<bool> {
        let Some(existing) = api_key::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .context("failed to query api key")?
        else {
            return Ok(false);
        };
        let mut model: api_key::ActiveModel = existing.into();
        model.allowed_models = Set(allowed_models_to_json(allowed_models));
        model.spend_cap_usd = Set(spend_cap_usd);
        model
            .update(&self.db)
            .await
            .context("failed to update key limits")?;
        Ok(true)
    }

    async fn active_key_map(&self) -> anyhow::Result<ApiKeyMap> {
        let keys = api_key::Entity::find()
            .filter(api_key::Column::Status.eq(KeyStatus::Active.as_str()))
            .all(&self.db)
            .await
            .context("failed to load api keys")?;
        let users = user::Entity::find()
            .filter(user::Column::Status.eq(UserStatus::Active.as_str()))
            .all(&self.db)
            .await
            .context("failed to load users")?;
        let active_users: std::collections::HashSet<Uuid> =
            users.into_iter().map(|user| user.id).collect();

        let mut map = ApiKeyMap::new();
        for key in keys {
            if !active_users.contains(&key.user_id) {
                continue;
            }
            let Some(digest) = decode_hex_32(&key.key_hash) else {
                tracing::warn!(key_id = %key.id, "api key has malformed key_hash; skipping");
                continue;
            };
            let allowed_models = key
                .allowed_models
                .and_then(parse_allowed_models)
                .map(|names| std::sync::Arc::new(names.into_iter().collect()));
            map.insert(
                digest,
                KeyIdentity {
                    user_id: key.user_id,
                    api_key_id: key.id,
                    allowed_models,
                    spend_cap_usd: key.spend_cap_usd,
                },
            );
        }
        Ok(map)
    }
}

/// Serializes an allowed-models list into the JSONB column value. `None` maps
/// to SQL NULL (unrestricted).
fn allowed_models_to_json(allowed_models: Option<&[String]>) -> Option<serde_json::Value> {
    allowed_models.map(|names| serde_json::Value::Array(
        names
            .iter()
            .map(|name| serde_json::Value::String(name.clone()))
            .collect(),
    ))
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        let high = (chunk[0] as char).to_digit(16)?;
        let low = (chunk[1] as char).to_digit(16)?;
        out[index] = (high * 16 + low) as u8;
    }
    Some(out)
}

/// In-memory store for tests: same semantics, no database.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct MemoryAccountStore {
    inner: std::sync::Arc<std::sync::Mutex<MemoryInner>>,
}

#[cfg(test)]
#[derive(Default)]
struct MemoryInner {
    users: Vec<UserRecord>,
    keys: Vec<(ApiKeyRecord, String)>,
}

#[cfg(test)]
impl MemoryAccountStore {
    pub(crate) fn with_user(username: &str, password: &str, role: Role) -> Self {
        let store = Self::default();
        store.insert_user(username, password, role);
        store
    }

    pub(crate) fn insert_user(&self, username: &str, password: &str, role: Role) -> Uuid {
        let id = Uuid::new_v4();
        self.inner.lock().unwrap().users.push(UserRecord {
            id,
            username: username.to_owned(),
            password_hash: crate::admin::password::hash_password(password)
                .expect("test password hash"),
            role,
            status: UserStatus::Active,
            created_at: Utc::now(),
        });
        id
    }
}

#[cfg(test)]
#[async_trait]
impl AccountStore for MemoryAccountStore {
    async fn find_user_by_username(&self, username: &str) -> anyhow::Result<Option<UserRecord>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .users
            .iter()
            .find(|user| user.username == username)
            .cloned())
    }

    async fn get_user(&self, id: Uuid) -> anyhow::Result<Option<UserRecord>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .users
            .iter()
            .find(|user| user.id == id)
            .cloned())
    }

    async fn list_users(&self) -> anyhow::Result<Vec<UserRecord>> {
        Ok(self.inner.lock().unwrap().users.clone())
    }

    async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: Role,
    ) -> anyhow::Result<UserRecord> {
        let mut inner = self.inner.lock().unwrap();
        if inner.users.iter().any(|user| user.username == username) {
            bail!(Conflict(format!("username '{username}' already exists")));
        }
        let record = UserRecord {
            id: Uuid::new_v4(),
            username: username.to_owned(),
            password_hash: password_hash.to_owned(),
            role,
            status: UserStatus::Active,
            created_at: Utc::now(),
        };
        inner.users.push(record.clone());
        Ok(record)
    }

    async fn set_user_status(&self, id: Uuid, status: UserStatus) -> anyhow::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        match inner.users.iter_mut().find(|user| user.id == id) {
            Some(user) => {
                user.status = status;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn set_password_hash(&self, id: Uuid, password_hash: &str) -> anyhow::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        match inner.users.iter_mut().find(|user| user.id == id) {
            Some(user) => {
                user.password_hash = password_hash.to_owned();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn list_keys(&self, owner: Option<Uuid>) -> anyhow::Result<Vec<ApiKeyRecord>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .keys
            .iter()
            .map(|(record, _)| record)
            .filter(|record| owner.is_none_or(|owner| record.user_id == owner))
            .cloned()
            .collect())
    }

    async fn get_key(&self, id: Uuid) -> anyhow::Result<Option<ApiKeyRecord>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .keys
            .iter()
            .map(|(record, _)| record)
            .find(|record| record.id == id)
            .cloned())
    }

    async fn create_key(
        &self,
        owner: Uuid,
        name: &str,
        key_hash_hex: &str,
        prefix: &str,
        allowed_models: Option<&[String]>,
        spend_cap_usd: Option<rust_decimal::Decimal>,
    ) -> anyhow::Result<ApiKeyRecord> {
        let record = ApiKeyRecord {
            id: Uuid::new_v4(),
            user_id: owner,
            prefix: prefix.to_owned(),
            name: name.to_owned(),
            status: KeyStatus::Active,
            created_at: Utc::now(),
            last_used_at: None,
            allowed_models: allowed_models.map(<[String]>::to_vec),
            spend_cap_usd,
        };
        self.inner
            .lock()
            .unwrap()
            .keys
            .push((record.clone(), key_hash_hex.to_owned()));
        Ok(record)
    }

    async fn revoke_key(&self, id: Uuid) -> anyhow::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        match inner.keys.iter_mut().find(|(record, _)| record.id == id) {
            Some((record, _)) => {
                record.status = KeyStatus::Revoked;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn set_key_limits(
        &self,
        id: Uuid,
        allowed_models: Option<&[String]>,
        spend_cap_usd: Option<rust_decimal::Decimal>,
    ) -> anyhow::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        match inner.keys.iter_mut().find(|(record, _)| record.id == id) {
            Some((record, _)) => {
                record.allowed_models = allowed_models.map(<[String]>::to_vec);
                record.spend_cap_usd = spend_cap_usd;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn active_key_map(&self) -> anyhow::Result<ApiKeyMap> {
        let inner = self.inner.lock().unwrap();
        let mut map = ApiKeyMap::new();
        for (record, hash_hex) in &inner.keys {
            if record.status != KeyStatus::Active {
                continue;
            }
            let owner_active = inner
                .users
                .iter()
                .any(|user| user.id == record.user_id && user.status == UserStatus::Active);
            if !owner_active {
                continue;
            }
            if let Some(digest) = decode_hex_32(hash_hex) {
                map.insert(
                    digest,
                    KeyIdentity {
                        user_id: record.user_id,
                        api_key_id: record.id,
                        allowed_models: record
                            .allowed_models
                            .clone()
                            .map(|names| std::sync::Arc::new(names.into_iter().collect())),
                        spend_cap_usd: record.spend_cap_usd,
                    },
                );
            }
        }
        Ok(map)
    }
}
