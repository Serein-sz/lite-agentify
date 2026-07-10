pub(crate) mod user {
    use chrono::{DateTime, Utc};
    use sea_orm::{DeriveEntityModel, DeriveRelation, EnumIter, entity::prelude::*};
    use uuid::Uuid;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "users")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub username: String,
        pub password_hash: String,
        pub role: String,
        pub status: String,
        pub created_at: DateTime<Utc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod api_key {
    use chrono::{DateTime, Utc};
    use sea_orm::{DeriveEntityModel, DeriveRelation, EnumIter, entity::prelude::*};
    use uuid::Uuid;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "api_keys")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub user_id: Uuid,
        pub key_hash: String,
        pub prefix: String,
        pub name: String,
        pub status: String,
        pub created_at: DateTime<Utc>,
        pub last_used_at: Option<DateTime<Utc>>,
        /// JSONB array of model names this key may call. NULL = all models.
        #[sea_orm(column_type = "JsonBinary", nullable)]
        pub allowed_models: Option<serde_json::Value>,
        /// Cumulative USD spend cap for this key. NULL = uncapped.
        #[sea_orm(column_type = "Decimal(Some((20, 10)))", nullable)]
        pub spend_cap_usd: Option<rust_decimal::Decimal>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
