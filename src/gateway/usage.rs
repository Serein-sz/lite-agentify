use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use anyhow::{Context, bail};
use axum::body::Bytes;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DeriveEntityModel, DeriveRelation, EntityTrait,
    EnumIter, Set, entity::prelude::*,
};
use serde_json::Value;
use tracing::warn;
use uuid::Uuid;

use super::{
    config::{PricingConfig, UsageDatabaseConfig},
    model::Protocol,
};

const TOKENS_PER_MILLION: i64 = 1_000_000;
const PRICING_WILDCARD: &str = "*";

#[derive(Clone, Debug)]
pub(super) struct Pricing {
    pub input_per_1m: Decimal,
    pub output_per_1m: Decimal,
    pub cached_input_per_1m: Option<Decimal>,
    pub cache_read_per_1m: Option<Decimal>,
    pub cache_write_per_1m: Option<Decimal>,
    pub currency: String,
    pub pricing_source: Option<String>,
}

pub(super) type PricingMap = Arc<HashMap<(String, String), Pricing>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TokenUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
}

impl TokenUsage {
    fn has_tokens(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cached_input_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_write_tokens.is_some()
            || self.total_tokens.is_some()
    }

    fn merge_from(&mut self, other: &TokenUsage) {
        if other.input_tokens.is_some() {
            self.input_tokens = other.input_tokens;
        }
        if other.output_tokens.is_some() {
            self.output_tokens = other.output_tokens;
        }
        if other.cached_input_tokens.is_some() {
            self.cached_input_tokens = other.cached_input_tokens;
        }
        if other.cache_read_tokens.is_some() {
            self.cache_read_tokens = other.cache_read_tokens;
        }
        if other.cache_write_tokens.is_some() {
            self.cache_write_tokens = other.cache_write_tokens;
        }
        if other.total_tokens.is_some() {
            self.total_tokens = other.total_tokens;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UsageSource {
    ProviderResponse,
    StreamSummary,
    Unavailable,
}

impl std::fmt::Display for UsageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProviderResponse => f.write_str("provider_response"),
            Self::StreamSummary => f.write_str("stream_summary"),
            Self::Unavailable => f.write_str("unavailable"),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct UsageRecord {
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

pub(super) type UsageRecordFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

pub(super) trait UsageRecorder: Send + Sync {
    fn record(&self, record: UsageRecord) -> UsageRecordFuture;
}

#[derive(Clone, Default)]
pub(super) struct NoopUsageRecorder;

impl UsageRecorder for NoopUsageRecorder {
    fn record(&self, _record: UsageRecord) -> UsageRecordFuture {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
#[derive(Clone, Default)]
pub(super) struct MemoryUsageRecorder {
    records: Arc<std::sync::Mutex<Vec<UsageRecord>>>,
    fail_writes: bool,
}

#[cfg(test)]
impl MemoryUsageRecorder {
    pub(super) fn failing() -> Self {
        Self {
            fail_writes: true,
            ..Self::default()
        }
    }

    pub(super) fn records(&self) -> Vec<UsageRecord> {
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
                bail!("simulated usage write failure");
            }
            records.lock().unwrap().push(record);
            Ok(())
        })
    }
}

pub(super) struct SeaOrmUsageRecorder {
    db: DatabaseConnection,
}

impl SeaOrmUsageRecorder {
    pub(super) async fn connect(config: &UsageDatabaseConfig) -> anyhow::Result<Self> {
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

pub(super) async fn recorder_from_config(
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

pub(super) fn pricing_map(entries: Vec<PricingConfig>) -> anyhow::Result<PricingMap> {
    let mut pricing = HashMap::new();
    for entry in entries {
        if entry.provider.trim().is_empty() {
            bail!("pricing provider cannot be empty");
        }
        if entry.model.trim().is_empty() {
            bail!("pricing model cannot be empty");
        }
        if entry.currency.trim().is_empty()
            || entry.currency.len() != 3
            || !entry.currency.chars().all(|ch| ch.is_ascii_uppercase())
        {
            bail!(
                "pricing entry '{}:{}' currency must be a three-letter uppercase ISO code",
                entry.provider,
                entry.model
            );
        }

        validate_non_negative("input_per_1m", entry.input_per_1m)?;
        validate_non_negative("output_per_1m", entry.output_per_1m)?;
        validate_optional_non_negative("cached_input_per_1m", entry.cached_input_per_1m)?;
        validate_optional_non_negative("cache_read_per_1m", entry.cache_read_per_1m)?;
        validate_optional_non_negative("cache_write_per_1m", entry.cache_write_per_1m)?;

        let key = (entry.provider, entry.model);
        if pricing.contains_key(&key) {
            bail!("duplicate pricing entry '{}:{}'", key.0, key.1);
        }

        pricing.insert(
            key,
            Pricing {
                input_per_1m: entry.input_per_1m,
                output_per_1m: entry.output_per_1m,
                cached_input_per_1m: entry.cached_input_per_1m,
                cache_read_per_1m: entry.cache_read_per_1m,
                cache_write_per_1m: entry.cache_write_per_1m,
                currency: entry.currency,
                pricing_source: entry.pricing_source,
            },
        );
    }

    Ok(Arc::new(pricing))
}

fn validate_non_negative(name: &str, value: Decimal) -> anyhow::Result<()> {
    if value.is_sign_negative() {
        bail!("{name} cannot be negative");
    }
    Ok(())
}

fn validate_optional_non_negative(name: &str, value: Option<Decimal>) -> anyhow::Result<()> {
    if value.is_some_and(|value| value.is_sign_negative()) {
        bail!("{name} cannot be negative");
    }
    Ok(())
}

pub(super) fn calculate_cost(
    pricing: &PricingMap,
    provider_id: &str,
    upstream_model: Option<&str>,
    usage: &TokenUsage,
) -> Option<(Decimal, String, Option<String>)> {
    let upstream_model = upstream_model?;
    if !usage.has_tokens() {
        return None;
    }
    let price = lookup_pricing(pricing, provider_id, upstream_model)?;

    let cached_input = usage.cached_input_tokens.unwrap_or(0);
    let cache_read = usage.cache_read_tokens.unwrap_or(0);
    let cache_write = usage.cache_write_tokens.unwrap_or(0);

    if cached_input > 0 && price.cached_input_per_1m.is_none() {
        return None;
    }
    if cache_read > 0 && price.cache_read_per_1m.is_none() {
        return None;
    }
    if cache_write > 0 && price.cache_write_per_1m.is_none() {
        return None;
    }

    let input_tokens = usage.input_tokens.unwrap_or(0);
    // Only cached_input (OpenAI cached_tokens) is a subset of input_tokens; cache_read and
    // cache_write are independent additive classes (Anthropic) and must not be subtracted.
    let regular_input = input_tokens.saturating_sub(cached_input).max(0);

    let mut cost = token_cost(regular_input, price.input_per_1m);
    cost += token_cost(usage.output_tokens.unwrap_or(0), price.output_per_1m);
    if let Some(cached_price) = price.cached_input_per_1m {
        cost += token_cost(cached_input, cached_price);
    }
    if let Some(cache_read_price) = price.cache_read_per_1m {
        cost += token_cost(cache_read, cache_read_price);
    }
    if let Some(cache_write_price) = price.cache_write_per_1m {
        cost += token_cost(cache_write, cache_write_price);
    }

    Some((cost, price.currency.clone(), price.pricing_source.clone()))
}

fn lookup_pricing<'a>(
    pricing: &'a PricingMap,
    provider_id: &str,
    upstream_model: &str,
) -> Option<&'a Pricing> {
    [
        (provider_id, upstream_model),
        (provider_id, PRICING_WILDCARD),
        (PRICING_WILDCARD, upstream_model),
        (PRICING_WILDCARD, PRICING_WILDCARD),
    ]
    .into_iter()
    .find_map(|(provider, model)| pricing.get(&(provider.to_owned(), model.to_owned())))
}

fn token_cost(tokens: i64, price_per_1m: Decimal) -> Decimal {
    Decimal::from(tokens) * price_per_1m / Decimal::from(TOKENS_PER_MILLION)
}

pub(super) fn parse_non_streaming_usage(protocol: Protocol, body: &[u8]) -> Option<TokenUsage> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    match protocol {
        Protocol::OpenAi => parse_openai_usage(value.get("usage")?),
        Protocol::Anthropic => parse_anthropic_usage(value.get("usage")?),
    }
}

fn parse_openai_usage(usage: &Value) -> Option<TokenUsage> {
    let input_tokens = number(usage, "prompt_tokens");
    let output_tokens = number(usage, "completion_tokens");
    let total_tokens = number(usage, "total_tokens");
    let cached_input_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|details| number(details, "cached_tokens"));

    let parsed = TokenUsage {
        input_tokens,
        output_tokens,
        cached_input_tokens,
        total_tokens,
        ..TokenUsage::default()
    };
    parsed.has_tokens().then_some(parsed)
}

fn parse_anthropic_usage(usage: &Value) -> Option<TokenUsage> {
    let parsed = TokenUsage {
        input_tokens: number(usage, "input_tokens"),
        output_tokens: number(usage, "output_tokens"),
        cache_write_tokens: number(usage, "cache_creation_input_tokens"),
        cache_read_tokens: number(usage, "cache_read_input_tokens"),
        ..TokenUsage::default()
    };
    parsed.has_tokens().then_some(parsed)
}

fn number(value: &Value, key: &str) -> Option<i64> {
    value.get(key)?.as_i64().filter(|value| *value >= 0)
}

pub(super) struct UsageObserver {
    protocol: Protocol,
    line_buffer: Vec<u8>,
    usage: TokenUsage,
    seen_usage: bool,
}

impl UsageObserver {
    pub(super) fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            line_buffer: Vec::new(),
            usage: TokenUsage::default(),
            seen_usage: false,
        }
    }

