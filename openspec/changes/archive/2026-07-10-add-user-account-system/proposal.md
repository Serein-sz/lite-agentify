# Proposal: add-user-account-system

## Why

The gateway's access model is flat and all-or-nothing: any key in the static `gateway_keys` list grants full access to every route, there is no notion of who is calling, and the admin console is guarded by a single shared password. To support multiple users who self-manage their own API keys — and to later attach per-user/per-key model permissions and spending quotas — the gateway needs real user accounts, role separation, and database-backed API keys attributed on every request.

This is the first of four sequenced changes (`add-user-account-system` → `move-config-to-database` → `add-model-catalog` → `add-credit-quota`) converging on: Postgres as the source of truth, an in-process read-only snapshot on the hot path, models as the only externally visible surface, and cumulative USD credit quotas.

## What Changes

- Add a `users` table (argon2id password hash, `admin`/`user` role, `active`/`disabled` status) and an `api_keys` table (SHA-256 key hash, display prefix, name, status, owner) in Postgres.
- **BREAKING**: PostgreSQL becomes a hard dependency. The gateway refuses to start without a configured, reachable database (previously `usage_database` was optional).
- **BREAKING**: Proxy authentication switches from the static `gateway_keys` config list to database-backed API keys. The `gateway_keys` config field is removed; on first boot existing `gateway_keys` entries are imported as API keys owned by the bootstrap admin so existing clients keep working.
- Bootstrap: on first boot with an empty `users` table, the existing `admin_password` config value seeds the initial `admin` user (reusing the current plaintext→argon2id write-back mechanism).
- Login changes from password-only to username + password; sessions carry user identity and role. Session and login-lockout behavior is otherwise preserved.
- Self-service key management: any user can create, list, and revoke their own API keys; the plaintext key is shown exactly once at creation. Admins can manage users (create, disable, reset password) and see all keys.
- Usage records gain `user_id` and `api_key_id` attribution columns; users see only their own usage in the console, admins see everything.
- The web console gains role partitioning: `user`-role accounts see key management and their own usage; `admin` accounts additionally see user management and the existing config/usage views.
- API key lookups stay off the hot path via the existing arc-swap snapshot pattern: key hashes are loaded into the in-process snapshot and refreshed on change.

## Capabilities

### New Capabilities

- `user-accounts`: user records, roles, lifecycle (create/disable/password reset), bootstrap admin seeding, and the Postgres hard-dependency requirement.
- `api-key-management`: self-service key creation/listing/revocation, hashed storage with one-time plaintext reveal, key attribution on proxied requests, and import of legacy `gateway_keys`.

### Modified Capabilities

- `admin-auth`: login becomes username + password against user accounts; sessions carry identity and role; the single shared `admin_password` becomes only the bootstrap seed for the first admin user.
- `llm-gateway`: the "gateway key" authentication requirement is replaced by database-backed API key authentication resolved from the in-process snapshot.
- `admin-usage`: usage records are attributed to a user and key; usage visibility becomes role-scoped (own usage for users, all usage for admins).
- `admin-ui`: the console is partitioned by role; a `user` role sees only self-service views.

## Impact

- **Code**: `src/state.rs` (snapshot gains users/keys), `src/proxy/` (auth path), `src/admin/` (login, new user/key APIs, role checks), `src/usage/` (attribution columns), `src/config.rs` (remove `gateway_keys`, database config required), new `src/account/` module and SQL migrations, `ui/` (login form, key management, user management, role routing).
- **APIs**: `POST /admin/api/login` body changes (adds `username`); new endpoints for users and keys under `/admin/api/`.
- **Dependencies**: none new at runtime beyond making Postgres mandatory (sea-orm already present); `rand` for key generation if not already transitively available.
- **Deployment**: docker-compose Postgres service becomes required; config gains no new secrets (database URL already exists as `usage_database.url`, which this change renames conceptually to the primary database).
- **Migration**: one-time on-boot import of `gateway_keys` → admin-owned API keys; `usage_records` table gains two nullable columns (old rows keep NULL).
