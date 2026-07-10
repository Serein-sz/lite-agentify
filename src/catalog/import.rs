use std::collections::HashMap;

use tracing::{info, warn};

use super::{ModelConfig, store::CatalogStore, uncovered_deployments};
use crate::config::GatewayConfig;

/// One-time file → database import, run at startup:
///
/// - When the `providers` table is empty and the config file has `[[providers]]`,
///   import them (aliases included).
/// - When the `pricing` table is empty and the file has `[[pricing]]`, import
///   those rules.
///
/// With non-empty tables the file sections are dead: a warning is logged and
/// they are not applied. Import is idempotent — it never runs against
/// populated tables.
pub(crate) async fn import_config_once(
    store: &dyn CatalogStore,
    config: &GatewayConfig,
) -> anyhow::Result<()> {
    let provider_count = store.provider_count().await?;
    if provider_count == 0 && !config.providers.is_empty() {
        for provider in &config.providers {
            store.upsert_provider(provider.clone(), true).await?;
        }
        info!(
            count = config.providers.len(),
            "imported providers from the config file into the database; \
             remove the [[providers]] sections from the file"
        );
    } else if !config.providers.is_empty() {
        warn!(
            "config file still contains [[providers]] sections that are no longer used; \
             providers are managed in the database — remove them from the file"
        );
    }

    let pricing_count = store.pricing_count().await?;
    if pricing_count == 0 && !config.pricing.is_empty() {
        for pricing in &config.pricing {
            store.create_pricing(pricing.clone()).await?;
        }
        info!(
            count = config.pricing.len(),
            "imported pricing rules from the config file into the database; \
             remove the [[pricing]] sections from the file"
        );
    } else if !config.pricing.is_empty() {
        warn!(
            "config file still contains [[pricing]] sections that are no longer used; \
             pricing is managed in the database — remove them from the file"
        );
    }

    Ok(())
}

