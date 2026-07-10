use anyhow::{Context, bail};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set, TransactionTrait,
};
use tracing::{info, warn};
use uuid::Uuid;

use super::{
    KeyStatus, Role, UserStatus,
    entity::{api_key, user},
    keys,
};
use crate::config::GatewayConfig;

/// One-time, idempotent account seeding, run at startup inside a transaction:
///
/// 1. Empty `users` table → create the bootstrap `admin` user from the
///    config's `admin_password` (already argon2id-hashed by the password
///    write-back). Startup fails when the table is empty and no password is
///    configured.
/// 2. Empty `api_keys` table + legacy `gateway_keys` in config → import each
///    entry as an active key owned by the bootstrap admin so existing clients
///    keep authenticating.
///
/// With non-empty tables both steps are skipped; a lingering `gateway_keys`
/// config field only produces a warning.
pub(crate) async fn bootstrap_accounts(
    db: &DatabaseConnection,
    config: &GatewayConfig,
) -> anyhow::Result<()> {
    let txn = db.begin().await.context("failed to begin bootstrap")?;

    let user_count = user::Entity::find()
        .count(&txn)
        .await
        .context("failed to count users")?;

    let admin_id = if user_count == 0 {
        let Some(password_hash) = config.admin_password.as_ref() else {
            bail!(
                "no users exist and no admin_password is configured: set admin_password \
                 in the config file to seed the bootstrap admin account"
            );
        };
        // bootstrap_admin_password has already replaced plaintext with a PHC
        // hash; hash here defensively in case the file write-back failed.
        let password_hash = if crate::admin::password::is_phc_hash(password_hash) {
            password_hash.clone()
        } else {
            crate::admin::password::hash_password(password_hash)?
        };
        let id = Uuid::new_v4();
        user::ActiveModel {
            id: Set(id),
            username: Set("admin".to_owned()),
            password_hash: Set(password_hash),
            role: Set(Role::Admin.as_str().to_owned()),
            status: Set(UserStatus::Active.as_str().to_owned()),
            created_at: Set(Utc::now()),
        }
        .insert(&txn)
        .await
        .context("failed to create bootstrap admin user")?;
        info!("seeded bootstrap 'admin' user from admin_password");
        Some(id)
    } else {
        None
    };

    let key_count = api_key::Entity::find()
        .count(&txn)
        .await
        .context("failed to count api keys")?;

    if key_count == 0 && !config.gateway_keys.is_empty() {
        let owner = match admin_id {
            Some(id) => Some(id),
            None => user::Entity::find()
                .filter(user::Column::Role.eq(Role::Admin.as_str()))
                .one(&txn)
                .await
                .context("failed to find an admin user for key import")?
                .map(|admin| admin.id),
        };
        let Some(owner) = owner else {
            bail!("cannot import gateway_keys: no admin user exists");
        };

        for (index, key) in config.gateway_keys.iter().enumerate() {
            api_key::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(owner),
                key_hash: Set(keys::hash_api_key_hex(key)),
                prefix: Set(keys::key_prefix(key)),
                name: Set(format!("imported-{}", index + 1)),
                status: Set(KeyStatus::Active.as_str().to_owned()),
                created_at: Set(Utc::now()),
                last_used_at: Set(None),
                allowed_models: Set(None),
                spend_cap_usd: Set(None),
            }
            .insert(&txn)
            .await
            .context("failed to import legacy gateway key")?;
        }
        info!(
            count = config.gateway_keys.len(),
            "imported legacy gateway_keys as admin-owned API keys; \
             remove gateway_keys from the config file"
        );
    }

    txn.commit().await.context("failed to commit bootstrap")?;

    if key_count > 0 && !config.gateway_keys.is_empty() {
        warn!(
            "config field gateway_keys is no longer used for authentication and can be removed; \
             API keys are managed in the database"
        );
    }

    Ok(())
}
