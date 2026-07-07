use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

pub(crate) type UsageQueryFuture<T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusFilter {
    Exact(u16),
    ClientError,
    ServerError,
}

impl StatusFilter {
    /// In-memory matching, used by the test recorder; the SQL recorder
    /// expresses the same predicate in its WHERE clause.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn matches(&self, status: u16) -> bool {
        match self {
            Self::Exact(expected) => status == *expected,
            Self::ClientError => (400..500).contains(&status),
            Self::ServerError => status >= 500,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UsageListParams {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub status: Option<StatusFilter>,
    /// 1-based page index.
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UsageRow {
    pub request_id: String,
    pub created_at: DateTime<Utc>,
    pub provider_id: String,
    pub protocol: String,
    pub path: String,
    pub requested_model: Option<String>,
    pub upstream_model: Option<String>,
    pub status: u16,
    pub latency_ms: i64,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub estimated_cost: Option<Decimal>,
    pub currency: Option<String>,
    pub usage_source: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub(crate) struct UsagePage {
    pub rows: Vec<UsageRow>,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SummaryBucket {
    Hour,
    Day,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageSummaryParams {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub bucket: SummaryBucket,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct CostByCurrency {
    pub currency: String,
    pub amount: Decimal,
}

#[derive(Debug, Clone, Serialize, Default)]
pub(crate) struct UsageTotals {
    pub requests: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub avg_latency_ms: f64,
    /// Share of requests with status >= 400, in [0, 1].
    pub error_rate: f64,
    pub cost: Vec<CostByCurrency>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UsageSeriesPoint {
    pub bucket_start: DateTime<Utc>,
    pub requests: u64,
    pub total_tokens: i64,
    pub cost: Vec<CostByCurrency>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UsageBreakdownRow {
    pub provider_id: String,
    pub model: Option<String>,
    pub requests: u64,
    pub total_tokens: i64,
    pub cost: Vec<CostByCurrency>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub(crate) struct UsageSummary {
    pub totals: UsageTotals,
    pub series: Vec<UsageSeriesPoint>,
    pub breakdown: Vec<UsageBreakdownRow>,
}

/// Read access to recorded usage, offered by recorders backed by a readable
/// store. Returned futures own their data and never borrow the recorder.
pub(crate) trait UsageQuery: Send + Sync {
    fn list(&self, params: UsageListParams) -> UsageQueryFuture<UsagePage>;
    fn summary(&self, params: UsageSummaryParams) -> UsageQueryFuture<UsageSummary>;
}
