use rust_decimal::Decimal;

use super::model::{Pricing, PricingMap};
use crate::gateway::domain::TokenUsage;

const TOKENS_PER_MILLION: i64 = 1_000_000;
pub(super) const PRICING_WILDCARD: &str = "*";

pub(crate) fn calculate_cost(
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use rust_decimal::Decimal;

    use super::*;
    use crate::gateway::domain::TokenUsage;

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
