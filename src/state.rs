use std::{
    collections::HashMap,
    str::FromStr,
    sync::Arc,
};

use anyhow::{Context, bail};
use axum::http::{HeaderMap, HeaderName, Uri, header::AUTHORIZATION};
use tracing::warn;

use crate::{
    account::{ApiKeyMap, KeyIdentity, hash_api_key},
    catalog::CatalogSnapshot,
    config::GatewayConfig,
    model::{Deployment, ModelEntry, Protocol, Provider, RetryPolicy, trim_trailing_slash},
    pricing::{PricingMap, pricing_map},
    proxy::upstream::{HyperUpstreamClient, UpstreamClient},
    usage::{NoopUsageRecorder, UsageRecorder},
};

#[derive(Clone)]
pub(crate) struct GatewayState {
    /// SHA-256(key) → caller identity for every active key of every active
    /// user. Loaded from the database; refreshed on account mutations.
    pub(crate) api_keys: Arc<ApiKeyMap>,
    pub(crate) providers: Arc<HashMap<String, Provider>>,
    /// The public catalog: model name → enabled state + ordered deployments.
    /// The only routing surface clients see.
    pub(crate) models: Arc<HashMap<String, ModelEntry>>,
    /// Σ credit grants per user (the prepaid side of every balance check).
    /// Refreshed on grant mutations, like every other snapshot field.
    pub(crate) granted: Arc<HashMap<uuid::Uuid, rust_decimal::Decimal>>,
    /// Cumulative spend counters (user + key scopes). Process-wide: carried
    /// across snapshot rebuilds, never reset by a reload.
    pub(crate) spend_counter: Arc<dyn crate::quota::SpendCounter>,
    pub(crate) upstream: Arc<dyn UpstreamClient>,
    pub(crate) usage_recorder: Arc<dyn UsageRecorder>,
    pub(crate) pricing: PricingMap,
    pub(crate) retry_policy: RetryPolicy,
}

/// The outcome of resolving a request's `(protocol, model, key)` against the
/// catalog, before any upstream contact.
pub(crate) enum Resolution<'a> {
    /// The filtered, ordered deployment chain to walk (non-empty).
    Chain(Vec<ResolvedDeployment<'a>>),
    /// No such model, or the model is disabled.
    UnknownModel,
    /// The key exists but is not allowed to call this model.
    Forbidden,
    /// The model exists and is enabled but has no deployment on this endpoint's
    /// protocol. `available` lists the protocols it *is* reachable on.
    WrongProtocol { available: Vec<Protocol> },
}

/// One resolved failover hop: the provider to contact and the upstream model
/// name to rewrite the request body to.
pub(crate) struct ResolvedDeployment<'a> {
    pub provider: &'a Provider,
    pub upstream_model: &'a str,
}

/// The prepaid-quota verdict for a request, computed before upstream contact.
pub(crate) enum QuotaDecision {
    Allowed,
    /// The user's cumulative spend has reached their granted balance.
    UserExhausted { granted: rust_decimal::Decimal },
    /// The key's own cumulative spend cap is reached.
    KeyCapReached { cap: rust_decimal::Decimal },
}

