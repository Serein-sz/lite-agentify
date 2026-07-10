pub(crate) mod credit_grant {
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use sea_orm::{DeriveEntityModel, DeriveRelation, EnumIter, entity::prelude::*};
    use uuid::Uuid;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "credit_grants")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub user_id: Uuid,
        pub amount_usd: Decimal,
        pub note: Option<String>,
        pub granted_by: Uuid,
        pub created_at: DateTime<Utc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
