use std::collections::HashMap;

use anyhow::{Context, bail};
use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use uuid::Uuid;

use super::{
    CatalogSnapshot, DeploymentConfig, ModelConfig,
    entity::{model, model_deployment, pricing, provider},
};
use crate::{
    config::{PricingConfig, ProviderConfig},
    model::Protocol,
};

/// A conflict on a unique constraint (duplicate provider id, or duplicate
/// pricing provider+model pair).
#[derive(Debug)]
pub(crate) struct CatalogConflict(pub String);

impl std::fmt::Display for CatalogConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CatalogConflict {}

/// A pricing rule with its database id, for the management API.
#[derive(Debug, Clone)]
pub(crate) struct PricingRecord {
    pub id: Uuid,
    pub config: PricingConfig,
}

/// Persistence for providers and pricing. A trait so the management API can be
/// tested without PostgreSQL.
#[async_trait]
pub(crate) trait CatalogStore: Send + Sync {
    /// The full provider + pricing set that seeds the gateway snapshot.
    async fn snapshot(&self) -> anyhow::Result<CatalogSnapshot>;

    async fn list_providers(&self) -> anyhow::Result<Vec<ProviderConfig>>;
    async fn get_provider(&self, id: &str) -> anyhow::Result<Option<ProviderConfig>>;
    async fn upsert_provider(&self, provider: ProviderConfig, is_new: bool)
    -> anyhow::Result<()>;
    async fn delete_provider(&self, id: &str) -> anyhow::Result<bool>;

    async fn list_pricing(&self) -> anyhow::Result<Vec<PricingRecord>>;
    async fn create_pricing(&self, pricing: PricingConfig) -> anyhow::Result<Uuid>;
    async fn update_pricing(&self, id: Uuid, pricing: PricingConfig) -> anyhow::Result<bool>;
    async fn delete_pricing(&self, id: Uuid) -> anyhow::Result<bool>;

    /// Every model with its ordered deployments (priority ascending).
    async fn list_models(&self) -> anyhow::Result<Vec<ModelConfig>>;
    async fn get_model(&self, name: &str) -> anyhow::Result<Option<ModelConfig>>;
    /// Creates a model (disabled by default). Duplicate name → conflict.
    async fn create_model(&self, name: &str) -> anyhow::Result<ModelConfig>;
    async fn set_model_status(&self, name: &str, enabled: bool) -> anyhow::Result<bool>;
    async fn delete_model(&self, name: &str) -> anyhow::Result<bool>;
    /// Replaces a model's deployment chain wholesale, in the given order
    /// (index = priority). Duplicate providers → conflict.
    async fn set_deployments(
        &self,
        model_name: &str,
        deployments: &[(String, String)],
    ) -> anyhow::Result<bool>;

    /// Whether any deployment references `provider_id`; used to protect a
    /// provider from deletion while a model depends on it.
    async fn provider_in_use(&self, provider_id: &str) -> anyhow::Result<Option<String>>;

    async fn provider_count(&self) -> anyhow::Result<u64>;
    async fn pricing_count(&self) -> anyhow::Result<u64>;
    async fn model_count(&self) -> anyhow::Result<u64>;
}

fn provider_config(model: provider::Model) -> anyhow::Result<ProviderConfig> {
    let protocol = match model.protocol.as_str() {
        "openai" | "open-ai" => Protocol::OpenAi,
        "anthropic" => Protocol::Anthropic,
        other => bail!("provider '{}' has unknown protocol '{}'", model.id, other),
    };
    let model_aliases: HashMap<String, String> =
        serde_json::from_value(model.model_aliases).unwrap_or_default();
    Ok(ProviderConfig {
        id: model.id,
        protocol,
        base_url: model.base_url,
        api_key: model.api_key,
        anthropic_version: model.anthropic_version,
        model_aliases,
    })
}

