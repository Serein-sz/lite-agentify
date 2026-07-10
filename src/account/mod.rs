mod bootstrap;
mod entity;
mod keys;
mod store;

use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(crate) use bootstrap::bootstrap_accounts;
pub(crate) use keys::{generate_api_key, hash_api_key, hash_api_key_hex, key_prefix};
pub(crate) use store::{AccountStore, Conflict, SeaOrmAccountStore};

#[cfg(test)]
pub(crate) use store::MemoryAccountStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Admin,
    User,
}

impl Role {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "admin" => Some(Self::Admin),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UserStatus {
    Active,
    Disabled,
}

impl UserStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum KeyStatus {
    Active,
    Revoked,
}

impl KeyStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UserRecord {
    pub id: Uuid,
    pub username: String,
    #[serde(skip)]
    pub password_hash: String,
    pub role: Role,
    pub status: UserStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApiKeyRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub prefix: String,
    pub name: String,
    pub status: KeyStatus,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// Model names this key may call; `None` = every enabled model.
    pub allowed_models: Option<Vec<String>>,
    /// Cumulative USD spend cap; `None` = uncapped.
    pub spend_cap_usd: Option<rust_decimal::Decimal>,
}

/// The caller identity an API key resolves to on the request hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyIdentity {
    pub user_id: Uuid,
    pub api_key_id: Uuid,
    /// The set of model names this key may call; `None` = unrestricted. Shared
    /// (Arc) so cloning an identity per request stays cheap.
    pub allowed_models: Option<Arc<std::collections::HashSet<String>>>,
    /// Cumulative USD spend cap for this key; `None` = uncapped.
    pub spend_cap_usd: Option<rust_decimal::Decimal>,
}

impl KeyIdentity {
    /// Whether this key is permitted to call `model`.
    pub(crate) fn may_call(&self, model: &str) -> bool {
        self.allowed_models
            .as_ref()
            .is_none_or(|set| set.contains(model))
    }
}

/// SHA-256(key bytes) → identity. Lives in the arc-swapped gateway snapshot;
/// rebuilt from the database on boot and after any user/key mutation.
pub(crate) type ApiKeyMap = HashMap<[u8; 32], KeyIdentity>;

/// Builds an [`ApiKeyMap`] from plaintext keys with fresh identities. Used by
/// tests and anywhere key material exists outside the database.
#[cfg(test)]
pub(crate) fn api_key_map_from_plaintext(keys: &[&str]) -> ApiKeyMap {
    keys.iter()
        .map(|key| {
            (
                hash_api_key(key),
                KeyIdentity {
                    user_id: Uuid::new_v4(),
                    api_key_id: Uuid::new_v4(),
                    allowed_models: None,
                    spend_cap_usd: None,
                },
            )
        })
        .collect()
}
