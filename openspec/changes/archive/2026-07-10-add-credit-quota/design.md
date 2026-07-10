# Design: add-credit-quota

## Context

Changes 1–3 delivered attributed callers, database-backed configuration, and a model catalog whose pricing gate guarantees every proxied request produces a cost. Usage persistence is asynchronous and eventually consistent by design (the response path never awaits the database). Quota must therefore enforce from a faster medium than the usage table. Exploration decisions: cumulative prepaid USD (no reset periods), soft limits (bounded overdraft acceptable), Redis available (already running, `:6379`, password auth) but as an **optional** dependency with in-memory fallback — "Redis 挂了可以降级,Postgres 挂了才是真的挂了".

## Goals / Non-Goals

**Goals:**
- Append-only credit ledger; balance = Σ grants − Σ usage cost.
- Pre-flight enforcement (user balance, key cap) with zero database access on the hot path.
- Redis-backed counters/sessions/lockout when configured; seamless in-memory fallback.
- Reconciliation loop bounding counter drift.

**Non-Goals:**
- Hard (transactional pre-reservation) quotas — rejected in exploration.
- Billing periods, resets, rolling windows — cumulative only.
- Multi-instance deployment — pub/sub channel is reserved, not implemented beyond the single-instance no-op.
- Payment integration; grants are manual admin actions.

## Decisions

### D1: Ledger is append-only; balance is derived, never stored

`credit_grants(id, user_id, amount_usd numeric, note, granted_by, created_at)` — no mutable "balance" column anywhere in Postgres. Corrections are negative grants. Balance is `Σ grants − Σ usage.estimated_cost` (per user; per key uses only the usage side against its cap). This keeps the audit trail primary and makes counter reconciliation a pure recomputation.

*Alternative*: mutable balance column debited by the usage writer — rejected: two writers (grants, usage) to one cell invites drift with no way to audit; the ledger recomputation is always right.

### D2: Enforcement reads a counter, never the database

The proxy pre-flight is: `spent_user + estimated_in_flight? nothing — just spent_user ≥ granted_user → 402` and `key cap set && spent_key ≥ cap → 402`, all from the counter backend (two reads) plus the granted total cached in the snapshot (refreshed on grant mutation, like every other snapshot field). After the response, the existing usage observer increments both counters by the record's cost — asynchronously, off the response path. Soft-limit drift is therefore bounded by: cost of requests in flight + increments not yet applied + (Redis mode) nothing else; (memory mode) records lost to a crash since the last reconciliation.

`402 Payment Required` with a protocol-native error body naming the exhausted scope (user balance vs key cap); `429` stays reserved for upstream rate limits to avoid confusing retry-with-backoff clients into hammering an empty wallet.

### D3: Counter backend is a trait with two implementations

`SpendCounter`: `get(scope) -> Decimal`, `add(scope, cost)`, `reset(scope, value)`. Implementations: `MemoryCounter` (DashMap-style atomics) and `RedisCounter` (`INCRBYFLOAT` on `spent:user:{id}` / `spent:key:{id}`, `GET` on read; no TTL — cumulative forever). Selection at startup from the presence of the `redis` config section. Redis command failures log once per outage window and flip a degraded flag routing to a memory shadow seeded from the last known values; a background probe reconnects and re-seeds Redis from Postgres. Enforcement never errors a request because the counter backend hiccuped — worst case it briefly under-counts (soft limit, by agreement).

### D4: Reconciliation is a periodic recomputation, both modes

Every 60s (configurable): recompute Σ grants and Σ usage per active scope from Postgres, `reset` counters to truth. This heals Redis drift (missed increments, restarts of Redis itself), heals memory-mode crash loss, and picks up out-of-band corrections. Boot seeds counters the same way before serving. Scopes touched lazily: only users/keys seen in the snapshot.

### D5: Sessions and lockout move behind the same optionality

A `SessionStore` trait over the existing in-memory map; `RedisSessionStore` keys `session:{token}` with the 24h TTL native to Redis, values carrying user id + role. Login lockout counters likewise (`lockout:{username}`, TTL = window). Configured Redis → sessions survive gateway restarts (fixing a known annoyance); Redis outage mid-session → session reads fail → affected admins re-login when it returns (fail-closed for auth, fail-open never). No session data is written anywhere else.

### D6: Redis config is a file-level bootstrap section

```toml
[redis]
url = "redis://:password@host:6379/0"   # secret, masked like database.url
```

Restart-only (like `database`); masked in config reads; revealable via the existing single-secret endpoint. The connection string carries the password — README notes to change the currently weak one and to firewall the port, since session tokens will live there.

### D7: Grant totals ride the snapshot; a pub/sub slot is reserved

Granted-sum per user is part of the snapshot (rebuilt on grant mutations — human-scale). A `config_changed` Redis channel name is reserved and documented; the single-instance subscriber is a no-op self-notification. Actual multi-instance fan-out is future work and deliberately unimplemented.

## Risks / Trade-offs

- [Soft overdraft can exceed the balance] → Bounded by in-flight cost + reconciliation interval; acceptable per exploration ("软限即可"). Worst-case widened in memory-mode crashes; reconciliation heals within one interval.
- [Float drift from INCRBYFLOAT] → Counters are advisory (enforcement threshold), truth is Postgres decimals; reconciliation snaps back every interval. Store USD as micro-dollar integers in Redis if drift proves visible.
- [Redis holds session tokens] → Documented: treat Redis access as account access; change the weak password, bind to private interfaces, `requirepass` minimum.
- [Zero-balance surprise at rollout] → Explicit migration step: grant credits before announcing; until granted, users' requests 402 with a clear message.
- [NULL-cost usage rows (pre-gate history, failed pricing)] → Counted as zero by both counter and reconciliation; the change-3 gate keeps new rows priced.

## Migration Plan

1. Migrate schema (additive); deploy binary with or without `[redis]`.
2. Admin grants initial credit per user (console or API); verify balances render.
3. Optionally add `[redis]`, restart, verify sessions survive a second restart and counters live in Redis.
4. Rollback: previous binary ignores `credit_grants` and the `redis` section entirely; no data loss.

## Open Questions

- None blocking. Micro-dollar integer counters (D3) and multi-instance pub/sub (D7) are pre-scoped follow-ups.
