use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use anyhow::Context;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, EntityTrait, Set,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::warn;
use uuid::Uuid;

use super::entity::usage_record;
use super::query::{
    CostByCurrency, SummaryBucket, UsageBreakdownRow, UsageListParams, UsagePage, UsageQuery,
    UsageQueryFuture, UsageRow, UsageSeriesPoint, UsageSummary, UsageSummaryParams, UsageTotals,
};
use super::record::UsageRecord;
use crate::config::UsageDatabaseConfig;

pub(crate) type UsageRecordFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

/// Bounded backlog of usage records awaiting a batched insert. A full channel
/// drops the record rather than blocking the proxy response path.
const USAGE_CHANNEL_CAPACITY: usize = 1024;
/// Flush the buffer once this many records accumulate.
const USAGE_BATCH_SIZE: usize = 128;
/// Flush a non-empty buffer at least this often, even below the batch size.
const USAGE_FLUSH_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) trait UsageRecorder: Send + Sync {
    fn record(&self, record: UsageRecord) -> UsageRecordFuture;

    /// Read access to recorded usage; `None` when this recorder has no
    /// readable store (usage recording disabled).
    fn query(&self) -> Option<&dyn UsageQuery> {
        None
    }

    /// Flushes any buffered records and stops background work before exit.
    /// The default is a no-op for recorders that write synchronously or not
    /// at all; the batched SeaORM recorder overrides it to drain its queue.
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
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
    fn query(&self) -> Option<&dyn UsageQuery> {
        Some(self)
    }

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

/// Persists usage records through a background batch writer. `record()` only
/// enqueues onto a bounded channel and returns immediately, so the proxy
/// response path never awaits a database round-trip. Reads go straight to the
/// database connection, which is shared with the writer task.
pub(crate) struct SeaOrmUsageRecorder {
    db: DatabaseConnection,
    sender: mpsc::Sender<UsageRecord>,
    /// Held so the recorder can flush and join the writer on shutdown; taken
    /// once by `shutdown`.
    writer: std::sync::Mutex<Option<WriterHandle>>,
}

/// The background writer task plus the shutdown signal that tells it to drain
/// and flush before exiting.
struct WriterHandle {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
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

        let (sender, receiver) = mpsc::channel(USAGE_CHANNEL_CAPACITY);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let sink = DbSink { db: db.clone() };
        let task = tokio::spawn(async move {
            run_writer(sink, receiver, shutdown_rx).await;
        });

        Ok(Self {
            db,
            sender,
            writer: std::sync::Mutex::new(Some(WriterHandle {
                shutdown: shutdown_tx,
                task,
            })),
        })
    }

}

impl UsageRecorder for SeaOrmUsageRecorder {
    fn query(&self) -> Option<&dyn UsageQuery> {
        Some(self)
    }

    /// Signals the writer task to drain the channel, flush the final batch, and
    /// exit, then awaits its completion. Idempotent: later calls are no-ops.
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let handle = self.writer.lock().unwrap().take();
            if let Some(handle) = handle {
                // Dropping our sender lets the writer see the channel close
                // after it drains; the oneshot tells it to stop waiting for
                // new records.
                let _ = handle.shutdown.send(());
                if let Err(error) = handle.task.await {
                    warn!(%error, "usage writer task did not shut down cleanly");
                }
            }
        })
    }

    fn record(&self, record: UsageRecord) -> UsageRecordFuture {
        // Non-blocking enqueue: a full channel drops the record with a warning
        // rather than stalling the proxy. Usage is best-effort by design.
        match self.sender.try_send(record) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!("usage write buffer full; dropping usage record");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("usage writer channel closed; dropping usage record");
            }
        }
        Box::pin(async { Ok(()) })
    }
}

fn active_from_record(record: UsageRecord) -> usage_record::ActiveModel {
    usage_record::ActiveModel {
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
    }
}

/// Receives batches of records to persist. The production sink writes to
/// Postgres; tests use an in-memory sink to observe the writer's batching and
/// flush behavior without a database.
trait BatchSink: Send + Sync + 'static {
    fn flush(&self, batch: Vec<UsageRecord>) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Inserts a batch in one statement; on failure logs and drops the batch so a
/// transient database problem never wedges the writer or grows memory.
struct DbSink {
    db: DatabaseConnection,
}

