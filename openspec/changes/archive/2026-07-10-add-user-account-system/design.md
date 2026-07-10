# Design: add-user-account-system

## Context

Today authentication is flat: `gateway_keys` is a `HashSet<String>` in the TOML config checked on the hot path, and the admin console is guarded by one shared `admin_password` (argon2id-hashed on first boot, in-memory sessions). Usage records carry no caller identity. The four-change sequence agreed in exploration lands on: Postgres as the source of truth, an arc-swap in-process snapshot on the hot path, models as the only external surface, and cumulative USD credits. This change lays the account foundation; it deliberately does NOT touch provider/route config (change 2), model permissions (change 3), or quotas (change 4).

Constraints:
- The proxy hot path must stay free of per-request database queries.
- Existing clients authenticate with static keys; they must survive the cutover.
- The existing session, lockout, and password write-back mechanisms are sound — reuse, don't rebuild.

## Goals / Non-Goals

**Goals:**
- `users` and `api_keys` tables in Postgres; Postgres becomes mandatory.
- Username + password login with role-carrying sessions (`admin` / `user`).
- Self-service API key lifecycle; admin-managed user lifecycle.
- Every proxied request attributed to a user and key in `usage_records`.
- Zero-downtime migration for existing `gateway_keys` clients.

**Non-Goals:**
- Key→model permissions (`allowed_models` lands with the model catalog, change 3).
- Quotas, balances, credit grants (change 4).
- Redis anything (change 4; sessions stay in-memory here).
- Moving providers/pricing into the database (change 2).
- Open registration, email flows, password self-reset (admin resets passwords).

## Decisions

### D1: Key format and storage — random token, SHA-256 lookup hash, display prefix

Keys are `la-<32 bytes base62>` generated server-side. The database stores `sha256(key)` for lookup plus the first 8 characters as a display prefix; plaintext is returned exactly once at creation. SHA-256 (not argon2) because keys are high-entropy random strings — brute force is infeasible, and the hot path needs a cheap deterministic digest to index the snapshot map. Argon2 stays for passwords only (low-entropy, human-chosen).

*Alternative considered*: storing plaintext keys (like today's `gateway_keys`) — rejected: a database dump must not yield usable credentials.

### D2: Hot path — key hashes live in the arc-swap snapshot

`GatewayState` gains `api_keys: HashMap<[u8; 32], KeyIdentity>` (key-hash → user_id/key_id/status). Request auth becomes: hash the presented token, one HashMap lookup — same cost profile as today's HashSet. The snapshot is rebuilt from Postgres on boot and refreshed when an admin/user mutates keys or users through the API (the mutation handler triggers a state rebuild, replacing the file-watcher trigger for this slice of state). A disabled user's keys are excluded at snapshot build time, so disabling a user revokes access on the next snapshot swap.

*Alternative considered*: per-request `SELECT` with a small TTL cache — rejected: violates the zero-DB hot path constraint and adds tail latency.

### D3: One database, one config section

`usage_database` is renamed to `database` (with a deprecation alias so old configs keep parsing, warning logged) and becomes required. Accounts and usage share the pool. Sea-orm migrations (a `migration` module run at startup) create `users`, `api_keys`, and alter `usage_records`. Startup fails fast if the database is unreachable or migrations fail.

*Alternative considered*: separate optional usage DB + required accounts DB — rejected: two pools/two configs for one Postgres instance is complexity without a user.

### D4: Bootstrap and legacy-key import are idempotent boot steps

On boot, inside one transaction:
1. If `users` is empty: create user `admin` (role `admin`) from the config `admin_password` (reusing the existing plaintext→hash write-back; if the value is already a PHC hash, use it directly).
2. If `api_keys` is empty and the config still contains `gateway_keys`: import each entry as an active key owned by `admin`, named `imported-<n>`. Legacy plaintext keys hash the same way (`sha256`), so existing clients' credentials keep working unchanged. A warning tells the operator to delete `gateway_keys` from the file.

After import, `gateway_keys` in config is ignored (warn if present). This gives a strict once-only migration with a clean rollback story (the file is never modified except the already-existing password write-back).

### D5: Sessions gain identity; login gains username

The in-memory session store now maps token → `{user_id, role, expiry}`. `POST /admin/api/login` takes `{username, password}`. Lockout counting moves from global to per-username (5 failures → 60s lockout for that username) to prevent one attacker locking out everyone. Session cookie mechanics (HttpOnly, SameSite=Strict, `/admin` path, 24h TTL, survives reload, dies on restart) are unchanged.

### D6: Authorization is a two-level role check in handlers

`admin`-only endpoints (user management, config, all-usage) check `session.role == Admin`; self-service endpoints (own keys, own usage, own password change) check only a valid session and scope queries by `session.user_id`. No permission table, no middleware framework — an extractor that yields the session identity, and handlers assert the role. Two roles is a deliberate ceiling for this change.

### D7: Usage attribution flows through the existing observer

`UsageRecord` gains `user_id: Option<Uuid>` and `api_key_id: Option<Uuid>`, populated at request time from the snapshot lookup and handed to the existing async recorder untouched. Columns are nullable; historical rows stay NULL and the dashboard treats NULL as "pre-accounts".

## Risks / Trade-offs

- [Postgres becomes a single point of failure for startup] → Fail fast with a clear error; document the docker-compose change; the gateway keeps serving with its last snapshot if the DB dies *after* boot (auth still works, key mutations and usage writes fail visibly in logs).
- [Snapshot refresh on mutation adds a rebuild per admin action] → Rebuilds are already the reload mechanism today (file watcher); frequency is human-scale.
- [Legacy keys imported as admin-owned means old clients bill to admin] → Acceptable transitional state; operator re-issues per-user keys and revokes imports at their pace.
- [Per-username lockout allows username enumeration via timing] → Uniform 401 for unknown-user and wrong-password; lockout replies 429 regardless of whether the username exists.
- [In-memory sessions still die on restart] → Known, unchanged from today; Redis session store arrives in change 4.

## Migration Plan

1. Deploy Postgres (docker-compose gains a required service) — can precede the binary.
2. Start new binary: migrations run, bootstrap admin + key import happen in one transaction.
3. Verify old clients still proxy (imported keys), log in as `admin`, create real users/keys.
4. Remove `gateway_keys` from the TOML.
5. Rollback: previous binary ignores the new tables entirely; `gateway_keys` still in the file keeps old auth working. The two new `usage_records` columns are nullable and harmless to the old binary.

## Open Questions

- None blocking. UI route naming (`/admin` serving non-admin users) was deferred by agreement; revisit in change 4's portal polish or a UI-only follow-up.
