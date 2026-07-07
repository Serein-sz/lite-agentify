use std::collections::HashMap;
use std::sync::Arc;

use anyhow::bail;
use rust_decimal::Decimal;

use super::model::{Pricing, PricingMap};
use crate::config::PricingConfig;

pub(crate) fn pricing_map(entries: Vec<PricingConfig>) -> anyhow::Result<PricingMap> {
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
