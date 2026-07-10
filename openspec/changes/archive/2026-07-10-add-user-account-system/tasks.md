# Tasks: add-user-account-system

## 1. Database foundation

- [x] 1.1 Rename `usage_database` config to required `database` section (keep deprecated alias with warning); fail startup when absent or unreachable
- [x] 1.2 Add a migration runner executed at startup (sea-orm migration module) and port the existing `usage_records` table creation into it
- [x] 1.3 Migration: create `users` table (id, username unique, password_hash, role, status, created_at)
- [x] 1.4 Migration: create `api_keys` table (id, user_id FK, key_hash unique, prefix, name, status, created_at, last_used_at)
- [x] 1.5 Migration: add nullable `user_id` and `api_key_id` columns to `usage_records`
- [x] 1.6 Update docker-compose and README for mandatory Postgres

## 2. Account domain module

- [x] 2.1 Create `src/account/` with sea-orm entities for `users` and `api_keys`
- [x] 2.2 Key generation: `la-` + 40 base62 chars (>192 bits), SHA-256 hashing, prefix extraction; unit tests
- [x] 2.3 Bootstrap step in one transaction: seed `admin` user from `admin_password` when `users` is empty (reuse plaintext→argon2 write-back; fail startup if empty table and no password), import `gateway_keys` as admin-owned keys when `api_keys` is empty
- [x] 2.4 Account repository functions: create/list/disable/enable users, reset password, change own password, create/list/revoke keys, touch last_used_at

## 3. Hot-path auth swap

- [x] 3.1 Extend `GatewayState` with `api_keys: HashMap<[u8;32], KeyIdentity>` (user_id, key_id) built from DB, excluding revoked keys and disabled users
- [x] 3.2 Replace `gateway_keys` HashSet auth in `is_authorized` with SHA-256 snapshot lookup returning the caller identity; remove `gateway_keys` from config struct and validation (warn if present in file)
- [x] 3.3 Add a snapshot refresh trigger invoked after any user/key mutation (rebuild state from DB, arc-swap store); keep file-watcher reload working for the remaining file-based fields
- [x] 3.4 Update proxy tests for key-based auth

## 4. Usage attribution

- [x] 4.1 Add `user_id`/`api_key_id` to `UsageRecord`, populate from the auth lookup, persist via the existing recorder
- [x] 4.2 Usage query layer: filter by user/key; scope queries to a user id for `user`-role callers; treat NULL attribution rows as pre-accounts history

## 5. Session and admin API

- [x] 5.1 Sessions store user_id + role; login takes `{username, password}` with uniform 401 and per-username lockout (429)
- [x] 5.2 Session extractor yielding identity; `403` guard for admin-only endpoints; reject sessions of disabled users
- [x] 5.3 New endpoints: `GET/POST /admin/api/users`, `POST /admin/api/users/{id}/disable|enable|reset-password` (admin-only); `POST /admin/api/me/password`; `GET/POST /admin/api/keys`, `POST /admin/api/keys/{id}/revoke` (self-service; admin sees all)
- [x] 5.4 Remove the 404-when-no-admin-password gating (console always served); `admin_password` used only for bootstrap
- [x] 5.5 Scope existing usage endpoints by role (user → own rows only; admin → all + user/key filters)
- [x] 5.6 Integration tests: login/lockout/roles covered; key/bootstrap logic unit-tested (full DB-backed e2e requires Postgres, run at 6.7)

## 6. Console UI

- [x] 6.1 Login form gains username field; auth context via `/me` stores role
- [x] 6.2 Role-partitioned navigation and route guards (user: keys/own usage/password; admin: + users/config)
- [x] 6.3 Key management view: create with name, one-time plaintext display with copy + warning, list with prefix/status/timestamps, revoke with confirmation
- [x] 6.4 User management view (admin): list, create, disable/enable, reset password with confirmations
- [x] 6.5 Password change view
- [x] 6.6 Dashboard/usage views: user role sees own data (server-scoped); admin all
- [x] 6.7 UI builds (tsc+vite), full Rust test suite green (120 passed); live Postgres smoke test pending a running DB
