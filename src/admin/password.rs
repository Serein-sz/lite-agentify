use std::path::Path;

use anyhow::Context;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use tracing::{info, warn};

use crate::config::GatewayConfig;

/// True when the stored value is an argon2 PHC string rather than plaintext.
pub(crate) fn is_phc_hash(value: &str) -> bool {
    value.starts_with("$argon2")
}

pub(crate) fn hash_password(plain: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!("failed to hash admin password: {error}"))?;
    Ok(hash.to_string())
}

/// Verifies a submitted password against the stored value. The stored value is
/// normally a PHC hash; plaintext is still accepted so that a hand-edited
/// password works after a hot reload (write-back only happens at startup).
pub(crate) fn verify_password(stored: &str, submitted: &str) -> bool {
    if is_phc_hash(stored) {
        let Ok(hash) = PasswordHash::new(stored) else {
            warn!("stored admin password looks like a PHC string but failed to parse");
            return false;
        };
        Argon2::default()
            .verify_password(submitted.as_bytes(), &hash)
            .is_ok()
    } else {
        constant_time_eq(stored.as_bytes(), submitted.as_bytes())
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Replaces a plaintext `admin_password` with its argon2id hash, both in the
/// in-memory config and in the config file (comments preserved). Runs before
/// the config watcher spawns so the rewrite cannot trigger a spurious reload.
/// A write-back failure only warns: the in-memory hash keeps logins working,
/// but the plaintext remains on disk until the file becomes writable.
pub fn bootstrap_admin_password(config: &mut GatewayConfig, config_path: &Path) {
    let Some(password) = config.admin_password.as_ref() else {
        return;
    };
    if is_phc_hash(password) {
        return;
    }

    let hash = match hash_password(password) {
        Ok(hash) => hash,
        Err(error) => {
            warn!(%error, "failed to hash admin_password; admin console logins will fail");
            return;
        }
    };

    match write_back_hash(config_path, &hash) {
        Ok(()) => info!(
            config_path = %config_path.display(),
            "replaced plaintext admin_password in config file with its argon2id hash"
        ),
        Err(error) => warn!(
            error = format!("{error:#}"),
            config_path = %config_path.display(),
            "failed to write hashed admin_password back to config file; \
             the plaintext remains on disk until the file is writable"
        ),
    }

    config.admin_password = Some(hash);
}

fn write_back_hash(config_path: &Path, hash: &str) -> anyhow::Result<()> {
    let contents = std::fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut document = contents
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse config file as TOML")?;
    // Replace only the value, keeping the key's surrounding whitespace and any
    // same-line comment intact.
    match document
        .get_mut("admin_password")
        .and_then(toml_edit::Item::as_value_mut)
    {
        Some(value) => {
            let decor = value.decor().clone();
            *value = toml_edit::Value::from(hash);
            *value.decor_mut() = decor;
        }
        None => document["admin_password"] = toml_edit::value(hash),
    }
    std::fs::write(config_path, document.to_string())
        .with_context(|| format!("failed to write {}", config_path.display()))
}
