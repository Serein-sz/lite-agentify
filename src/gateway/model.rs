use std::{collections::HashMap, net::SocketAddr};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum Protocol {
    #[serde(rename = "openai", alias = "open-ai")]
    OpenAi,
    #[serde(rename = "anthropic")]
    Anthropic,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenAi => f.write_str("openai"),
            Self::Anthropic => f.write_str("anthropic"),
        }
    }
}

#[derive(Clone)]
pub(super) struct Provider {
    pub id: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub api_key: String,
    pub anthropic_version: Option<String>,
    pub model_aliases: HashMap<String, String>,
}

#[derive(Clone)]
pub(super) struct Route {
    pub path_prefix: String,
    pub provider_ids: Vec<String>,
    pub model_prefix: Option<String>,
}

pub(super) fn default_listen_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 3000))
}

pub(super) fn trim_trailing_slash(value: &str) -> &str {
    value.trim_end_matches('/')
}