fn pricing_config(model: pricing::Model) -> PricingConfig {
    PricingConfig {
        provider: model.provider,
        model: model.model,
        input_per_1m: model.input_per_1m,
        output_per_1m: model.output_per_1m,
        cached_input_per_1m: model.cached_input_per_1m,
        cache_read_per_1m: model.cache_read_per_1m,
        cache_write_per_1m: model.cache_write_per_1m,
        currency: model.currency,
        pricing_source: model.pricing_source,
    }
}

#[derive(Clone)]
pub(crate) struct SeaOrmCatalogStore {
    db: DatabaseConnection,
}

impl SeaOrmCatalogStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl CatalogStore for SeaOrmCatalogStore {
    async fn snapshot(&self) -> anyhow::Result<CatalogSnapshot> {
        Ok(CatalogSnapshot {
            providers: self.list_providers().await?,
            pricing: self
                .list_pricing()
                .await?
                .into_iter()
                .map(|record| record.config)
                .collect(),
            models: self.list_models().await?,
        })
    }

    async fn list_providers(&self) -> anyhow::Result<Vec<ProviderConfig>> {
        provider::Entity::find()
            .order_by_asc(provider::Column::Id)
            .all(&self.db)
            .await
            .context("failed to list providers")?
            .into_iter()
            .map(provider_config)
            .collect()
    }

    async fn get_provider(&self, id: &str) -> anyhow::Result<Option<ProviderConfig>> {
        provider::Entity::find_by_id(id.to_owned())
            .one(&self.db)
            .await
            .context("failed to query provider")?
            .map(provider_config)
            .transpose()
    }

    async fn upsert_provider(
        &self,
        provider: ProviderConfig,
        is_new: bool,
    ) -> anyhow::Result<()> {
        let aliases = serde_json::to_value(&provider.model_aliases)
            .context("failed to serialize model aliases")?;
        let now = Utc::now();
        let existing = provider::Entity::find_by_id(provider.id.clone())
            .one(&self.db)
            .await
            .context("failed to query provider")?;

        if is_new && existing.is_some() {
            bail!(CatalogConflict(format!(
                "provider '{}' already exists",
                provider.id
            )));
        }

        match existing {
            Some(model) => {
                let mut active: provider::ActiveModel = model.into();
                active.protocol = Set(provider.protocol.to_string());
                active.base_url = Set(provider.base_url);
                active.api_key = Set(provider.api_key);
                active.anthropic_version = Set(provider.anthropic_version);
                active.model_aliases = Set(aliases);
                active.updated_at = Set(now);
                active
                    .update(&self.db)
                    .await
                    .context("failed to update provider")?;
            }
            None => {
                provider::ActiveModel {
                    id: Set(provider.id),
                    protocol: Set(provider.protocol.to_string()),
                    base_url: Set(provider.base_url),
                    api_key: Set(provider.api_key),
                    anthropic_version: Set(provider.anthropic_version),
                    model_aliases: Set(aliases),
                    created_at: Set(now),
                    updated_at: Set(now),
                }
                .insert(&self.db)
                .await
                .context("failed to insert provider")?;
            }
        }
        Ok(())
    }

    async fn delete_provider(&self, id: &str) -> anyhow::Result<bool> {
        let result = provider::Entity::delete_by_id(id.to_owned())
            .exec(&self.db)
            .await
            .context("failed to delete provider")?;
        Ok(result.rows_affected > 0)
    }

    async fn list_pricing(&self) -> anyhow::Result<Vec<PricingRecord>> {
        Ok(pricing::Entity::find()
            .order_by_asc(pricing::Column::Provider)
            .order_by_asc(pricing::Column::Model)
            .all(&self.db)
            .await
            .context("failed to list pricing")?
            .into_iter()
            .map(|model| PricingRecord {
                id: model.id,
                config: pricing_config(model),
            })
            .collect())
    }

