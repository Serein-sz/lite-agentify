use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::Context;
use async_trait::async_trait;
use tracing::warn;
use uuid::Uuid;

use super::SessionIdentity;

pub(crate) const LOCKOUT_THRESHOLD: u32 = 5;
pub(crate) const LOCKOUT_WINDOW: Duration = Duration::from_secs(60);
/// Upper bound on tracked lockout entries so an attacker cycling usernames
/// cannot grow the in-memory map unbounded (Redis bounds itself via TTLs).
const LOCKOUT_MAX_ENTRIES: usize = 10_000;

/// Admin sessions and login lockout behind one abstraction: process memory by
/// default, Redis when `[redis]` is configured — sessions then survive gateway
/// restarts. Auth is fail-closed: a backend error on a session read means the
/// caller is treated as signed out, never signed in.
#[async_trait]
pub(crate) trait SessionStore: Send + Sync {
    /// Persists a session under `token`. An error means the session was not
    /// stored and the login must fail.
    async fn open(
        &self,
        token: &str,
        identity: &SessionIdentity,
        ttl: Duration,
    ) -> anyhow::Result<()>;

    /// Resolves a token. `Ok(None)` = unknown or expired; `Err` = backend
    /// outage, which callers must treat as unauthenticated (fail closed).
    async fn get(&self, token: &str) -> anyhow::Result<Option<SessionIdentity>>;

    /// Removes one session (logout). Best effort: with Redis down the token
    /// is unreadable anyway and expires at its TTL.
    async fn remove(&self, token: &str);

    /// Removes every session of a user (disable / password reset). An error
    /// means sessions may outlive the mutation once the backend recovers —
    /// callers should log it loudly.
    async fn remove_user(&self, user_id: Uuid) -> anyhow::Result<()>;

    /// True when the username is currently locked out from logging in.
    async fn is_locked(&self, username: &str) -> bool;

    /// Records a failed login; at `LOCKOUT_THRESHOLD` consecutive failures
    /// the username locks for the lockout window.
    async fn register_failure(&self, username: &str);

    /// Clears failure state after a successful login.
    async fn clear_failures(&self, username: &str);
}

// --- in-memory implementation (the default) ---

struct MemorySession {
    identity: SessionIdentity,
    expires_at: Instant,
}

pub(crate) struct MemorySessionStore {
    sessions: Mutex<HashMap<String, MemorySession>>,
    limiter: Mutex<LoginLimiter>,
}

impl MemorySessionStore {
    pub(crate) fn new(lockout: Duration) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            limiter: Mutex::new(LoginLimiter::new(lockout)),
        }
    }
}

impl Default for MemorySessionStore {
    fn default() -> Self {
        Self::new(LOCKOUT_WINDOW)
    }
}

#[async_trait]
impl SessionStore for MemorySessionStore {
    async fn open(
        &self,
        token: &str,
        identity: &SessionIdentity,
        ttl: Duration,
    ) -> anyhow::Result<()> {
        self.sessions.lock().unwrap().insert(
            token.to_owned(),
            MemorySession {
                identity: identity.clone(),
                expires_at: Instant::now() + ttl,
            },
        );
        Ok(())
    }

    /// Validates a token, lazily removing it once expired.
    async fn get(&self, token: &str) -> anyhow::Result<Option<SessionIdentity>> {
        let mut sessions = self.sessions.lock().unwrap();
        Ok(match sessions.get(token) {
            Some(session) if session.expires_at > Instant::now() => {
                Some(session.identity.clone())
            }
            Some(_) => {
                sessions.remove(token);
                None
            }
            None => None,
        })
    }

    async fn remove(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }

    async fn remove_user(&self, user_id: Uuid) -> anyhow::Result<()> {
        self.sessions
            .lock()
            .unwrap()
            .retain(|_, session| session.identity.user_id != user_id);
        Ok(())
    }

    async fn is_locked(&self, username: &str) -> bool {
        self.limiter.lock().unwrap().is_locked(username)
    }

    async fn register_failure(&self, username: &str) {
        self.limiter.lock().unwrap().register_failure(username);
    }

    async fn clear_failures(&self, username: &str) {
        self.limiter.lock().unwrap().reset(username);
    }
}

/// Per-username lockout: after `LOCKOUT_THRESHOLD` consecutive failures for a
/// username, logins for that username are rejected for the lockout window.
/// The response is identical whether or not the username exists, so lockout
/// state does not enable user enumeration.
struct LoginLimiter {
    entries: HashMap<String, LimiterEntry>,
    lockout: Duration,
}

#[derive(Default)]
struct LimiterEntry {
    consecutive_failures: u32,
    locked_until: Option<Instant>,
}

