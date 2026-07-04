use std::{
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use serde::Deserialize;

use super::model::{Protocol, default_listen_addr};

const CONFIG_ENV_VAR: &str = "LITE_AGENTIFY_GATEWAY_CONFIG";
const CONFIG_DIR_NAME: &str = ".config/lite-agentify";
const CONFIG_FILE_NAME: &str = "llm-gateway.toml";

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: SocketAddr,
    #[serde(default)]
    pub gateway_keys: Vec<String>,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub anthropic_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteConfig {
    pub path_prefix: String,
    pub provider: String,
    #[serde(default)]
    pub model_prefix: Option<String>,
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

pub fn load_config_from_env() -> anyhow::Result<GatewayConfig> {
    let path = env::var(CONFIG_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_config_path());
    GatewayConfig::load_from_path(path)
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
