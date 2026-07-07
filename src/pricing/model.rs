use std::{collections::HashMap, sync::Arc};

use rust_decimal::Decimal;

#[derive(Clone, Debug)]
pub(crate) struct Pricing {
    pub input_per_1m: Decimal,
    pub output_per_1m: Decimal,
    pub cached_input_per_1m: Option<Decimal>,
    pub cache_read_per_1m: Option<Decimal>,
    pub cache_write_per_1m: Option<Decimal>,
    pub currency: String,
    pub pricing_source: Option<String>,
}

pub(crate) type PricingMap = Arc<HashMap<(String, String), Pricing>>;
