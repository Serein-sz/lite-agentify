use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Context;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, EntityTrait, Set,
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

pub(crate) trait UsageRecorder: Send + Sync {
    fn record(&self, record: UsageRecord) -> UsageRecordFuture;

    /// Read access to recorded usage; `None` when this recorder has no
    /// readable store (usage recording disabled).
    fn query(&self) -> Option<&dyn UsageQuery> {
        None
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
    fn query(&self) -> Option<&dyn UsageQuery> {
        Some(self)
    }

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
