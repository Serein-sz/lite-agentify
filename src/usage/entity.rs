pub(crate) mod usage_record {
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use sea_orm::{DeriveEntityModel, DeriveRelation, EnumIter, entity::prelude::*};
    use uuid::Uuid;

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
        pub user_id: Option<Uuid>,
        pub api_key_id: Option<Uuid>,
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
