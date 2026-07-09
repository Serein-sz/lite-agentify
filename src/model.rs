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
pub(crate) struct Provider {
    pub id: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub api_key: String,
    pub anthropic_version: Option<String>,
    pub model_aliases: HashMap<String, String>,
}

#[derive(Clone)]
pub(crate) struct Route {
    pub path_prefix: String,
    pub provider_ids: Vec<String>,
    pub model_prefix: Option<String>,
}

/// Resolved, validated retry policy carried in the hot-reloadable state
/// snapshot. Built from `config::RetryConfig` in `GatewayState::from_config`.
#[derive(Clone, Debug)]
pub(crate) struct RetryPolicy {
    pub retryable_statuses: std::collections::HashSet<u16>,
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl RetryPolicy {
    /// Whether a provider that returned `status` should be retried against
    /// itself before the failover chain advances.
    pub(crate) fn is_retryable(&self, status: u16) -> bool {
        self.retryable_statuses.contains(&status)
    }

    /// Full-jitter backoff for a zero-based attempt index: a uniform random
    /// wait in `[0, min(max_delay, base_delay * 2^attempt)]` milliseconds.
    /// `jitter` is a caller-supplied fraction in `[0, 1)` so the draw stays
    /// testable without a global RNG.
    pub(crate) fn backoff_ms(&self, attempt: u32, jitter: f64) -> u64 {
        let ceiling = self
            .base_delay_ms
            .saturating_mul(1u64 << attempt.min(63))
            .min(self.max_delay_ms);
        (ceiling as f64 * jitter.clamp(0.0, 1.0)) as u64
    }

    /// Caps a parsed `Retry-After` (in milliseconds) at `max_delay_ms` so a
    /// hostile or slow provider cannot stall a request unbounded.
    pub(crate) fn cap_delay_ms(&self, delay_ms: u64) -> u64 {
        delay_ms.min(self.max_delay_ms)
    }
}

pub(crate) fn default_listen_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 3000))
}

pub(crate) fn trim_trailing_slash(value: &str) -> &str {
    value.trim_end_matches('/')
}