impl LoginLimiter {
    fn new(lockout: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            lockout,
        }
    }

    fn is_locked(&mut self, username: &str) -> bool {
        let Some(entry) = self.entries.get_mut(username) else {
            return false;
        };
        match entry.locked_until {
            Some(until) if until > Instant::now() => true,
            Some(_) => {
                entry.locked_until = None;
                false
            }
            None => false,
        }
    }

    fn register_failure(&mut self, username: &str) {
        if self.entries.len() >= LOCKOUT_MAX_ENTRIES && !self.entries.contains_key(username) {
            // Drop expired entries before refusing to grow further.
            let now = Instant::now();
            self.entries.retain(|_, entry| {
                entry.locked_until.is_some_and(|until| until > now)
                    || entry.consecutive_failures > 0
            });
            if self.entries.len() >= LOCKOUT_MAX_ENTRIES {
                return;
            }
        }
        let lockout = self.lockout;
        let entry = self.entries.entry(username.to_owned()).or_default();
        entry.consecutive_failures += 1;
        if entry.consecutive_failures >= LOCKOUT_THRESHOLD {
            entry.locked_until = Some(Instant::now() + lockout);
            entry.consecutive_failures = 0;
        }
    }

    fn reset(&mut self, username: &str) {
        self.entries.remove(username);
    }
}

// --- Redis implementation (selected by the [redis] config section) ---

fn session_key(token: &str) -> String {
    format!("session:{token}")
}

/// Per-user token index so disabling a user can find all their sessions. Its
/// TTL is refreshed to the full session TTL on every open, so it outlives
/// every member token.
fn user_sessions_key(user_id: Uuid) -> String {
    format!("session_user:{user_id}")
}

fn lockout_key(username: &str) -> String {
    format!("lockout:{username}")
}

fn lockout_fails_key(username: &str) -> String {
    format!("lockout_fails:{username}")
}

/// Redis-backed sessions with native TTLs: they survive gateway restarts and
/// are shareable across instances. Unlike the spend counters there is no
/// in-memory shadow — a Redis outage makes session reads fail, which callers
/// turn into 401s (fail closed for auth, never open).
///
/// Lockout failures live in `lockout_fails:{username}` with the window as a
/// rolling TTL (the in-memory limiter keeps them until success; a TTL bounds
/// the Redis keyspace instead of an entry cap).
pub(crate) struct RedisSessionStore {
    connection: redis::aio::ConnectionManager,
    lockout: Duration,
}

impl RedisSessionStore {
    pub(crate) fn new(connection: redis::aio::ConnectionManager) -> Self {
        Self {
            connection,
            lockout: LOCKOUT_WINDOW,
        }
    }
}

#[async_trait]
impl SessionStore for RedisSessionStore {
    async fn open(
        &self,
        token: &str,
        identity: &SessionIdentity,
        ttl: Duration,
    ) -> anyhow::Result<()> {
        let payload = serde_json::to_string(identity).context("serialize session")?;
        let mut connection = self.connection.clone();
        redis::pipe()
            .cmd("SET")
            .arg(session_key(token))
            .arg(payload)
            .arg("EX")
            .arg(ttl.as_secs())
            .ignore()
            .cmd("SADD")
            .arg(user_sessions_key(identity.user_id))
            .arg(token)
            .ignore()
            .cmd("EXPIRE")
            .arg(user_sessions_key(identity.user_id))
            .arg(ttl.as_secs())
            .ignore()
            .query_async::<()>(&mut connection)
            .await
            .context("redis session write failed")
    }

    async fn get(&self, token: &str) -> anyhow::Result<Option<SessionIdentity>> {
        let mut connection = self.connection.clone();
        let value: Option<String> = redis::cmd("GET")
            .arg(session_key(token))
            .query_async(&mut connection)
            .await
            .context("redis session read failed")?;
        Ok(value.and_then(|json| match serde_json::from_str(&json) {
            Ok(identity) => Some(identity),
            Err(error) => {
                warn!(%error, "stored session payload is corrupt; treating as signed out");
                None
            }
        }))
    }

    async fn remove(&self, token: &str) {
        // Fetch the identity first so the per-user index stays clean; if the
        // read fails we still try the delete.
        let identity = self.get(token).await.ok().flatten();
        let mut connection = self.connection.clone();
        let mut pipe = redis::pipe();
        pipe.cmd("DEL").arg(session_key(token)).ignore();
        if let Some(identity) = &identity {
            pipe.cmd("SREM")
                .arg(user_sessions_key(identity.user_id))
                .arg(token)
                .ignore();
        }
        if let Err(error) = pipe.query_async::<()>(&mut connection).await {
            warn!(%error, "redis session delete failed; the token expires at its TTL");
        }
    }

    async fn remove_user(&self, user_id: Uuid) -> anyhow::Result<()> {
        let mut connection = self.connection.clone();
        let tokens: Vec<String> = redis::cmd("SMEMBERS")
            .arg(user_sessions_key(user_id))
            .query_async(&mut connection)
            .await
            .context("redis session index read failed")?;
        let mut pipe = redis::pipe();
        for token in &tokens {
            pipe.cmd("DEL").arg(session_key(token)).ignore();
        }
        pipe.cmd("DEL").arg(user_sessions_key(user_id)).ignore();
        pipe.query_async::<()>(&mut connection)
            .await
            .context("redis session delete failed")
    }

