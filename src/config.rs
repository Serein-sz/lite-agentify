use std::{
    collections::HashMap,
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::model::{Protocol, default_listen_addr};

const CONFIG_ENV_VAR: &str = "LITE_AGENTIFY_GATEWAY_CONFIG";
const CONFIG_DIR_NAME: &str = ".config/lite-agentify";
const CONFIG_FILE_NAME: &str = "lite-agentify.toml";

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: SocketAddr,
    #[serde(default)]
    pub gateway_keys: Vec<String>,
    /// Admin console password. Plaintext on first boot; replaced in the file
    /// by its argon2id PHC hash at startup. The console is disabled when unset.
    #[serde(default)]
    pub admin_password: Option<String>,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
    #[serde(default)]
    pub usage_database: Option<UsageDatabaseConfig>,
    #[serde(default)]
    pub pricing: Vec<PricingConfig>,
    #[serde(default)]
    pub retry: RetryConfig,
}

/// Same-provider retry policy for rate-limit responses (default 429/529). An
/// absent `[retry]` section yields these defaults. Hot-reloadable.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// Upstream statuses that trigger a backed-off retry against the same
    /// provider before advancing the failover chain.
    pub retryable_statuses: Vec<u16>,
    /// Total attempts per provider, including the initial try. Must be >= 1.
    pub max_attempts: u32,
    /// First backoff delay; subsequent delays grow toward `max_delay_ms`.
    pub base_delay_ms: u64,
    /// Upper bound on any single backoff wait, also capping a large `Retry-After`.
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            retryable_statuses: vec![429, 529],
            max_attempts: 4,
            base_delay_ms: 1000,
            max_delay_ms: 8000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub anthropic_version: Option<String>,
    #[serde(default)]
    pub model_aliases: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteConfig {
    pub path_prefix: String,
    pub providers: Vec<String>,
    #[serde(default)]
    pub model_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UsageDatabaseConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub url: String,
    #[serde(default)]
    pub max_connections: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PricingConfig {
    pub provider: String,
    pub model: String,
    pub input_per_1m: Decimal,
    pub output_per_1m: Decimal,
    #[serde(default)]
    pub cached_input_per_1m: Option<Decimal>,
    #[serde(default)]
    pub cache_read_per_1m: Option<Decimal>,
    #[serde(default)]
    pub cache_write_per_1m: Option<Decimal>,
    pub currency: String,
    #[serde(default)]
    pub pricing_source: Option<String>,
}

impl GatewayConfig {
    pub fn load_from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read gateway config {}", path.display()))?;

        match path.extension().and_then(|extension| extension.to_str()) {
            Some("toml") => {
                toml::from_str(&contents).context("failed to parse TOML gateway config")
            }
            _ => bail!("gateway config must be .toml"),
        }
    }
}

/// Resolves the config file path from the environment or the default location.
/// Reload reuses the same path, so a process always reloads the file it booted from.
pub fn resolve_config_path() -> PathBuf {
    env::var(CONFIG_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_config_path())
}

fn default_config_path() -> PathBuf {
    user_home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(CONFIG_DIR_NAME)
        .join(CONFIG_FILE_NAME)
}

fn user_home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
}

fn default_true() -> bool {
    true
}
