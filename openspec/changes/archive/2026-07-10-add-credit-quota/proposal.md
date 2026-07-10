# Proposal: add-credit-quota

## Why

Every request now has an attributed caller (change 1), a mandatory price (change 3's pricing gate), and an admin who curates what can be called — but nothing limits spend. The agreed model is prepaid cumulative USD credit: admins grant credit to users, usage draws it down, and requests are softly rejected once the balance is exhausted. Keys can carry their own spend cap as a budget sub-envelope. Enforcement must not put the database on the proxy hot path, so balances are enforced from fast counters — Redis when configured (surviving restarts and shared across instances), in-memory otherwise — reconciled against Postgres.

Final change of the sequence: `add-user-account-system` → `move-config-to-database` → `add-model-catalog` → **`add-credit-quota`**.

## What Changes

- Add a `credit_grants` ledger table: admins grant (or correct with negative amounts) USD credit to users; grants are append-only and auditable. A user's balance = Σ grants − Σ attributed usage cost.
- Enforce a soft spending limit before upstream contact: user balance exhausted → protocol-native `402` error; key spend cap (optional per-key cumulative USD cap) exceeded → `402` scoped to that key. Soft means bounded overdraft: in-flight requests and counter lag may exceed the limit by a small margin, never blocking the response path.
- Spend counters: per-user and per-key cumulative spent, incremented as usage records are produced, checked on every request. Backend is Redis (`INCRBYFLOAT`) when a `redis` config section is present, in-memory otherwise; counters are seeded from Postgres at startup and periodically reconciled against it.
- Optional Redis integration (new `redis` config section, connection string treated as a secret): spend counters, admin/user sessions (sessions survive gateway restarts), and login-lockout state move to Redis when configured, with automatic in-memory fallback and reconnection when Redis is unavailable. A config-refresh pub/sub channel is reserved for future multi-instance deployment.
- Console: admin credit management (grant credit, correction entries, per-user balance and ledger view), user-visible balance and spend on the dashboard, key cap editing in key management.

## Capabilities

### New Capabilities

- `credit-quota`: grants ledger, balance semantics, per-key caps, soft enforcement before upstream contact, counter seeding/reconciliation, admin grant API, balance visibility APIs.
- `redis-hot-state`: optional Redis configuration, counter/session/lockout storage with in-memory fallback and reconnection semantics, reserved config-refresh channel.

### Modified Capabilities

- `admin-auth`: sessions and login lockout use Redis when configured (sessions survive restart); in-memory behavior unchanged otherwise.
- `api-key-management`: keys gain an optional cumulative USD spend cap, editable by the owner within admin-set credit; key listings expose spent-to-date.
- `admin-ui`: credit management view (admin), balance/spend display (all roles), key cap field in key management.

## Impact

- **Code**: new migration + entities (`credit_grants`), `src/quota/` (counters, enforcement, reconciliation), `src/proxy/` (pre-flight balance check, post-usage increment), `src/admin/` (grant/balance APIs, session store abstraction), `src/state.rs`/config (redis section), `ui/` (credit views, balance cards, key cap).
- **APIs**: new `/admin/api/credits` (grant, ledger, balances), balance fields on `/admin/api/me`-style responses, key cap on key endpoints; proxied requests can now fail `402`.
- **Dependencies**: a Rust Redis client (e.g. `redis`/`fred`) — used only when configured.
- **Deployment**: Redis optional (docker-compose gains an optional service); the provided Redis (`:6379`, password auth) can be configured via the new section; connection string is masked like `database.url`.
- **Migration**: purely additive schema; users start with zero balance — admins must grant credit before users can spend, so the rollout order is: deploy → grant credits → (optionally) communicate the new `402` semantics to clients.
