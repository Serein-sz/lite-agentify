use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::warn;
use uuid::Uuid;

/// A spend-counter scope: cumulative USD spent by a user or through a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Scope {
    User(Uuid),
    Key(Uuid),
}

impl Scope {
    fn redis_key(&self) -> String {
        match self {
            Self::User(id) => format!("spent:user:{id}"),
            Self::Key(id) => format!("spent:key:{id}"),
        }
    }
}

/// Fast cumulative spend counters backing the soft quota check. Counters are
/// advisory: Postgres is the truth, and the reconciliation loop periodically
/// `reset`s them to recomputed sums. Implementations must never fail a request
/// because the backend hiccuped — reads degrade to the last known value.
#[async_trait]
pub(crate) trait SpendCounter: Send + Sync {
    async fn get(&self, scope: Scope) -> Decimal;
    async fn add(&self, scope: Scope, cost: Decimal);
    async fn reset(&self, scope: Scope, value: Decimal);
}

/// Process-local counters; the default when no `[redis]` section is
/// configured. Crash loss is healed by the reconciliation loop.
#[derive(Default)]
pub(crate) struct MemoryCounter {
    values: Mutex<HashMap<Scope, Decimal>>,
}

#[async_trait]
impl SpendCounter for MemoryCounter {
    async fn get(&self, scope: Scope) -> Decimal {
        self.values
            .lock()
            .unwrap()
            .get(&scope)
            .copied()
            .unwrap_or_default()
    }

    async fn add(&self, scope: Scope, cost: Decimal) {
        *self.values.lock().unwrap().entry(scope).or_default() += cost;
    }

    async fn reset(&self, scope: Scope, value: Decimal) {
        self.values.lock().unwrap().insert(scope, value);
    }
}

/// Redis-backed counters (`INCRBYFLOAT`/`GET`/`SET`), surviving gateway
/// restarts and shareable across instances. Command failures degrade to an
/// in-memory shadow seeded from the last known values; the reconciliation
/// loop re-seeds Redis from Postgres truth once it is reachable again.
pub(crate) struct RedisCounter {
    connection: redis::aio::ConnectionManager,
    shadow: MemoryCounter,
    degraded: AtomicBool,
}

impl RedisCounter {
    pub(crate) fn new(connection: redis::aio::ConnectionManager) -> Self {
        Self {
            connection,
            shadow: MemoryCounter::default(),
            degraded: AtomicBool::new(false),
        }
    }

    /// Logs the first failure of an outage window, then stays quiet until a
    /// command succeeds again.
    fn note_outcome(&self, result: &Result<(), String>) {
        match result {
            Ok(()) => {
                if self.degraded.swap(false, Ordering::Relaxed) {
                    warn!("redis spend counters recovered; leaving degraded in-memory mode");
                }
            }
            Err(error) => {
                if !self.degraded.swap(true, Ordering::Relaxed) {
                    warn!(
                        %error,
                        "redis spend counters unavailable; degrading to in-memory shadow until it recovers"
                    );
                }
            }
        }
    }
}

#[async_trait]
impl SpendCounter for RedisCounter {
    async fn get(&self, scope: Scope) -> Decimal {
        let mut connection = self.connection.clone();
        let result: Result<Option<String>, _> = redis::cmd("GET")
            .arg(scope.redis_key())
            .query_async(&mut connection)
            .await;
        match result {
            Ok(value) => {
                self.note_outcome(&Ok(()));
                let parsed = value
                    .and_then(|text| text.parse::<Decimal>().ok())
                    .unwrap_or_default();
                // Keep the shadow warm so a later outage starts from here.
                self.shadow.reset(scope, parsed).await;
                parsed
            }
            Err(error) => {
                self.note_outcome(&Err(error.to_string()));
                self.shadow.get(scope).await
            }
        }
    }

    async fn add(&self, scope: Scope, cost: Decimal) {
        // The shadow always advances so degraded reads keep counting.
        self.shadow.add(scope, cost).await;
        let mut connection = self.connection.clone();
        let result: Result<String, _> = redis::cmd("INCRBYFLOAT")
            .arg(scope.redis_key())
            .arg(cost.to_string())
            .query_async(&mut connection)
            .await;
        self.note_outcome(&result.map(|_| ()).map_err(|error| error.to_string()));
    }

    async fn reset(&self, scope: Scope, value: Decimal) {
        self.shadow.reset(scope, value).await;
        let mut connection = self.connection.clone();
        let result: Result<(), _> = redis::cmd("SET")
            .arg(scope.redis_key())
            .arg(value.to_string())
            .query_async(&mut connection)
            .await;
        self.note_outcome(&result.map_err(|error| error.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_counter_round_trips() {
        let counter = MemoryCounter::default();
        let scope = Scope::User(Uuid::new_v4());
        assert_eq!(counter.get(scope).await, Decimal::ZERO);
        counter.add(scope, Decimal::new(150, 2)).await;
        counter.add(scope, Decimal::new(50, 2)).await;
        assert_eq!(counter.get(scope).await, Decimal::new(200, 2));
        counter.reset(scope, Decimal::ONE).await;
        assert_eq!(counter.get(scope).await, Decimal::ONE);
    }

    /// Gated Redis round-trip: runs only when LITE_AGENTIFY_TEST_REDIS_URL is
    /// set (e.g. redis://:password@host:6379/0); silently passes otherwise.
    #[tokio::test]
    async fn redis_counter_round_trips_when_configured() {
        let Ok(url) = std::env::var("LITE_AGENTIFY_TEST_REDIS_URL") else {
            return;
        };
        let client = redis::Client::open(url).expect("valid redis url");
        let connection = redis::aio::ConnectionManager::new(client)
            .await
            .expect("redis reachable");
        let counter = RedisCounter::new(connection);
        let scope = Scope::Key(Uuid::new_v4());
        counter.reset(scope, Decimal::ZERO).await;
        counter.add(scope, Decimal::new(125, 2)).await;
        assert_eq!(counter.get(scope).await, Decimal::new(125, 2));
        counter.reset(scope, Decimal::ZERO).await;
    }
}