impl GatewayState {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_config(config: GatewayConfig) -> anyhow::Result<Self> {
        Self::from_config_with_upstream(config, Arc::new(HyperUpstreamClient::new()))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_config_with_upstream(
        config: GatewayConfig,
        upstream: Arc<dyn UpstreamClient>,
    ) -> anyhow::Result<Self> {
        Self::from_config_with_upstream_and_recorder(config, upstream, Arc::new(NoopUsageRecorder))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_config_with_upstream_and_recorder(
        config: GatewayConfig,
        upstream: Arc<dyn UpstreamClient>,
        usage_recorder: Arc<dyn UsageRecorder>,
    ) -> anyhow::Result<Self> {
        let models = CatalogSnapshot {
            providers: config.providers.clone(),
            pricing: config.pricing.clone(),
            models: Vec::new(),
        };
        Self::from_parts(config, models, upstream, usage_recorder)
    }

    /// Builds the snapshot from config plus the database-sourced catalog
    /// (providers/pricing/models). `catalog.models` seeds the routing surface;
    /// `config` supplies retry policy (and, in test builds, gateway_keys).
    pub(crate) fn from_parts(
        config: GatewayConfig,
        catalog: CatalogSnapshot,
        upstream: Arc<dyn UpstreamClient>,
        usage_recorder: Arc<dyn UsageRecorder>,
    ) -> anyhow::Result<Self> {
        if !config.gateway_keys.is_empty() {
            warn!(
                "config field gateway_keys is no longer used for authentication; \
                 API keys are managed in the database (imported once on first boot)"
            );
        }

        // Test builds treat config gateway_keys as plaintext API keys so the
        // proxy test suite can authenticate without a database. Production
        // always starts from an empty map and loads keys from PostgreSQL.
        #[cfg(test)]
        let api_keys = crate::account::api_key_map_from_plaintext(
            &config
                .gateway_keys
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        #[cfg(not(test))]
        let api_keys = ApiKeyMap::new();

        let pricing = pricing_map(catalog.pricing)?;
        let mut providers = HashMap::new();

        for provider in catalog.providers {
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

            // provider.model_aliases is intentionally ignored here: aliases are
            // legacy data kept in the database this release only so a
            // pre-catalog binary can roll back. Routing uses the model catalog.
            providers.insert(
                provider.id.clone(),
                Provider {
                    id: provider.id,
                    protocol: provider.protocol,
                    base_url: trim_trailing_slash(&provider.base_url).to_owned(),
                    api_key: provider.api_key,
                    anthropic_version: provider.anthropic_version,
                },
            );
        }

        // An empty provider set is a valid (fresh-install) state: the admin
        // console is reachable and the catalog fills in through it. Requests
        // simply resolve no model until then.
        let models = build_model_map(catalog.models, &providers)?;

        let retry = config.retry;
        if retry.max_attempts < 1 {
            bail!("retry.max_attempts must be at least 1");
        }
        if retry.base_delay_ms > retry.max_delay_ms {
            bail!(
                "retry.base_delay_ms ({}) must not exceed retry.max_delay_ms ({})",
                retry.base_delay_ms,
                retry.max_delay_ms
            );
        }
        if let Some(status) = retry
            .retryable_statuses
            .iter()
            .find(|status| !(100..=599).contains(*status))
        {
            bail!("retry.retryable_statuses contains out-of-range HTTP status {status}");
        }
        let retry_policy = RetryPolicy {
            retryable_statuses: retry.retryable_statuses.into_iter().collect(),
            max_attempts: retry.max_attempts,
            base_delay_ms: retry.base_delay_ms,
            max_delay_ms: retry.max_delay_ms,
        };

        // Test builds also grant every config-seeded identity a large balance,
        // so non-quota tests never trip the prepaid check. Quota tests replace
        // the map via `with_granted`.
        #[cfg(test)]
        let granted: HashMap<uuid::Uuid, rust_decimal::Decimal> = api_keys
            .values()
            .map(|identity| {
                (
                    identity.user_id,
                    rust_decimal::Decimal::from(1_000_000_000),
                )
            })
            .collect();
        #[cfg(not(test))]
        let granted = HashMap::new();

        Ok(Self {
            api_keys: Arc::new(api_keys),
            providers: Arc::new(providers),
            models: Arc::new(models),
            granted: Arc::new(granted),
            spend_counter: Arc::new(crate::quota::MemoryCounter::default()),
            upstream,
            usage_recorder,
            pricing,
            retry_policy,
        })
    }

    /// The same snapshot with a replacement key map. Used when account
    /// mutations refresh authentication without re-reading the config file.
    pub(crate) fn with_api_keys(&self, api_keys: ApiKeyMap) -> Self {
        let mut next = self.clone();
        next.api_keys = Arc::new(api_keys);
        next
    }

    /// The same snapshot with a replacement granted-credit map. Used when a
    /// grant mutation refreshes balances without re-reading the config file.
    pub(crate) fn with_granted(
        &self,
        granted: HashMap<uuid::Uuid, rust_decimal::Decimal>,
    ) -> Self {
        let mut next = self.clone();
        next.granted = Arc::new(granted);
        next
    }

    /// The same snapshot with the given spend-counter backend. The counter is
    /// process-wide state: boot installs it once and every rebuild carries it.
    pub(crate) fn with_spend_counter(
        &self,
        spend_counter: Arc<dyn crate::quota::SpendCounter>,
    ) -> Self {
        let mut next = self.clone();
        next.spend_counter = spend_counter;
        next
    }

    /// Resolves the caller identity from the presented credentials, or `None`
    /// when no active key matches. A pure in-memory lookup: request
    /// authentication never touches the database.
    pub(crate) fn authorize(&self, headers: &HeaderMap) -> Option<KeyIdentity> {
        gateway_key_candidates(headers)
            .find_map(|candidate| self.api_keys.get(&hash_api_key(candidate)))
            .cloned()
    }

    /// Resolves `(endpoint protocol, requested model, caller)` to the ordered
    /// deployment chain to walk — or the error the client should see. Pure
    /// in-memory: rejections happen before any upstream contact.
    pub(crate) fn resolve(
        &self,
        protocol: Protocol,
        model: &str,
        identity: &KeyIdentity,
    ) -> Resolution<'_> {
        let Some(entry) = self.models.get(model) else {
            return Resolution::UnknownModel;
        };
        // A disabled model is indistinguishable from an absent one: it is not
        // listed, so it does not resolve.
        if !entry.enabled {
            return Resolution::UnknownModel;
        }
        if !identity.may_call(model) {
            return Resolution::Forbidden;
        }

        let mut chain = Vec::new();
        let mut available = Vec::new();
        for deployment in &entry.deployments {
            let Some(provider) = self.providers.get(&deployment.provider_id) else {
                // build_model_map guarantees every id resolves; skip defensively.
                continue;
            };
            if provider.protocol == protocol {
                chain.push(ResolvedDeployment {
                    provider,
                    upstream_model: &deployment.upstream_model,
                });
            } else if !available.contains(&provider.protocol) {
                available.push(provider.protocol);
            }
        }

        if chain.is_empty() {
            return Resolution::WrongProtocol { available };
        }
        Resolution::Chain(chain)
    }

    /// Enabled models the given key may call, sorted by name — the content of
    /// the gateway-owned `GET /v1/models` listing.
    pub(crate) fn listable_models(&self, identity: &KeyIdentity) -> Vec<(&str, &ModelEntry)> {
        let mut entries: Vec<(&str, &ModelEntry)> = self
            .models
            .iter()
            .filter(|(name, entry)| entry.enabled && identity.may_call(name))
            .map(|(name, entry)| (name.as_str(), entry))
            .collect();
        entries.sort_unstable_by_key(|(name, _)| *name);
        entries
    }

    /// The soft prepaid-quota decision for a caller: two counter reads plus a
    /// snapshot map lookup, zero database access. "Soft" because in-flight
    /// requests and counter lag can overshoot the boundary slightly — the
    /// check only gates new requests.
    pub(crate) async fn check_quota(&self, identity: &KeyIdentity) -> QuotaDecision {
        let granted = self
            .granted
            .get(&identity.user_id)
            .copied()
            .unwrap_or_default();
        let spent_user = self
            .spend_counter
            .get(crate::quota::Scope::User(identity.user_id))
            .await;
        if spent_user >= granted {
            return QuotaDecision::UserExhausted { granted };
        }
        if let Some(cap) = identity.spend_cap_usd {
            let spent_key = self
                .spend_counter
                .get(crate::quota::Scope::Key(identity.api_key_id))
                .await;
            if spent_key >= cap {
                return QuotaDecision::KeyCapReached { cap };
            }
        }
        QuotaDecision::Allowed
    }
}

/// Validates catalog models against the provider set and produces the routing
/// map. Deployments referencing unknown providers or empty upstream names are
/// build errors, so a bad catalog mutation is rejected before the swap.
fn build_model_map(
    models: Vec<crate::catalog::ModelConfig>,
    providers: &HashMap<String, Provider>,
) -> anyhow::Result<HashMap<String, ModelEntry>> {
    let mut map = HashMap::new();
    for model in models {
        if model.name.trim().is_empty() {
            bail!("model name cannot be empty");
        }
        let mut deployments = Vec::new();
        for deployment in model.deployments {
            if !providers.contains_key(&deployment.provider_id) {
                bail!(
                    "model '{}' references unknown provider '{}'",
                    model.name,
                    deployment.provider_id
                );
            }
            if deployment.upstream_model.trim().is_empty() {
                bail!(
                    "model '{}' has an empty upstream model for provider '{}'",
                    model.name,
                    deployment.provider_id
                );
            }
            deployments.push(Deployment {
                provider_id: deployment.provider_id,
                upstream_model: deployment.upstream_model,
            });
        }
        if map
            .insert(
                model.name.clone(),
                ModelEntry {
                    enabled: model.enabled,
                    created_at: model.created_at,
                    deployments,
                },
            )
            .is_some()
        {
            bail!("duplicate model '{}'", model.name);
        }
    }
    Ok(map)
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
