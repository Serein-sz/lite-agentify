use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::Arc,
};

use anyhow::{Context, bail};
use axum::http::{HeaderMap, HeaderName, Uri, header::AUTHORIZATION};
use serde_json::Value;
use tracing::warn;

use super::{
    config::GatewayConfig,
    model::{Provider, Route, trim_trailing_slash},
    pricing::{PricingMap, pricing_map},
    upstream::{HyperUpstreamClient, UpstreamClient},
    usage::{NoopUsageRecorder, UsageRecorder},
};

#[derive(Clone)]
pub(super) struct GatewayState {
    pub(super) gateway_keys: Arc<HashSet<String>>,
    pub(super) providers: Arc<HashMap<String, Provider>>,
    pub(super) routes: Arc<Vec<Route>>,
    pub(super) upstream: Arc<dyn UpstreamClient>,
    pub(super) usage_recorder: Arc<dyn UsageRecorder>,
    pub(super) pricing: PricingMap,
}

impl GatewayState {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn from_config(config: GatewayConfig) -> anyhow::Result<Self> {
        Self::from_config_with_upstream(config, Arc::new(HyperUpstreamClient::new()))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn from_config_with_upstream(
        config: GatewayConfig,
        upstream: Arc<dyn UpstreamClient>,
    ) -> anyhow::Result<Self> {
        Self::from_config_with_upstream_and_recorder(config, upstream, Arc::new(NoopUsageRecorder))
    }

    pub(super) fn from_config_with_upstream_and_recorder(
        config: GatewayConfig,
        upstream: Arc<dyn UpstreamClient>,
        usage_recorder: Arc<dyn UsageRecorder>,
    ) -> anyhow::Result<Self> {
        if config.gateway_keys.is_empty() {
            bail!("at least one gateway key is required");
        }

        let gateway_keys = config.gateway_keys.into_iter().collect::<HashSet<_>>();
        let pricing = pricing_map(config.pricing)?;
        let mut providers = HashMap::new();

        for provider in config.providers {
            if provider.id.trim().is_empty() {
                bail!("provider id cannot be empty");
            }

            if providers.contains_key(&provider.id) {
                bail!("duplicate provider id '{}'", provider.id);
            }

            let base_uri = Uri::from_str(&provider.base_url)
                .with_context(|| format!("invalid base URL for provider '{}'", provider.id))?;
            if base_uri.scheme().is_none() || base_uri.authority().is_none() {
                bail!(
                    "provider '{}' base URL must include scheme and host",
                    provider.id
                );
            }

            if provider.api_key.trim().is_empty() {
                bail!("provider '{}' api_key cannot be empty", provider.id);
            }

            for (alias, upstream_model) in &provider.model_aliases {
                if alias.trim().is_empty() {
                    bail!(
                        "provider '{}' model_aliases cannot contain an empty alias",
                        provider.id
                    );
                }

                if upstream_model.trim().is_empty() {
                    bail!(
                        "provider '{}' model_aliases cannot map '{}' to an empty upstream model",
                        provider.id,
                        alias
                    );
                }
            }

            providers.insert(
                provider.id.clone(),
                Provider {
                    id: provider.id,
                    protocol: provider.protocol,
                    base_url: trim_trailing_slash(&provider.base_url).to_owned(),
                    api_key: provider.api_key,
                    anthropic_version: provider.anthropic_version,
                    model_aliases: provider.model_aliases,
                },
            );
        }

        if providers.is_empty() {
            bail!("at least one provider is required");
        }

        let mut routes = Vec::new();
        for route in config.routes {
            if route.path_prefix.trim().is_empty() || !route.path_prefix.starts_with('/') {
                bail!("route path_prefix must start with '/'");
            }

            if route.providers.is_empty() {
                bail!(
                    "route '{}' must configure at least one provider",
                    route.path_prefix
                );
            }

            let resolved = route
                .providers
                .iter()
                .map(|id| providers.get(id).map(|provider| (id, provider)))
                .collect::<Vec<_>>();

            if resolved.iter().all(Option::is_none) {
                warn!(
                    path_prefix = %route.path_prefix,
                    "skipping route because none of its providers are configured"
                );
                continue;
            }

            if let Some(missing) = route
                .providers
                .iter()
                .zip(&resolved)
                .find_map(|(id, slot)| slot.is_none().then_some(id))
            {
                bail!(
                    "route '{}' references unknown provider '{}'",
                    route.path_prefix,
                    missing
                );
            }

            let protocol = resolved[0]
                .expect("chain is non-empty and fully resolved")
                .1
                .protocol;
            if let Some((id, provider)) = resolved
                .iter()
                .flatten()
                .find(|(_, provider)| provider.protocol != protocol)
            {
                bail!(
                    "route '{}' mixes protocols: provider '{}' is {} but the chain starts with {}",
                    route.path_prefix,
                    id,
                    provider.protocol,
                    protocol
                );
            }

            routes.push(Route {
                path_prefix: route.path_prefix,
                provider_ids: route.providers,
                model_prefix: route.model_prefix,
            });
        }

        if routes.is_empty() {
            bail!("at least one route is required");
        }

        Ok(Self {
            gateway_keys: Arc::new(gateway_keys),
            providers: Arc::new(providers),
            routes: Arc::new(routes),
            upstream,
            usage_recorder,
            pricing,
        })
    }

    pub(super) fn is_authorized(&self, headers: &HeaderMap) -> bool {
        gateway_key_candidates(headers).any(|key| self.gateway_keys.contains(key))
    }

    pub(super) fn match_route(&self, path: &str, body: &[u8]) -> Option<&Route> {
        let model = extract_model(body);

        self.routes
            .iter()
            .filter(|route| path.starts_with(&route.path_prefix))
            .find(|route| {
                route.model_prefix.as_deref().is_none_or(|prefix| {
                    model
                        .as_deref()
                        .is_some_and(|model| model.starts_with(prefix))
                })
            })
    }

    pub(super) fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.get(id)
    }
}

fn gateway_key_candidates(headers: &HeaderMap) -> impl Iterator<Item = &str> {
    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .into_iter()
        .flat_map(|value| {
            [
                value.strip_prefix("Bearer ").unwrap_or(value),
                value.strip_prefix("bearer ").unwrap_or(value),
            ]
        });

    let api_keys = [
        HeaderName::from_static("x-api-key"),
        HeaderName::from_static("api-key"),
    ]
    .into_iter()
    .filter_map(|name| headers.get(name))
    .filter_map(|value| value.to_str().ok());

    authorization.chain(api_keys)
}

fn extract_model(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }

    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}
