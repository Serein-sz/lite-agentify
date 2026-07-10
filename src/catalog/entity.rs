pub(crate) mod provider {
    use chrono::{DateTime, Utc};
    use sea_orm::{DeriveEntityModel, DeriveRelation, EnumIter, entity::prelude::*};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "providers")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub protocol: String,
        pub base_url: String,
        pub api_key: String,
        pub anthropic_version: Option<String>,
        /// `{ alias: upstream_model }` as JSONB.
        pub model_aliases: Json,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod model {
    use chrono::{DateTime, Utc};
    use sea_orm::{DeriveEntityModel, DeriveRelation, EnumIter, entity::prelude::*};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "models")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub name: String,
        pub status: String,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod model_deployment {
    use sea_orm::{DeriveEntityModel, DeriveRelation, EnumIter, entity::prelude::*};
    use uuid::Uuid;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "model_deployments")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub model_name: String,
        pub provider_id: String,
        pub upstream_model: String,
        pub priority: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod pricing {
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use sea_orm::{DeriveEntityModel, DeriveRelation, EnumIter, entity::prelude::*};
    use uuid::Uuid;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "pricing")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub provider: String,
        pub model: String,
        pub input_per_1m: Decimal,
        pub output_per_1m: Decimal,
        pub cached_input_per_1m: Option<Decimal>,
        pub cache_read_per_1m: Option<Decimal>,
        pub cache_write_per_1m: Option<Decimal>,
        pub currency: String,
        pub pricing_source: Option<String>,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
