use std::collections::HashMap;

use anyhow::Context;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use uuid::Uuid;

use super::entity::credit_grant;

/// One append-only ledger entry. Corrections are negative amounts.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct GrantRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub amount_usd: Decimal,
    pub note: Option<String>,
    pub granted_by: Uuid,
    pub created_at: DateTime<Utc>,
}

/// Per-scope cumulative spend recomputed from the usage table.
#[derive(Debug, Default, Clone)]
pub(crate) struct SpendSums {
    pub by_user: HashMap<Uuid, Decimal>,
    pub by_key: HashMap<Uuid, Decimal>,
}

/// Persistence for the credit ledger and the Postgres-side truth the counters
/// reconcile against. A trait so quota flows are testable without PostgreSQL.
#[async_trait]
pub(crate) trait QuotaStore: Send + Sync {
    /// Appends a grant (possibly negative) and returns the stored row.
    async fn append_grant(
        &self,
        user_id: Uuid,
        amount_usd: Decimal,
        note: Option<&str>,
        granted_by: Uuid,
    ) -> anyhow::Result<GrantRecord>;

    /// The most recent ledger entries, optionally scoped to one user.
    async fn list_grants(
        &self,
        user_id: Option<Uuid>,
        limit: u64,
    ) -> anyhow::Result<Vec<GrantRecord>>;

    /// Σ grants per user — the "granted" side of every balance.
    async fn grant_sums(&self) -> anyhow::Result<HashMap<Uuid, Decimal>>;

    /// Σ attributed usage cost per user and per key (NULL costs count as 0) —
    /// the "spent" side, recomputed from the usage table.
    async fn spend_sums(&self) -> anyhow::Result<SpendSums>;
}

fn grant_record(model: credit_grant::Model) -> GrantRecord {
    GrantRecord {
        id: model.id,
        user_id: model.user_id,
        amount_usd: model.amount_usd,
        note: model.note,
        granted_by: model.granted_by,
        created_at: model.created_at,
    }
}

#[derive(Clone)]
pub(crate) struct SeaOrmQuotaStore {
    db: DatabaseConnection,
}

impl SeaOrmQuotaStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl QuotaStore for SeaOrmQuotaStore {
    async fn append_grant(
        &self,
        user_id: Uuid,
        amount_usd: Decimal,
        note: Option<&str>,
        granted_by: Uuid,
    ) -> anyhow::Result<GrantRecord> {
        use sea_orm::ActiveModelTrait;
        let inserted = credit_grant::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(user_id),
            amount_usd: Set(amount_usd),
            note: Set(note.map(str::to_owned)),
            granted_by: Set(granted_by),
            created_at: Set(Utc::now()),
        }
        .insert(&self.db)
        .await
        .context("failed to append credit grant")?;
        Ok(grant_record(inserted))
    }

    async fn list_grants(
        &self,
        user_id: Option<Uuid>,
        limit: u64,
    ) -> anyhow::Result<Vec<GrantRecord>> {
        let mut query = credit_grant::Entity::find()
            .order_by_desc(credit_grant::Column::CreatedAt)
            .limit(limit);
        if let Some(user_id) = user_id {
            query = query.filter(credit_grant::Column::UserId.eq(user_id));
        }
        Ok(query
            .all(&self.db)
            .await
            .context("failed to list credit grants")?
            .into_iter()
            .map(grant_record)
            .collect())
    }

    async fn grant_sums(&self) -> anyhow::Result<HashMap<Uuid, Decimal>> {
        use sea_orm::{ConnectionTrait, Statement};
        let rows = self
            .db
            .query_all(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT user_id, COALESCE(SUM(amount_usd), 0) AS total \
                 FROM credit_grants GROUP BY user_id",
            ))
            .await
            .context("failed to sum credit grants")?;
        let mut sums = HashMap::new();
        for row in rows {
            let user_id: Uuid = row.try_get("", "user_id")?;
            let total: Decimal = row.try_get("", "total")?;
            sums.insert(user_id, total);
        }
        Ok(sums)
    }

    async fn spend_sums(&self) -> anyhow::Result<SpendSums> {
        use sea_orm::{ConnectionTrait, Statement};
        let mut sums = SpendSums::default();

        let rows = self
            .db
            .query_all(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT user_id, COALESCE(SUM(estimated_cost), 0) AS total \
                 FROM usage_records WHERE user_id IS NOT NULL GROUP BY user_id",
            ))
            .await
            .context("failed to sum usage cost by user")?;
        for row in rows {
            let user_id: Uuid = row.try_get("", "user_id")?;
            let total: Decimal = row.try_get("", "total")?;
            sums.by_user.insert(user_id, total);
        }

        let rows = self
            .db
            .query_all(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT api_key_id, COALESCE(SUM(estimated_cost), 0) AS total \
                 FROM usage_records WHERE api_key_id IS NOT NULL GROUP BY api_key_id",
            ))
            .await
            .context("failed to sum usage cost by key")?;
        for row in rows {
            let api_key_id: Uuid = row.try_get("", "api_key_id")?;
            let total: Decimal = row.try_get("", "total")?;
            sums.by_key.insert(api_key_id, total);
        }

        Ok(sums)
    }
}

/// In-memory quota store for tests: grants live in a Vec; spend sums are
/// whatever the test seeds (the proxy-path counters do the live counting).
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct MemoryQuotaStore {
    inner: std::sync::Arc<std::sync::Mutex<MemoryInner>>,
}

#[cfg(test)]
#[derive(Default)]
struct MemoryInner {
    grants: Vec<GrantRecord>,
    spend: SpendSums,
}

#[cfg(test)]
impl MemoryQuotaStore {
    pub(crate) fn set_spend_sums(&self, spend: SpendSums) {
        self.inner.lock().unwrap().spend = spend;
    }
}

#[cfg(test)]
#[async_trait]
impl QuotaStore for MemoryQuotaStore {
    async fn append_grant(
        &self,
        user_id: Uuid,
        amount_usd: Decimal,
        note: Option<&str>,
        granted_by: Uuid,
    ) -> anyhow::Result<GrantRecord> {
        let record = GrantRecord {
            id: Uuid::new_v4(),
            user_id,
            amount_usd,
            note: note.map(str::to_owned),
            granted_by,
            created_at: Utc::now(),
        };
        self.inner.lock().unwrap().grants.push(record.clone());
        Ok(record)
    }

    async fn list_grants(
        &self,
        user_id: Option<Uuid>,
        limit: u64,
    ) -> anyhow::Result<Vec<GrantRecord>> {
        let inner = self.inner.lock().unwrap();
        let mut grants: Vec<GrantRecord> = inner
            .grants
            .iter()
            .filter(|grant| user_id.is_none_or(|user_id| grant.user_id == user_id))
            .cloned()
            .collect();
        grants.reverse();
        grants.truncate(limit as usize);
        Ok(grants)
    }

    async fn grant_sums(&self) -> anyhow::Result<HashMap<Uuid, Decimal>> {
        let inner = self.inner.lock().unwrap();
        let mut sums: HashMap<Uuid, Decimal> = HashMap::new();
        for grant in &inner.grants {
            *sums.entry(grant.user_id).or_default() += grant.amount_usd;
        }
        Ok(sums)
    }

    async fn spend_sums(&self) -> anyhow::Result<SpendSums> {
        Ok(self.inner.lock().unwrap().spend.clone())
    }
}
