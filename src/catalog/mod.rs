mod entity;
mod import;
mod store;

pub(crate) use import::{import_config_once, migrate_routes_once};
pub(crate) use store::{CatalogConflict, CatalogStore, PricingRecord, SeaOrmCatalogStore};

#[cfg(test)]
pub(crate) use store::MemoryCatalogStore;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::config::{PricingConfig, ProviderConfig};

/// One entry in a model's ordered failover chain: which provider serves it and
/// under which upstream model name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeploymentConfig {
    pub id: Uuid,
    pub provider_id: String,
    pub upstream_model: String,
}

/// A public model: the only routing surface clients see. Deployments are in
/// priority order (first = tried first).
#[derive(Debug, Clone)]
pub(crate) struct ModelConfig {
    pub name: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub deployments: Vec<DeploymentConfig>,
}

/// The database-sourced provider, pricing, and model-catalog configuration
/// that seeds the gateway snapshot. Carried across file reloads (which only
/// re-read retry settings) and refreshed when the management APIs mutate the
/// database — mirroring how API keys are handled.
#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogSnapshot {
    pub providers: Vec<ProviderConfig>,
    pub pricing: Vec<PricingConfig>,
    pub models: Vec<ModelConfig>,
}

/// Resolves the pricing rule covering `(provider, model)` with the standard
/// wildcard fallback: provider+model → provider+`*` → `*`+model → `*`+`*`.
/// This is the coverage check behind the listing gate: an enabled model's
/// every deployment must resolve a rule.
pub(crate) fn pricing_rule_for<'a>(
    pricing: &'a [PricingConfig],
    provider: &str,
    model: &str,
) -> Option<&'a PricingConfig> {
    let lookup = [
        (provider, model),
        (provider, "*"),
        ("*", model),
        ("*", "*"),
    ];
    lookup.iter().find_map(|(p, m)| {
        pricing
            .iter()
            .find(|rule| rule.provider == *p && rule.model == *m)
    })
}

/// The deployments of `model` that no pricing rule covers, given a candidate
/// pricing rule set. Empty = fully covered = listable.
pub(crate) fn uncovered_deployments<'a>(
    model: &'a ModelConfig,
    pricing: &[PricingConfig],
) -> Vec<&'a DeploymentConfig> {
    model
        .deployments
        .iter()
        .filter(|deployment| {
            pricing_rule_for(pricing, &deployment.provider_id, &deployment.upstream_model)
                .is_none()
        })
        .collect()
}