impl BatchSink for DbSink {
    fn flush(&self, batch: Vec<UsageRecord>) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if batch.is_empty() {
                return;
            }
            let count = batch.len();
            let models = batch.into_iter().map(active_from_record);
            if let Err(error) = usage_record::Entity::insert_many(models)
                .exec_without_returning(&self.db)
                .await
            {
                warn!(%error, count, "failed to insert usage record batch; dropping it");
            }
        })
    }
}

/// Drains the channel, flushing to the sink on batch size or interval. On
/// shutdown it drains whatever remains and flushes a final batch before
/// returning.
async fn run_writer<S: BatchSink>(
    sink: S,
    mut receiver: mpsc::Receiver<UsageRecord>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let mut buffer: Vec<UsageRecord> = Vec::with_capacity(USAGE_BATCH_SIZE);
    let mut ticker = tokio::time::interval(USAGE_FLUSH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            maybe_record = receiver.recv() => {
                match maybe_record {
                    Some(record) => {
                        buffer.push(record);
                        if buffer.len() >= USAGE_BATCH_SIZE {
                            sink.flush(std::mem::take(&mut buffer)).await;
                        }
                    }
                    // All senders dropped: drain nothing more, flush and exit.
                    None => break,
                }
            }
            _ = ticker.tick() => {
                if !buffer.is_empty() {
                    sink.flush(std::mem::take(&mut buffer)).await;
                }
            }
            _ = &mut shutdown => {
                // Drain any records already queued, then stop.
                while let Ok(record) = receiver.try_recv() {
                    buffer.push(record);
                }
                break;
            }
        }
    }

    sink.flush(std::mem::take(&mut buffer)).await;
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

/// Builds a `WHERE` clause with Postgres `$N` placeholders from list filters.
struct SqlFilter {
    clauses: Vec<String>,
    values: Vec<sea_orm::Value>,
}

impl SqlFilter {
    fn from_params(
        from: Option<chrono::DateTime<chrono::Utc>>,
        to: Option<chrono::DateTime<chrono::Utc>>,
        provider: Option<String>,
        model: Option<String>,
        status: Option<super::query::StatusFilter>,
    ) -> Self {
        let mut filter = Self {
            clauses: Vec::new(),
            values: Vec::new(),
        };
        if let Some(from) = from {
            filter.push_value("created_at >= ", from);
        }
        if let Some(to) = to {
            filter.push_value("created_at <= ", to);
        }
        if let Some(provider) = provider {
            filter.push_value("provider_id = ", provider);
        }
        if let Some(model) = model {
            let first = filter.next_placeholder();
            filter.values.push(model.clone().into());
            let second = filter.next_placeholder();
            filter.values.push(model.into());
            filter
                .clauses
                .push(format!("(requested_model = {first} OR upstream_model = {second})"));
        }
        match status {
            Some(super::query::StatusFilter::Exact(code)) => {
                filter.push_value("status = ", i32::from(code));
            }
            Some(super::query::StatusFilter::ClientError) => {
                filter
                    .clauses
                    .push("status >= 400 AND status < 500".to_owned());
            }
            Some(super::query::StatusFilter::ServerError) => {
                filter.clauses.push("status >= 500".to_owned());
            }
            None => {}
        }
        filter
    }

    fn next_placeholder(&self) -> String {
        format!("${}", self.values.len() + 1)
    }

    fn push_value(&mut self, prefix: &str, value: impl Into<sea_orm::Value>) {
        let placeholder = self.next_placeholder();
        self.clauses.push(format!("{prefix}{placeholder}"));
        self.values.push(value.into());
    }

    fn where_clause(&self) -> String {
        if self.clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", self.clauses.join(" AND "))
        }
    }
}

fn fold_cost(
    map: &mut std::collections::BTreeMap<String, rust_decimal::Decimal>,
    currency: Option<String>,
    amount: Option<rust_decimal::Decimal>,
) {
    if let (Some(currency), Some(amount)) = (currency, amount) {
        *map.entry(currency).or_default() += amount;
    }
}

fn cost_vec(map: std::collections::BTreeMap<String, rust_decimal::Decimal>) -> Vec<CostByCurrency> {
    map.into_iter()
        .map(|(currency, amount)| CostByCurrency { currency, amount })
        .collect()
}