/// One-time file routes → model catalog migration, run at startup after the
/// provider/pricing import:
///
/// - When the `models` table is empty and the config file has `[[routes]]`,
///   derive catalog entries: for each route, for each provider in its chain,
///   each model alias `(public → upstream)` becomes a deployment of model
///   `public` at the route's chain position.
/// - Chain providers without aliases imply pass-through models that cannot be
///   enumerated; each is logged with a "create catalog entries manually" hint.
/// - Migrated models start enabled only when every deployment has pricing
///   coverage (wildcard fallback included) — the listing gate stays honest.
///
/// With a non-empty `models` table the file routes are dead: a warning is
/// logged and nothing is applied. Provider alias data is left intact this
/// release so a pre-catalog binary can still roll back.
pub(crate) async fn migrate_routes_once(
    store: &dyn CatalogStore,
    config: &GatewayConfig,
) -> anyhow::Result<()> {
    if config.routes.is_empty() {
        return Ok(());
    }
    if store.model_count().await? > 0 {
        warn!(
            "config file still contains [[routes]] sections that are no longer used; \
             routing is defined by the model catalog — remove them from the file"
        );
        return Ok(());
    }

    let providers: HashMap<String, _> = store
        .list_providers()
        .await?
        .into_iter()
        .map(|provider| (provider.id.clone(), provider))
        .collect();
    let pricing: Vec<_> = store
        .list_pricing()
        .await?
        .into_iter()
        .map(|record| record.config)
        .collect();

    // model name → ordered (provider, upstream) chain; first occurrence wins
    // both the chain position and duplicate (model, provider) pairs.
    let mut chains: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for route in &config.routes {
        for provider_id in &route.providers {
            let Some(provider) = providers.get(provider_id) else {
                warn!(
                    provider = %provider_id,
                    route = %route.path_prefix,
                    "route references a provider that is not in the database; skipping it in the catalog migration"
                );
                continue;
            };
            if provider.model_aliases.is_empty() {
                warn!(
                    provider = %provider_id,
                    route = %route.path_prefix,
                    "provider has no model aliases, so its pass-through models cannot be derived; \
                     create its catalog entries manually in the console"
                );
                continue;
            }
            for (public, upstream) in &provider.model_aliases {
                let chain = match chains.get_mut(public) {
                    Some(chain) => chain,
                    None => {
                        order.push(public.clone());
                        chains.entry(public.clone()).or_default()
                    }
                };
                if !chain.iter().any(|(existing, _)| existing == provider_id) {
                    chain.push((provider_id.clone(), upstream.clone()));
                }
            }
        }
    }

    if chains.is_empty() {
        warn!(
            "config file has [[routes]] but no catalog entries could be derived; \
             create models manually in the console and remove the routes from the file"
        );
        return Ok(());
    }

    let mut enabled_count = 0usize;
    let mut disabled = Vec::new();
    for name in &order {
        let chain = &chains[name];
        store.create_model(name).await?;
        store.set_deployments(name, chain).await?;
        let model = ModelConfig {
            name: name.clone(),
            enabled: false,
            created_at: chrono::Utc::now(),
            deployments: chain
                .iter()
                .map(|(provider_id, upstream_model)| super::DeploymentConfig {
                    id: uuid::Uuid::new_v4(),
                    provider_id: provider_id.clone(),
                    upstream_model: upstream_model.clone(),
                })
                .collect(),
        };
        if uncovered_deployments(&model, &pricing).is_empty() {
            store.set_model_status(name, true).await?;
            enabled_count += 1;
        } else {
            disabled.push(name.clone());
        }
    }

    info!(
        models = order.len(),
        enabled = enabled_count,
        "migrated file routes and provider aliases into the model catalog; \
         remove the [[routes]] sections from the file"
    );
    if !disabled.is_empty() {
        warn!(
            models = %disabled.join(", "),
            "migrated models without full pricing coverage start disabled; \
             add pricing rules and enable them in the console"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::{
        catalog::MemoryCatalogStore,
        config::{PricingConfig, ProviderConfig, RouteConfig},
        model::{Protocol, default_listen_addr},
    };

    fn provider(id: &str, aliases: &[(&str, &str)]) -> ProviderConfig {
        ProviderConfig {
            id: id.to_owned(),
            protocol: Protocol::OpenAi,
            base_url: format!("http://{id}.test"),
            api_key: format!("{id}-secret"),
            anthropic_version: None,
            model_aliases: aliases
                .iter()
                .map(|(public, upstream)| ((*public).to_owned(), (*upstream).to_owned()))
                .collect(),
        }
    }

    fn wildcard_pricing() -> PricingConfig {
        PricingConfig {
            provider: "*".to_owned(),
            model: "*".to_owned(),
            input_per_1m: Decimal::ONE,
            output_per_1m: Decimal::ONE,
            cached_input_per_1m: None,
            cache_read_per_1m: None,
            cache_write_per_1m: None,
            currency: "USD".to_owned(),
            pricing_source: None,
        }
    }

    fn config_with_routes(routes: Vec<RouteConfig>) -> GatewayConfig {
        GatewayConfig {
            listen_addr: default_listen_addr(),
            gateway_keys: Vec::new(),
            admin_password: None,
            providers: Vec::new(),
            routes,
            database: None,
            redis: None,
            pricing: Vec::new(),
            retry: Default::default(),
        }
    }

    fn route(prefix: &str, providers: &[&str]) -> RouteConfig {
        RouteConfig {
            path_prefix: prefix.to_owned(),
            providers: providers.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    async fn run(store: &MemoryCatalogStore, routes: Vec<RouteConfig>) {
        migrate_routes_once(store, &config_with_routes(routes))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn aliased_chain_becomes_models_with_route_order_priority() {
        let store = MemoryCatalogStore::with_models(
            vec![
                provider("primary", &[("public-chat", "primary-real")]),
                provider("fallback", &[("public-chat", "fallback-real")]),
            ],
            vec![wildcard_pricing()],
            Vec::new(),
        );

        run(&store, vec![route("/v1/chat/completions", &["primary", "fallback"])]).await;

        let model = store.get_model("public-chat").await.unwrap().unwrap();
        // Chain order follows the route's provider order; pricing coverage
        // (wildcard) enables the model.
        assert!(model.enabled);
        assert_eq!(
            model
                .deployments
                .iter()
                .map(|d| (d.provider_id.as_str(), d.upstream_model.as_str()))
                .collect::<Vec<_>>(),
            vec![("primary", "primary-real"), ("fallback", "fallback-real")]
        );
    }

    #[tokio::test]
    async fn unpriced_migrated_model_starts_disabled() {
        let store = MemoryCatalogStore::with_models(
            vec![provider("openai", &[("public-chat", "gpt-real")])],
            Vec::new(), // no pricing at all
            Vec::new(),
        );

        run(&store, vec![route("/v1/chat/completions", &["openai"])]).await;

        let model = store.get_model("public-chat").await.unwrap().unwrap();
        assert!(!model.enabled);
    }

    #[tokio::test]
    async fn alias_less_provider_is_skipped_with_no_model() {
        let store = MemoryCatalogStore::with_models(
            vec![provider("passthrough", &[])],
            vec![wildcard_pricing()],
            Vec::new(),
        );

        run(&store, vec![route("/v1/chat/completions", &["passthrough"])]).await;

        // Nothing derivable: the catalog stays empty (warned, not failed).
        assert_eq!(store.model_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn mixed_chain_migrates_only_aliased_providers() {
        let store = MemoryCatalogStore::with_models(
            vec![
                provider("aliased", &[("public-chat", "real-chat")]),
                provider("passthrough", &[]),
            ],
            vec![wildcard_pricing()],
            Vec::new(),
        );

        run(
            &store,
            vec![route("/v1/chat/completions", &["aliased", "passthrough"])],
        )
        .await;

        let model = store.get_model("public-chat").await.unwrap().unwrap();
        assert_eq!(model.deployments.len(), 1);
        assert_eq!(model.deployments[0].provider_id, "aliased");
    }

    #[tokio::test]
    async fn migration_skips_when_models_already_exist() {
        let store = MemoryCatalogStore::with_models(
            vec![provider("openai", &[("public-chat", "gpt-real")])],
            vec![wildcard_pricing()],
            vec![crate::catalog::ModelConfig {
                name: "existing".to_owned(),
                enabled: true,
                created_at: chrono::Utc::now(),
                deployments: Vec::new(),
            }],
        );

        run(&store, vec![route("/v1/chat/completions", &["openai"])]).await;

        // Idempotent: the populated table means the routes are dead sections.
        assert_eq!(store.model_count().await.unwrap(), 1);
        assert!(store.get_model("public-chat").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn duplicate_model_provider_pairs_across_routes_keep_first_position() {
        let store = MemoryCatalogStore::with_models(
            vec![provider("openai", &[("public-chat", "gpt-real")])],
            vec![wildcard_pricing()],
            Vec::new(),
        );

        // The same provider appears in two routes: one deployment, not two.
        run(
            &store,
            vec![
                route("/v1/chat/completions", &["openai"]),
                route("/v1/responses", &["openai"]),
            ],
        )
        .await;

        let model = store.get_model("public-chat").await.unwrap().unwrap();
        assert_eq!(model.deployments.len(), 1);
    }
}