    async fn create_pricing(&self, config: PricingConfig) -> anyhow::Result<Uuid> {
        if pricing::Entity::find()
            .filter(pricing::Column::Provider.eq(config.provider.clone()))
            .filter(pricing::Column::Model.eq(config.model.clone()))
            .one(&self.db)
            .await
            .context("failed to query pricing")?
            .is_some()
        {
            bail!(CatalogConflict(format!(
                "pricing rule for '{}:{}' already exists",
                config.provider, config.model
            )));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        pricing::ActiveModel {
            id: Set(id),
            provider: Set(config.provider),
            model: Set(config.model),
            input_per_1m: Set(config.input_per_1m),
            output_per_1m: Set(config.output_per_1m),
            cached_input_per_1m: Set(config.cached_input_per_1m),
            cache_read_per_1m: Set(config.cache_read_per_1m),
            cache_write_per_1m: Set(config.cache_write_per_1m),
            currency: Set(config.currency),
            pricing_source: Set(config.pricing_source),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&self.db)
        .await
        .context("failed to insert pricing")?;
        Ok(id)
    }

    async fn update_pricing(&self, id: Uuid, config: PricingConfig) -> anyhow::Result<bool> {
        let Some(model) = pricing::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .context("failed to query pricing")?
        else {
            return Ok(false);
        };
        // A provider+model change must not collide with another rule.
        if (model.provider != config.provider || model.model != config.model)
            && pricing::Entity::find()
                .filter(pricing::Column::Provider.eq(config.provider.clone()))
                .filter(pricing::Column::Model.eq(config.model.clone()))
                .one(&self.db)
                .await
                .context("failed to query pricing")?
                .is_some()
        {
            bail!(CatalogConflict(format!(
                "pricing rule for '{}:{}' already exists",
                config.provider, config.model
            )));
        }
        let mut active: pricing::ActiveModel = model.into();
        active.provider = Set(config.provider);
        active.model = Set(config.model);
        active.input_per_1m = Set(config.input_per_1m);
        active.output_per_1m = Set(config.output_per_1m);
        active.cached_input_per_1m = Set(config.cached_input_per_1m);
        active.cache_read_per_1m = Set(config.cache_read_per_1m);
        active.cache_write_per_1m = Set(config.cache_write_per_1m);
        active.currency = Set(config.currency);
        active.pricing_source = Set(config.pricing_source);
        active.updated_at = Set(Utc::now());
        active
            .update(&self.db)
            .await
            .context("failed to update pricing")?;
        Ok(true)
    }

    async fn delete_pricing(&self, id: Uuid) -> anyhow::Result<bool> {
        let result = pricing::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .context("failed to delete pricing")?;
        Ok(result.rows_affected > 0)
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelConfig>> {
        let models = model::Entity::find()
            .order_by_asc(model::Column::Name)
            .all(&self.db)
            .await
            .context("failed to list models")?;
        let deployments = model_deployment::Entity::find()
            .order_by_asc(model_deployment::Column::ModelName)
            .order_by_asc(model_deployment::Column::Priority)
            .all(&self.db)
            .await
            .context("failed to list deployments")?;

        let mut by_model: HashMap<String, Vec<DeploymentConfig>> = HashMap::new();
        for row in deployments {
            by_model
                .entry(row.model_name)
                .or_default()
                .push(DeploymentConfig {
                    id: row.id,
                    provider_id: row.provider_id,
                    upstream_model: row.upstream_model,
                });
        }

        Ok(models
            .into_iter()
            .map(|m| ModelConfig {
                deployments: by_model.remove(&m.name).unwrap_or_default(),
                name: m.name,
                enabled: m.status == "enabled",
                created_at: m.created_at,
            })
            .collect())
    }

    async fn get_model(&self, name: &str) -> anyhow::Result<Option<ModelConfig>> {
        let Some(m) = model::Entity::find_by_id(name.to_owned())
            .one(&self.db)
            .await
            .context("failed to query model")?
        else {
            return Ok(None);
        };
        let deployments = model_deployment::Entity::find()
            .filter(model_deployment::Column::ModelName.eq(name.to_owned()))
            .order_by_asc(model_deployment::Column::Priority)
            .all(&self.db)
            .await
            .context("failed to query deployments")?
            .into_iter()
            .map(|row| DeploymentConfig {
                id: row.id,
                provider_id: row.provider_id,
                upstream_model: row.upstream_model,
            })
            .collect();
        Ok(Some(ModelConfig {
            name: m.name,
            enabled: m.status == "enabled",
            created_at: m.created_at,
            deployments,
        }))
    }

    async fn create_model(&self, name: &str) -> anyhow::Result<ModelConfig> {
        if model::Entity::find_by_id(name.to_owned())
            .one(&self.db)
            .await
            .context("failed to query model")?
            .is_some()
        {
            bail!(CatalogConflict(format!("model '{name}' already exists")));
        }
        let now = Utc::now();
        model::ActiveModel {
            name: Set(name.to_owned()),
            status: Set("disabled".to_owned()),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&self.db)
        .await
        .context("failed to insert model")?;
        Ok(ModelConfig {
            name: name.to_owned(),
            enabled: false,
            created_at: now,
            deployments: Vec::new(),
        })
    }

    async fn set_model_status(&self, name: &str, enabled: bool) -> anyhow::Result<bool> {
        let Some(existing) = model::Entity::find_by_id(name.to_owned())
            .one(&self.db)
            .await
            .context("failed to query model")?
        else {
            return Ok(false);
        };
        let mut active: model::ActiveModel = existing.into();
        active.status = Set(if enabled { "enabled" } else { "disabled" }.to_owned());
        active.updated_at = Set(Utc::now());
        active
            .update(&self.db)
            .await
            .context("failed to update model status")?;
        Ok(true)
    }

    async fn delete_model(&self, name: &str) -> anyhow::Result<bool> {
        // Deployments cascade via the FK; delete the parent row.
        let result = model::Entity::delete_by_id(name.to_owned())
            .exec(&self.db)
            .await
            .context("failed to delete model")?;
        Ok(result.rows_affected > 0)
    }

    async fn set_deployments(
        &self,
        model_name: &str,
        deployments: &[(String, String)],
    ) -> anyhow::Result<bool> {
        let mut seen = std::collections::HashSet::new();
        for (provider_id, _) in deployments {
            if !seen.insert(provider_id.clone()) {
                bail!(CatalogConflict(format!(
                    "model '{model_name}' lists provider '{provider_id}' more than once"
                )));
            }
        }

        let txn = self.db.begin().await.context("failed to begin transaction")?;
        if model::Entity::find_by_id(model_name.to_owned())
            .one(&txn)
            .await
            .context("failed to query model")?
            .is_none()
        {
            return Ok(false);
        }
        model_deployment::Entity::delete_many()
            .filter(model_deployment::Column::ModelName.eq(model_name.to_owned()))
            .exec(&txn)
            .await
            .context("failed to clear deployments")?;
        for (priority, (provider_id, upstream_model)) in deployments.iter().enumerate() {
            model_deployment::ActiveModel {
                id: Set(Uuid::new_v4()),
                model_name: Set(model_name.to_owned()),
                provider_id: Set(provider_id.clone()),
                upstream_model: Set(upstream_model.clone()),
                priority: Set(priority as i32),
            }
            .insert(&txn)
            .await
            .context("failed to insert deployment")?;
        }
        let mut active: model::ActiveModel = model::Entity::find_by_id(model_name.to_owned())
            .one(&txn)
            .await
            .context("failed to query model")?
            .expect("model existence checked above")
            .into();
        active.updated_at = Set(Utc::now());
        active
            .update(&txn)
            .await
            .context("failed to touch model")?;
        txn.commit().await.context("failed to commit deployments")?;
        Ok(true)
    }

    async fn provider_in_use(&self, provider_id: &str) -> anyhow::Result<Option<String>> {
        Ok(model_deployment::Entity::find()
            .filter(model_deployment::Column::ProviderId.eq(provider_id.to_owned()))
            .order_by_asc(model_deployment::Column::ModelName)
            .one(&self.db)
            .await
            .context("failed to query deployments")?
            .map(|row| row.model_name))
    }

    async fn provider_count(&self) -> anyhow::Result<u64> {
        use sea_orm::PaginatorTrait;
        provider::Entity::find()
            .count(&self.db)
            .await
            .context("failed to count providers")
    }

    async fn pricing_count(&self) -> anyhow::Result<u64> {
        use sea_orm::PaginatorTrait;
        pricing::Entity::find()
            .count(&self.db)
            .await
            .context("failed to count pricing")
    }

    async fn model_count(&self) -> anyhow::Result<u64> {
        use sea_orm::PaginatorTrait;
        model::Entity::find()
            .count(&self.db)
            .await
            .context("failed to count models")
    }
}

/// In-memory catalog store for tests.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct MemoryCatalogStore {
    inner: std::sync::Arc<std::sync::Mutex<MemoryInner>>,
}

#[cfg(test)]
#[derive(Default)]
struct MemoryInner {
    providers: Vec<ProviderConfig>,
    pricing: Vec<PricingRecord>,
    models: Vec<ModelConfig>,
}

#[cfg(test)]
impl MemoryCatalogStore {
    pub(crate) fn with(providers: Vec<ProviderConfig>, pricing: Vec<PricingConfig>) -> Self {
        let store = Self::default();
        {
            let mut inner = store.inner.lock().unwrap();
            inner.providers = providers;
            inner.pricing = pricing
                .into_iter()
                .map(|config| PricingRecord {
                    id: Uuid::new_v4(),
                    config,
                })
                .collect();
        }
        store
    }