impl UsageQuery for SeaOrmUsageRecorder {
    fn list(&self, params: UsageListParams) -> UsageQueryFuture<UsagePage> {
        let db = self.db.clone();
        Box::pin(async move {
            let filter = SqlFilter::from_params(
                params.from,
                params.to,
                params.provider,
                params.model,
                params.status,
            );
            let where_clause = filter.where_clause();

            let count_statement = sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                format!("SELECT COUNT(*)::bigint AS total FROM usage_records{where_clause}"),
                filter.values.clone(),
            );
            let total = db
                .query_one(count_statement)
                .await
                .context("failed to count usage records")?
                .map(|row| row.try_get::<i64>("", "total"))
                .transpose()
                .context("failed to read usage record count")?
                .unwrap_or(0)
                .max(0) as u64;

            let mut values = filter.values.clone();
            let limit_placeholder = format!("${}", values.len() + 1);
            values.push((params.page_size as i64).into());
            let offset_placeholder = format!("${}", values.len() + 1);
            values.push((params.page.saturating_sub(1).saturating_mul(params.page_size) as i64).into());

            let list_statement = sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                format!(
                    "SELECT request_id, created_at, provider_id, protocol, path, requested_model, \
                     upstream_model, status, latency_ms, input_tokens, output_tokens, \
                     cached_input_tokens, cache_read_tokens, cache_write_tokens, total_tokens, \
                     estimated_cost, currency, usage_source \
                     FROM usage_records{where_clause} \
                     ORDER BY created_at DESC LIMIT {limit_placeholder} OFFSET {offset_placeholder}"
                ),
                values,
            );
            let rows = db
                .query_all(list_statement)
                .await
                .context("failed to list usage records")?
                .into_iter()
                .map(|row| {
                    Ok(UsageRow {
                        request_id: row.try_get("", "request_id")?,
                        created_at: row.try_get("", "created_at")?,
                        provider_id: row.try_get("", "provider_id")?,
                        protocol: row.try_get("", "protocol")?,
                        path: row.try_get("", "path")?,
                        requested_model: row.try_get("", "requested_model")?,
                        upstream_model: row.try_get("", "upstream_model")?,
                        status: row.try_get::<i32>("", "status")?.clamp(0, u16::MAX as i32) as u16,
                        latency_ms: row.try_get("", "latency_ms")?,
                        input_tokens: row.try_get("", "input_tokens")?,
                        output_tokens: row.try_get("", "output_tokens")?,
                        cached_input_tokens: row.try_get("", "cached_input_tokens")?,
                        cache_read_tokens: row.try_get("", "cache_read_tokens")?,
                        cache_write_tokens: row.try_get("", "cache_write_tokens")?,
                        total_tokens: row.try_get("", "total_tokens")?,
                        estimated_cost: row.try_get("", "estimated_cost")?,
                        currency: row.try_get("", "currency")?,
                        usage_source: row.try_get("", "usage_source")?,
                    })
                })
                .collect::<Result<Vec<_>, sea_orm::DbErr>>()
                .context("failed to read usage record row")?;

            Ok(UsagePage { rows, total })
        })
    }

    fn summary(&self, params: UsageSummaryParams) -> UsageQueryFuture<UsageSummary> {
        let db = self.db.clone();
        Box::pin(async move {
            let filter =
                SqlFilter::from_params(params.from, params.to, None, None, None);
            let where_clause = filter.where_clause();
            let bucket = match params.bucket {
                SummaryBucket::Hour => "hour",
                SummaryBucket::Day => "day",
            };

            let totals_statement = sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                format!(
                    "SELECT COUNT(*)::bigint AS requests, \
                     COALESCE(SUM(input_tokens),0)::bigint AS input_tokens, \
                     COALESCE(SUM(output_tokens),0)::bigint AS output_tokens, \
                     COALESCE(SUM(total_tokens),0)::bigint AS total_tokens, \
                     COALESCE(AVG(latency_ms)::float8,0) AS avg_latency_ms, \
                     COUNT(*) FILTER (WHERE status >= 400)::bigint AS errors \
                     FROM usage_records{where_clause}"
                ),
                filter.values.clone(),
            );
            let totals_row = db
                .query_one(totals_statement)
                .await
                .context("failed to aggregate usage totals")?
                .context("usage totals query returned no row")?;
            let requests = totals_row.try_get::<i64>("", "requests")?.max(0) as u64;
            let errors = totals_row.try_get::<i64>("", "errors")?.max(0) as u64;
            let mut totals = UsageTotals {
                requests,
                input_tokens: totals_row.try_get("", "input_tokens")?,
                output_tokens: totals_row.try_get("", "output_tokens")?,
                total_tokens: totals_row.try_get("", "total_tokens")?,
                avg_latency_ms: totals_row.try_get("", "avg_latency_ms")?,
                error_rate: if requests > 0 {
                    errors as f64 / requests as f64
                } else {
                    0.0
                },
                cost: Vec::new(),
            };

            let cost_statement = sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                format!(
                    "SELECT currency, SUM(estimated_cost) AS amount FROM usage_records{where_clause}{} \
                     currency IS NOT NULL AND estimated_cost IS NOT NULL \
                     GROUP BY currency ORDER BY currency",
                    if where_clause.is_empty() { " WHERE" } else { " AND" }
                ),
                filter.values.clone(),
            );
            let mut total_cost = std::collections::BTreeMap::new();
            for row in db
                .query_all(cost_statement)
                .await
                .context("failed to aggregate usage cost")?
            {
                fold_cost(
                    &mut total_cost,
                    row.try_get("", "currency")?,
                    row.try_get("", "amount")?,
                );
            }
            totals.cost = cost_vec(total_cost);

            let series_statement = sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                format!(
                    "SELECT date_trunc('{bucket}', created_at) AS bucket_start, currency, \
                     COUNT(*)::bigint AS requests, \
                     COALESCE(SUM(total_tokens),0)::bigint AS total_tokens, \
                     SUM(estimated_cost) AS amount \
                     FROM usage_records{where_clause} \
                     GROUP BY bucket_start, currency ORDER BY bucket_start"
                ),
                filter.values.clone(),
            );
            let mut series: std::collections::BTreeMap<
                chrono::DateTime<chrono::Utc>,
                (u64, i64, std::collections::BTreeMap<String, rust_decimal::Decimal>),
            > = std::collections::BTreeMap::new();
            for row in db
                .query_all(series_statement)
                .await
                .context("failed to aggregate usage series")?
            {
                let bucket_start: chrono::DateTime<chrono::Utc> =
                    row.try_get("", "bucket_start")?;
                let entry = series.entry(bucket_start).or_default();
                entry.0 += row.try_get::<i64>("", "requests")?.max(0) as u64;
                entry.1 += row.try_get::<i64>("", "total_tokens")?;
                fold_cost(
                    &mut entry.2,
                    row.try_get("", "currency")?,
                    row.try_get("", "amount")?,
                );
            }

            let breakdown_statement = sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                format!(
                    "SELECT provider_id, upstream_model, currency, \
                     COUNT(*)::bigint AS requests, \
                     COALESCE(SUM(total_tokens),0)::bigint AS total_tokens, \
                     SUM(estimated_cost) AS amount \
                     FROM usage_records{where_clause} \
                     GROUP BY provider_id, upstream_model, currency \
                     ORDER BY provider_id, upstream_model"
                ),
                filter.values,
            );
            let mut breakdown: std::collections::BTreeMap<
                (String, Option<String>),
                (u64, i64, std::collections::BTreeMap<String, rust_decimal::Decimal>),
            > = std::collections::BTreeMap::new();
            for row in db
                .query_all(breakdown_statement)
                .await
                .context("failed to aggregate usage breakdown")?
            {
                let key = (
                    row.try_get::<String>("", "provider_id")?,
                    row.try_get::<Option<String>>("", "upstream_model")?,
                );
                let entry = breakdown.entry(key).or_default();
                entry.0 += row.try_get::<i64>("", "requests")?.max(0) as u64;
                entry.1 += row.try_get::<i64>("", "total_tokens")?;
                fold_cost(
                    &mut entry.2,
                    row.try_get("", "currency")?,
                    row.try_get("", "amount")?,
                );
            }

            Ok(UsageSummary {
                totals,
                series: series
                    .into_iter()
                    .map(|(bucket_start, (requests, total_tokens, cost))| UsageSeriesPoint {
                        bucket_start,
                        requests,
                        total_tokens,
                        cost: cost_vec(cost),
                    })
                    .collect(),
                breakdown: breakdown
                    .into_iter()
                    .map(
                        |((provider_id, model), (requests, total_tokens, cost))| {
                            UsageBreakdownRow {
                                provider_id,
                                model,
                                requests,
                                total_tokens,
                                cost: cost_vec(cost),
                            }
                        },
                    )
                    .collect(),
            })
        })
    }
}

