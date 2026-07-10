mod counter;
mod entity;
mod store;

use std::sync::Arc;

use tracing::warn;

pub(crate) use counter::{MemoryCounter, RedisCounter, Scope, SpendCounter};
pub(crate) use store::{QuotaStore, SeaOrmQuotaStore};

#[cfg(test)]
pub(crate) use store::{MemoryQuotaStore, SpendSums};

/// How often counters are reset to Postgres-recomputed truth. This bounds both
/// Redis drift and memory-mode crash loss.
pub(crate) const RECONCILE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Recomputes spend sums from Postgres and resets every known counter scope to
/// truth. Called once at boot (seeding) and periodically thereafter.
pub(crate) async fn reconcile_counters(
    store: &dyn QuotaStore,
    counter: &dyn SpendCounter,
) -> anyhow::Result<()> {
    let sums = store.spend_sums().await?;
    for (user_id, total) in &sums.by_user {
        counter.reset(Scope::User(*user_id), *total).await;
    }
    for (key_id, total) in &sums.by_key {
        counter.reset(Scope::Key(*key_id), *total).await;
    }
    Ok(())
}

/// Spawns the periodic reconciliation loop.
pub(crate) fn spawn_reconciliation(
    store: Arc<dyn QuotaStore>,
    counter: Arc<dyn SpendCounter>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; boot already seeded, skip it.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = reconcile_counters(store.as_ref(), counter.as_ref()).await {
                warn!(
                    error = format!("{error:#}"),
                    "spend counter reconciliation failed; counters keep serving last values"
                );
            }
        }
    });
}