    async fn is_locked(&self, username: &str) -> bool {
        let mut connection = self.connection.clone();
        let result: Result<bool, _> = redis::cmd("EXISTS")
            .arg(lockout_key(username))
            .query_async(&mut connection)
            .await;
        match result {
            Ok(locked) => locked,
            Err(error) => {
                // Not locked on outage: the login cannot complete anyway
                // because the session write below it fails closed.
                warn!(%error, "redis lockout check failed; treating as not locked");
                false
            }
        }
    }

    async fn register_failure(&self, username: &str) {
        let window = self.lockout.as_secs().max(1);
        let fails = lockout_fails_key(username);
        let mut connection = self.connection.clone();
        let result: Result<(i64,), _> = redis::pipe()
            .atomic()
            .cmd("INCR")
            .arg(&fails)
            .cmd("EXPIRE")
            .arg(&fails)
            .arg(window)
            .ignore()
            .query_async(&mut connection)
            .await;
        match result {
            Ok((count,)) if count >= i64::from(LOCKOUT_THRESHOLD) => {
                let outcome = redis::pipe()
                    .atomic()
                    .cmd("SET")
                    .arg(lockout_key(username))
                    .arg(1)
                    .arg("EX")
                    .arg(window)
                    .ignore()
                    .cmd("DEL")
                    .arg(&fails)
                    .ignore()
                    .query_async::<()>(&mut connection)
                    .await;
                if let Err(error) = outcome {
                    warn!(%error, "redis lockout write failed");
                }
            }
            Ok(_) => {}
            Err(error) => warn!(%error, "redis lockout counter update failed"),
        }
    }

    async fn clear_failures(&self, username: &str) {
        let mut connection = self.connection.clone();
        let result: Result<(), _> = redis::pipe()
            .cmd("DEL")
            .arg(lockout_fails_key(username))
            .ignore()
            .cmd("DEL")
            .arg(lockout_key(username))
            .ignore()
            .query_async(&mut connection)
            .await;
        if let Err(error) = result {
            warn!(%error, "redis lockout clear failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Role;

    fn identity() -> SessionIdentity {
        SessionIdentity {
            user_id: Uuid::new_v4(),
            username: "session-test".to_owned(),
            role: Role::Admin,
        }
    }

    #[tokio::test]
    async fn memory_sessions_round_trip_and_drop_by_user() {
        let store = MemorySessionStore::default();
        let identity = identity();
        store
            .open("token-1", &identity, Duration::from_secs(60))
            .await
            .unwrap();
        let loaded = store.get("token-1").await.unwrap().expect("session exists");
        assert_eq!(loaded.user_id, identity.user_id);

        store.remove_user(identity.user_id).await.unwrap();
        assert!(store.get("token-1").await.unwrap().is_none());
    }

    async fn redis_store_from_env() -> Option<(redis::Client, RedisSessionStore)> {
        let url = std::env::var("LITE_AGENTIFY_TEST_REDIS_URL").ok()?;
        let client = redis::Client::open(url).expect("valid redis url");
        let connection = redis::aio::ConnectionManager::new(client.clone())
            .await
            .expect("redis reachable");
        Some((client, RedisSessionStore::new(connection)))
    }

    /// Gated: runs only when LITE_AGENTIFY_TEST_REDIS_URL is set. A second
    /// store over a fresh connection simulates a gateway restart — the
    /// session must survive it, and a user-wide drop must reach it.
    #[tokio::test]
    async fn redis_sessions_survive_simulated_restart_when_configured() {
        let Some((client, first)) = redis_store_from_env().await else {
            return;
        };
        let identity = identity();
        let token = format!("test-{}", Uuid::new_v4());
        first
            .open(&token, &identity, Duration::from_secs(60))
            .await
            .unwrap();

        let second = RedisSessionStore::new(
            redis::aio::ConnectionManager::new(client)
                .await
                .expect("redis reachable"),
        );
        let loaded = second.get(&token).await.unwrap().expect("session survives restart");
        assert_eq!(loaded.user_id, identity.user_id);
        assert_eq!(loaded.role, Role::Admin);
        assert_eq!(loaded.username, identity.username);

        second.remove_user(identity.user_id).await.unwrap();
        assert!(first.get(&token).await.unwrap().is_none());
    }

    /// Gated: threshold failures lock the username; clearing unlocks it.
    #[tokio::test]
    async fn redis_lockout_round_trips_when_configured() {
        let Some((_, store)) = redis_store_from_env().await else {
            return;
        };
        let username = format!("lockout-{}", Uuid::new_v4());
        assert!(!store.is_locked(&username).await);
        for _ in 0..LOCKOUT_THRESHOLD {
            store.register_failure(&username).await;
        }
        assert!(store.is_locked(&username).await);
        store.clear_failures(&username).await;
        assert!(!store.is_locked(&username).await);
    }
}