#[cfg(test)]
impl MemoryUsageRecorder {
    fn filtered(
        &self,
        from: Option<chrono::DateTime<chrono::Utc>>,
        to: Option<chrono::DateTime<chrono::Utc>>,
        provider: Option<&str>,
        model: Option<&str>,
        status: Option<super::query::StatusFilter>,
    ) -> Vec<UsageRecord> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                from.is_none_or(|from| record.created_at >= from)
                    && to.is_none_or(|to| record.created_at <= to)
                    && provider.is_none_or(|provider| record.provider_id == provider)
                    && model.is_none_or(|model| {
                        record.requested_model.as_deref() == Some(model)
                            || record.upstream_model.as_deref() == Some(model)
                    })
                    && status.is_none_or(|status| status.matches(record.status))
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
fn usage_row_from_record(record: &UsageRecord) -> UsageRow {
    UsageRow {
        request_id: record.request_id.clone(),
        created_at: record.created_at,
        provider_id: record.provider_id.clone(),
        protocol: record.protocol.to_string(),
        path: record.path.clone(),
        requested_model: record.requested_model.clone(),
        upstream_model: record.upstream_model.clone(),
        status: record.status,
        latency_ms: record.latency_ms,
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        cached_input_tokens: record.cached_input_tokens,
        cache_read_tokens: record.cache_read_tokens,
        cache_write_tokens: record.cache_write_tokens,
        total_tokens: record.total_tokens,
        estimated_cost: record.estimated_cost,
        currency: record.currency.clone(),
        usage_source: record.usage_source.to_string(),
    }
}

#[cfg(test)]
fn truncate_to_bucket(
    timestamp: chrono::DateTime<chrono::Utc>,
    bucket: SummaryBucket,
) -> chrono::DateTime<chrono::Utc> {
    use chrono::{TimeZone, Timelike};

    let naive = match bucket {
        SummaryBucket::Day => timestamp.date_naive().and_hms_opt(0, 0, 0).unwrap(),
        SummaryBucket::Hour => timestamp
            .naive_utc()
            .with_minute(0)
            .and_then(|value| value.with_second(0))
            .and_then(|value| value.with_nanosecond(0))
            .unwrap(),
    };
    chrono::Utc.from_utc_datetime(&naive)
}

#[cfg(test)]
impl UsageQuery for MemoryUsageRecorder {
    fn list(&self, params: UsageListParams) -> UsageQueryFuture<UsagePage> {
        let mut records = self.filtered(
            params.from,
            params.to,
            params.provider.as_deref(),
            params.model.as_deref(),
            params.status,
        );
        records.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let total = records.len() as u64;
        let rows = records
            .iter()
            .skip((params.page.saturating_sub(1) * params.page_size) as usize)
            .take(params.page_size as usize)
            .map(usage_row_from_record)
            .collect();
        Box::pin(async move { Ok(UsagePage { rows, total }) })
    }