    pub(crate) fn with_models(
        providers: Vec<ProviderConfig>,
        pricing: Vec<PricingConfig>,
        models: Vec<ModelConfig>,
    ) -> Self {
        let store = Self::with(providers, pricing);
        store.inner.lock().unwrap().models = models;
        store
    }
}

#[cfg(test)]
#[async_trait]
impl CatalogStore for MemoryCatalogStore {
    async fn snapshot(&self) -> anyhow::Result<CatalogSnapshot> {
        let inner = self.inner.lock().unwrap();
        Ok(CatalogSnapshot {
            providers: inner.providers.clone(),
            pricing: inner.pricing.iter().map(|r| r.config.clone()).collect(),
            models: inner.models.clone(),
        })
    }

    async fn list_providers(&self) -> anyhow::Result<Vec<ProviderConfig>> {
        Ok(self.inner.lock().unwrap().providers.clone())
    }

    async fn get_provider(&self, id: &str) -> anyhow::Result<Option<ProviderConfig>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .providers
            .iter()
            .find(|p| p.id == id)
            .cloned())
    }

    async fn upsert_provider(
        &self,
        provider: ProviderConfig,
        is_new: bool,
    ) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(existing) = inner.providers.iter_mut().find(|p| p.id == provider.id) {
            if is_new {
                bail!(CatalogConflict(format!(
                    "provider '{}' already exists",
                    provider.id
                )));
            }
            *existing = provider;
        } else {
            inner.providers.push(provider);
        }
        Ok(())
    }

    async fn delete_provider(&self, id: &str) -> anyhow::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.providers.len();
        inner.providers.retain(|p| p.id != id);
        Ok(inner.providers.len() != before)
    }

    async fn list_pricing(&self) -> anyhow::Result<Vec<PricingRecord>> {
        Ok(self.inner.lock().unwrap().pricing.clone())
    }

    async fn create_pricing(&self, config: PricingConfig) -> anyhow::Result<Uuid> {
        let mut inner = self.inner.lock().unwrap();
        if inner
            .pricing
            .iter()
            .any(|r| r.config.provider == config.provider && r.config.model == config.model)
        {
            bail!(CatalogConflict(format!(
                "pricing rule for '{}:{}' already exists",
                config.provider, config.model
            )));
        }
        let id = Uuid::new_v4();
        inner.pricing.push(PricingRecord { id, config });
        Ok(id)
    }

    async fn update_pricing(&self, id: Uuid, config: PricingConfig) -> anyhow::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        if inner
            .pricing
            .iter()
            .any(|r| r.id != id && r.config.provider == config.provider && r.config.model == config.model)
        {
            bail!(CatalogConflict(format!(
                "pricing rule for '{}:{}' already exists",
                config.provider, config.model
            )));
        }
        match inner.pricing.iter_mut().find(|r| r.id == id) {
            Some(record) => {
                record.config = config;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn delete_pricing(&self, id: Uuid) -> anyhow::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.pricing.len();
        inner.pricing.retain(|r| r.id != id);
        Ok(inner.pricing.len() != before)
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelConfig>> {
        let mut models = self.inner.lock().unwrap().models.clone();
        models.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(models)
    }

    async fn get_model(&self, name: &str) -> anyhow::Result<Option<ModelConfig>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .models
            .iter()
            .find(|m| m.name == name)
            .cloned())
    }

    async fn create_model(&self, name: &str) -> anyhow::Result<ModelConfig> {
        let mut inner = self.inner.lock().unwrap();
        if inner.models.iter().any(|m| m.name == name) {
            bail!(CatalogConflict(format!("model '{name}' already exists")));
        }
        let record = ModelConfig {
            name: name.to_owned(),
            enabled: false,
            created_at: Utc::now(),
            deployments: Vec::new(),
        };
        inner.models.push(record.clone());
        Ok(record)
    }

    async fn set_model_status(&self, name: &str, enabled: bool) -> anyhow::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        match inner.models.iter_mut().find(|m| m.name == name) {
            Some(m) => {
                m.enabled = enabled;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn delete_model(&self, name: &str) -> anyhow::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.models.len();
        inner.models.retain(|m| m.name != name);
        Ok(inner.models.len() != before)
    }

    async fn set_deployments(
        &self,
        model_name: &str,
        deployments: &[(String, String)],
    ) -> anyhow::Result<bool> {
        let mut seen = std::collections::HashSet::new();
        for (provider_id, _) in deployments {
            if !seen.insert(provider_id.clone()) {
                bail!(CatalogConflict(format!(
                    "model '{model_name}' lists provider '{provider_id}' more than once"
                )));
            }
        }
        let mut inner = self.inner.lock().unwrap();
        match inner.models.iter_mut().find(|m| m.name == model_name) {
            Some(m) => {
                m.deployments = deployments
                    .iter()
                    .map(|(provider_id, upstream_model)| DeploymentConfig {
                        id: Uuid::new_v4(),
                        provider_id: provider_id.clone(),
                        upstream_model: upstream_model.clone(),
                    })
                    .collect();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn provider_in_use(&self, provider_id: &str) -> anyhow::Result<Option<String>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .models
            .iter()
            .find(|m| m.deployments.iter().any(|d| d.provider_id == provider_id))
            .map(|m| m.name.clone()))
    }

    async fn provider_count(&self) -> anyhow::Result<u64> {
        Ok(self.inner.lock().unwrap().providers.len() as u64)
    }

    async fn pricing_count(&self) -> anyhow::Result<u64> {
        Ok(self.inner.lock().unwrap().pricing.len() as u64)
    }

    async fn model_count(&self) -> anyhow::Result<u64> {
        Ok(self.inner.lock().unwrap().models.len() as u64)
    }
}
