use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Context;
use sea_orm::{ConnectOptions, Database, DatabaseConnection, EntityTrait, Set};
use tracing::warn;
use uuid::Uuid;

use super::entity::usage_record;
use super::record::UsageRecord;
use crate::gateway::config::UsageDatabaseConfig;

pub(crate) type UsageRecordFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

pub(crate) trait UsageRecorder: Send + Sync {
    fn record(&self, record: UsageRecord) -> UsageRecordFuture;
}

#[derive(Clone, Default)]
pub(crate) struct NoopUsageRecorder;

impl UsageRecorder for NoopUsageRecorder {
    fn record(&self, _record: UsageRecord) -> UsageRecordFuture {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct MemoryUsageRecorder {
    records: Arc<std::sync::Mutex<Vec<UsageRecord>>>,
    fail_writes: bool,
}

#[cfg(test)]
impl MemoryUsageRecorder {
    pub(crate) fn failing() -> Self {
        Self {
            fail_writes: true,
            ..Self::default()
        }
    }

    pub(crate) fn records(&self) -> Vec<UsageRecord> {
        self.records.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl UsageRecorder for MemoryUsageRecorder {
    fn record(&self, record: UsageRecord) -> UsageRecordFuture {
        let records = self.records.clone();
        let fail_writes = self.fail_writes;
        Box::pin(async move {
            if fail_writes {
                anyhow::bail!("simulated usage write failure");
            }
            records.lock().unwrap().push(record);
            Ok(())
        })
    }
}

pub(crate) struct SeaOrmUsageRecorder {
    db: DatabaseConnection,
}

impl SeaOrmUsageRecorder {
    pub(crate) async fn connect(config: &UsageDatabaseConfig) -> anyhow::Result<Self> {
        let mut options = ConnectOptions::new(config.url.clone());
        if let Some(max_connections) = config.max_connections {
            options.max_connections(max_connections);
        }

        let db = Database::connect(options)
            .await
            .context("failed to connect usage database")?;
        Ok(Self { db })
    }
}

impl UsageRecorder for SeaOrmUsageRecorder {
    fn record(&self, record: UsageRecord) -> UsageRecordFuture {
        let db = self.db.clone();
        Box::pin(async move {
            let active = usage_record::ActiveModel {
                id: Set(Uuid::new_v4()),
                request_id: Set(record.request_id),
                created_at: Set(record.created_at),
                provider_id: Set(record.provider_id),
                protocol: Set(record.protocol.to_string()),
                path: Set(record.path),
                requested_model: Set(record.requested_model),
                upstream_model: Set(record.upstream_model),
                status: Set(record.status as i32),
                latency_ms: Set(record.latency_ms),
                input_tokens: Set(record.input_tokens),
                output_tokens: Set(record.output_tokens),
                cached_input_tokens: Set(record.cached_input_tokens),
                cache_read_tokens: Set(record.cache_read_tokens),
                cache_write_tokens: Set(record.cache_write_tokens),
                total_tokens: Set(record.total_tokens),
                estimated_cost: Set(record.estimated_cost),
                currency: Set(record.currency),
                usage_source: Set(record.usage_source.to_string()),
                pricing_source: Set(record.pricing_source),
            };
            usage_record::Entity::insert(active)
                .exec_without_returning(&db)
                .await
                .context("failed to insert usage record")?;
            Ok(())
        })
    }
}

pub(crate) async fn recorder_from_config(
    config: Option<&UsageDatabaseConfig>,
) -> anyhow::Result<Arc<dyn UsageRecorder>> {
    let Some(config) = config else {
        return Ok(Arc::new(NoopUsageRecorder));
    };

    if !config.enabled {
        return Ok(Arc::new(NoopUsageRecorder));
    }

    Ok(Arc::new(SeaOrmUsageRecorder::connect(config).await?))
}

pub(crate) fn warn_record_error(error: anyhow::Error) {
    warn!(error = %error, error_chain = ?error, "failed to record usage");
}