    fn summary(&self, params: UsageSummaryParams) -> UsageQueryFuture<UsageSummary> {
        let records = self.filtered(params.from, params.to, None, None, None);

        let requests = records.len() as u64;
        let errors = records.iter().filter(|record| record.status >= 400).count() as u64;
        let mut total_cost = std::collections::BTreeMap::new();
        for record in &records {
            fold_cost(&mut total_cost, record.currency.clone(), record.estimated_cost);
        }
        let totals = UsageTotals {
            requests,
            input_tokens: records.iter().filter_map(|record| record.input_tokens).sum(),
            output_tokens: records.iter().filter_map(|record| record.output_tokens).sum(),
            total_tokens: records.iter().filter_map(|record| record.total_tokens).sum(),
            avg_latency_ms: if requests > 0 {
                records.iter().map(|record| record.latency_ms).sum::<i64>() as f64
                    / requests as f64
            } else {
                0.0
            },
            error_rate: if requests > 0 {
                errors as f64 / requests as f64
            } else {
                0.0
            },
            cost: cost_vec(total_cost),
        };

        let mut series: std::collections::BTreeMap<
            chrono::DateTime<chrono::Utc>,
            (u64, i64, std::collections::BTreeMap<String, rust_decimal::Decimal>),
        > = std::collections::BTreeMap::new();
        let mut breakdown: std::collections::BTreeMap<
            (String, Option<String>),
            (u64, i64, std::collections::BTreeMap<String, rust_decimal::Decimal>),
        > = std::collections::BTreeMap::new();
        for record in &records {
            let bucket_start = truncate_to_bucket(record.created_at, params.bucket);
            let entry = series.entry(bucket_start).or_default();
            entry.0 += 1;
            entry.1 += record.total_tokens.unwrap_or(0);
            fold_cost(&mut entry.2, record.currency.clone(), record.estimated_cost);

            let key = (record.provider_id.clone(), record.upstream_model.clone());
            let entry = breakdown.entry(key).or_default();
            entry.0 += 1;
            entry.1 += record.total_tokens.unwrap_or(0);
            fold_cost(&mut entry.2, record.currency.clone(), record.estimated_cost);
        }

        let summary = UsageSummary {
            totals,
            series: series
                .into_iter()
                .map(|(bucket_start, (requests, total_tokens, cost))| UsageSeriesPoint {
                    bucket_start,
                    requests,
                    total_tokens,
                    cost: cost_vec(cost),
                })
                .collect(),
            breakdown: breakdown
                .into_iter()
                .map(|((provider_id, model), (requests, total_tokens, cost))| {
                    UsageBreakdownRow {
                        provider_id,
                        model,
                        requests,
                        total_tokens,
                        cost: cost_vec(cost),
                    }
                })
                .collect(),
        };
        Box::pin(async move { Ok(summary) })
    }
}

#[cfg(test)]
mod writer_tests {
    use std::sync::{Arc, Mutex};