    pub(super) fn feed(&mut self, chunk: &Bytes) {
        self.line_buffer.extend_from_slice(chunk);
        while let Some(newline) = self.line_buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.line_buffer.drain(..=newline).collect::<Vec<u8>>();
            self.consume_line(&line);
        }
    }

    pub(super) fn finish(&mut self) -> Option<TokenUsage> {
        if !self.line_buffer.is_empty() {
            let line = std::mem::take(&mut self.line_buffer);
            self.consume_line(&line);
        }
        self.seen_usage.then(|| self.usage.clone())
    }

    fn consume_line(&mut self, line: &[u8]) {
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        let Some(data) = line.trim_end().strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let parsed = match self.protocol {
            Protocol::OpenAi => value.get("usage").and_then(parse_openai_usage),
            Protocol::Anthropic => value
                .pointer("/message/usage")
                .and_then(parse_anthropic_usage)
                .or_else(|| value.get("usage").and_then(parse_anthropic_usage)),
        };
        if let Some(parsed) = parsed {
            self.usage.merge_from(&parsed);
            self.seen_usage = true;
        }
    }
}

pub(super) fn warn_record_error(error: anyhow::Error) {
    warn!(error = %error, error_chain = ?error, "failed to record usage");
}

pub mod usage_record {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "usage_records")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub request_id: String,
        pub created_at: DateTime<Utc>,
        pub provider_id: String,
        pub protocol: String,
        pub path: String,
        pub requested_model: Option<String>,
        pub upstream_model: Option<String>,
        pub status: i32,
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
        pub pricing_source: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;

    fn pricing(provider: &str, model: &str) -> PricingMap {
        Arc::new(HashMap::from([(
            (provider.to_owned(), model.to_owned()),
            Pricing {
                input_per_1m: Decimal::new(300, 2),
                output_per_1m: Decimal::new(1500, 2),
                cached_input_per_1m: Some(Decimal::new(30, 2)),
                cache_read_per_1m: Some(Decimal::new(30, 2)),
                cache_write_per_1m: Some(Decimal::new(375, 2)),
                currency: "USD".to_owned(),
                pricing_source: Some("test".to_owned()),
            },
        )]))
    }

    #[test]
    fn parses_openai_cached_usage() {
        let usage = parse_non_streaming_usage(
            Protocol::OpenAi,
            br#"{"usage":{"prompt_tokens":1000,"completion_tokens":200,"total_tokens":1200,"prompt_tokens_details":{"cached_tokens":400}}}"#,
        )
        .unwrap();

        assert_eq!(usage.input_tokens, Some(1000));
        assert_eq!(usage.output_tokens, Some(200));
        assert_eq!(usage.cached_input_tokens, Some(400));
        assert_eq!(usage.total_tokens, Some(1200));
    }

    #[test]
    fn parses_anthropic_cache_usage() {
        let usage = parse_non_streaming_usage(
            Protocol::Anthropic,
            br#"{"usage":{"input_tokens":1000,"output_tokens":200,"cache_creation_input_tokens":100,"cache_read_input_tokens":300}}"#,
        )
        .unwrap();

        assert_eq!(usage.input_tokens, Some(1000));
        assert_eq!(usage.output_tokens, Some(200));
        assert_eq!(usage.cache_write_tokens, Some(100));
        assert_eq!(usage.cache_read_tokens, Some(300));
    }

    #[test]
    fn calculates_cache_aware_cost() {
        let usage = TokenUsage {
            input_tokens: Some(1000),
            output_tokens: Some(200),
            cache_read_tokens: Some(300),
            cache_write_tokens: Some(100),
            ..TokenUsage::default()
        };

        let (cost, currency, source) = calculate_cost(
            &pricing("anthropic", "sonnet"),
            "anthropic",
            Some("sonnet"),
            &usage,
        )
        .unwrap();

        assert_eq!(currency, "USD");
        assert_eq!(source, Some("test".to_owned()));
        assert_eq!(cost, Decimal::new(6465, 6));
    }

    #[test]
    fn anthropic_cache_read_exceeding_input_stays_non_negative() {
        let usage = TokenUsage {
            input_tokens: Some(27586),
            output_tokens: Some(387),
            cache_read_tokens: Some(106262),
            ..TokenUsage::default()
        };

        let (cost, _, _) = calculate_cost(
            &pricing("anthropic", "sonnet"),
            "anthropic",
            Some("sonnet"),
            &usage,
        )
        .unwrap();

        assert!(cost >= Decimal::ZERO);
    }

    #[test]
    fn openai_cached_tokens_are_subtracted_from_regular_input() {
        let usage = TokenUsage {
            input_tokens: Some(1000),
            output_tokens: Some(0),
            cached_input_tokens: Some(400),
            ..TokenUsage::default()
        };

        let (cost, _, _) =
            calculate_cost(&pricing("openai", "gpt"), "openai", Some("gpt"), &usage).unwrap();

        // regular input 600 * 3.00 + cached 400 * 0.30, all per 1M.
        assert_eq!(cost, Decimal::new(192, 5));
    }

    #[test]
    fn missing_cache_pricing_leaves_cost_unavailable() {
        let pricing = Arc::new(HashMap::from([(
            ("openai".to_owned(), "gpt".to_owned()),
            Pricing {
                input_per_1m: Decimal::ONE,
                output_per_1m: Decimal::ONE,
                cached_input_per_1m: None,
                cache_read_per_1m: None,
                cache_write_per_1m: None,
                currency: "USD".to_owned(),
                pricing_source: None,
            },
        )]));
        let usage = TokenUsage {
            input_tokens: Some(100),
            cached_input_tokens: Some(50),
            ..TokenUsage::default()
        };

        assert!(calculate_cost(&pricing, "openai", Some("gpt"), &usage).is_none());
    }

    #[test]
    fn observer_merges_usage_fields_across_events() {
        let mut observer = UsageObserver::new(Protocol::Anthropic);
        observer.feed(&Bytes::from_static(
            b"data: {\"message\":{\"usage\":{\"input_tokens\":25}}}\n\n",
        ));
        observer.feed(&Bytes::from_static(
            b"data: {\"usage\":{\"output_tokens\":270}}\n\n",
        ));

        let usage = observer.finish().unwrap();
        assert_eq!(usage.input_tokens, Some(25));
        assert_eq!(usage.output_tokens, Some(270));
    }

    #[test]
    fn observer_reassembles_usage_line_split_across_chunks() {
        let mut observer = UsageObserver::new(Protocol::OpenAi);
        observer.feed(&Bytes::from_static(b"data: {\"usage\":{\"prompt_to"));
        observer.feed(&Bytes::from_static(
            b"kens\":100,\"completion_tokens\":25}}\n\n",
        ));

        let usage = observer.finish().unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(25));
    }

    #[test]
    fn observer_parses_final_line_without_trailing_newline() {
        let mut observer = UsageObserver::new(Protocol::OpenAi);
        observer.feed(&Bytes::from_static(
            b"data: {\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}",
        ));

        let usage = observer.finish().unwrap();
        assert_eq!(usage.input_tokens, Some(7));
        assert_eq!(usage.output_tokens, Some(3));
    }

    #[test]
    fn observer_without_usage_returns_none() {
        let mut observer = UsageObserver::new(Protocol::OpenAi);
        observer.feed(&Bytes::from_static(
            b"data: {\"choices\":[]}\n\ndata: [DONE]\n\n",
        ));

        assert!(observer.finish().is_none());
    }

    #[test]
    fn pricing_lookup_falls_back_by_specificity() {
        let pricing = Arc::new(HashMap::from([
            (
                ("provider-a".to_owned(), "*".to_owned()),
                Pricing {
                    input_per_1m: Decimal::ONE,
                    output_per_1m: Decimal::ONE,
                    cached_input_per_1m: None,
                    cache_read_per_1m: None,
                    cache_write_per_1m: None,
                    currency: "USD".to_owned(),
                    pricing_source: Some("provider-default".to_owned()),
                },
            ),
            (
                ("*".to_owned(), "model-a".to_owned()),
                Pricing {
                    input_per_1m: Decimal::ONE,
                    output_per_1m: Decimal::ONE,
                    cached_input_per_1m: None,
                    cache_read_per_1m: None,
                    cache_write_per_1m: None,
                    currency: "USD".to_owned(),
                    pricing_source: Some("model-default".to_owned()),
                },
            ),
            (
                ("*".to_owned(), "*".to_owned()),
                Pricing {
                    input_per_1m: Decimal::ONE,
                    output_per_1m: Decimal::ONE,
                    cached_input_per_1m: None,
                    cache_read_per_1m: None,
                    cache_write_per_1m: None,
                    currency: "USD".to_owned(),
                    pricing_source: Some("global-default".to_owned()),
                },
            ),
        ]));
        let usage = TokenUsage {
            input_tokens: Some(100),
            ..TokenUsage::default()
        };

        let (_, _, source) =
            calculate_cost(&pricing, "provider-a", Some("model-a"), &usage).unwrap();
        assert_eq!(source.as_deref(), Some("provider-default"));

        let (_, _, source) =
            calculate_cost(&pricing, "provider-b", Some("model-a"), &usage).unwrap();
        assert_eq!(source.as_deref(), Some("model-default"));

        let (_, _, source) =
            calculate_cost(&pricing, "provider-b", Some("model-b"), &usage).unwrap();
        assert_eq!(source.as_deref(), Some("global-default"));
    }
}
