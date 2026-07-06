use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::gateway::domain::UsageSource;
use crate::gateway::model::Protocol;

#[derive(Clone, Debug)]
pub(crate) struct UsageRecord {
    pub request_id: String,
    pub created_at: DateTime<Utc>,
    pub provider_id: String,
    pub protocol: Protocol,
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
    pub usage_source: UsageSource,
    pub pricing_source: Option<String>,
}