    use chrono::Utc;

    use super::*;
    use crate::model::Protocol;
    use crate::usage::UsageSource;

    /// In-memory sink recording every flushed batch so tests can observe the
    /// writer's batching and shutdown behavior without a database.
    #[derive(Clone, Default)]
    struct MemorySink {
        batches: Arc<Mutex<Vec<Vec<UsageRecord>>>>,
    }

    impl MemorySink {
        fn batches(&self) -> Vec<Vec<UsageRecord>> {
            self.batches.lock().unwrap().clone()
        }

        fn total_records(&self) -> usize {
            self.batches.lock().unwrap().iter().map(Vec::len).sum()
        }
    }

    impl BatchSink for MemorySink {
        fn flush(&self, batch: Vec<UsageRecord>) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            let batches = self.batches.clone();
            Box::pin(async move {
                if batch.is_empty() {
                    return;
                }
                batches.lock().unwrap().push(batch);
            })
        }
    }

    fn record(request_id: &str) -> UsageRecord {
        UsageRecord {
            request_id: request_id.to_owned(),
            created_at: Utc::now(),
            provider_id: "p".to_owned(),
            protocol: Protocol::OpenAi,
            path: "/v1/chat/completions".to_owned(),
            requested_model: None,
            upstream_model: None,
            status: 200,
            latency_ms: 1,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: None,
            estimated_cost: None,
            currency: None,
            usage_source: UsageSource::ProviderResponse,
            pricing_source: None,
        }
    }

    /// Task 7.8 / 7.10: records enqueued before shutdown are flushed when the
    /// writer is told to drain, even when far below the batch size.
    #[tokio::test]
    async fn shutdown_flushes_pending_records() {
        let sink = MemorySink::default();
        let (sender, receiver) = mpsc::channel(USAGE_CHANNEL_CAPACITY);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(run_writer(sink.clone(), receiver, shutdown_rx));

        for i in 0..3 {
            sender.try_send(record(&format!("r{i}"))).unwrap();
        }

        // Signal drain-and-exit, then join the writer.
        shutdown_tx.send(()).unwrap();
        drop(sender);
        task.await.unwrap();

        assert_eq!(sink.total_records(), 3);
    }

    /// A full run of `USAGE_BATCH_SIZE` records flushes as a single batch by
    /// size before any interval tick.
    #[tokio::test]
    async fn full_batch_flushes_by_size() {
        let sink = MemorySink::default();
        let (sender, receiver) = mpsc::channel(USAGE_CHANNEL_CAPACITY);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(run_writer(sink.clone(), receiver, shutdown_rx));

        for i in 0..USAGE_BATCH_SIZE {
            sender.try_send(record(&format!("r{i}"))).unwrap();
        }

        shutdown_tx.send(()).unwrap();
        drop(sender);
        task.await.unwrap();

        let batches = sink.batches();
        assert_eq!(batches[0].len(), USAGE_BATCH_SIZE);
        assert_eq!(sink.total_records(), USAGE_BATCH_SIZE);
    }

    /// Task 7.9: once the bounded channel is full, further `try_send` calls
    /// fail fast (the recorder drops with a warning) rather than blocking.
    #[test]
    fn full_channel_rejects_without_blocking() {
        let (sender, _receiver) = mpsc::channel::<UsageRecord>(2);
        assert!(sender.try_send(record("a")).is_ok());
        assert!(sender.try_send(record("b")).is_ok());
        // Third send has no capacity and no consumer; must not block.
        match sender.try_send(record("c")) {
            Err(mpsc::error::TrySendError::Full(_)) => {}
            other => panic!("expected Full, got {other:?}"),
        }
    }
}
