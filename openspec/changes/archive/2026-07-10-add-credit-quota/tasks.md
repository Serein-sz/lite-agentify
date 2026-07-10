# Tasks: add-credit-quota

## 1. Schema and ledger

- [x] 1.1 Migration: create `credit_grants` (id, user_id FK, amount_usd numeric, note, granted_by FK, created_at); add nullable `spend_cap_usd` to `api_keys`
- [x] 1.2 Entities + repository: append grant, per-user grant sums, per-user/per-key usage cost sums (NULL cost = 0), ledger query
- [x] 1.3 Grant/ledger/balance admin+self APIs: `POST /admin/api/credits/grants` (admin), `GET /admin/api/credits` (admin: all users), `GET /admin/api/me/balance`; key endpoints gain cap set/edit and spent-to-date

## 2. Counters and enforcement

- [x] 2.1 `SpendCounter` trait + `MemoryCounter` implementation; boot seeding from Postgres sums; snapshot carries per-user granted totals (refreshed on grant mutation)
- [x] 2.2 Pre-flight check in the proxy path: user spent ≥ granted → protocol-native 402; key cap reached → 402 naming key cap; zero DB access, before any upstream contact
- [x] 2.3 Post-response increment: usage observer adds each record's cost to user and key counters off the response path
- [x] 2.4 Reconciliation loop (default 60s): recompute sums, reset counters; covers memory-mode crash loss
- [x] 2.5 Tests: exhaustion → 402, new grant unblocks, key cap independence, drift healing, NULL-cost rows

## 3. Redis integration

- [x] 3.1 Add optional `[redis]` config section (secret URL, masked + revealable, restart-only) and a Redis client dependency
- [x] 3.2 `RedisCounter` (INCRBYFLOAT/GET/SET) selected when configured; outage → in-memory shadow with warning, background reconnect + re-seed from Postgres truth
- [x] 3.3 `SessionStore` trait over the existing in-memory map; `RedisSessionStore` with native TTL; lockout state behind the same selection; session reads during outage → 401 (fail closed)
- [x] 3.4 Config-refresh channel: publish after snapshot-affecting mutations, subscribe → rebuild trigger (single-instance self-notification)
- [x] 3.5 Tests (gated, skipped without a Redis URL): counter round-trip, session survival across simulated restart, outage degradation

## 4. Console UI

- [x] 4.1 Credit management view (admin): user balances table, grant form with note, negative-correction confirmation, ledger history
- [x] 4.2 Balance cards on dashboard (own balance/granted/spent for all roles)
- [x] 4.3 Key management: cap field on create/edit, spent-to-date column, cap progress indication

## 5. Documentation and verification

- [x] 5.1 README: credit model (prepaid cumulative, soft limits, 402 semantics), Redis section (optional, what moves there, security notes: strong password, private binding), docker-compose optional redis service
- [x] 5.2 Full suite + manual verify: grant → spend → 402 → re-grant cycle end-to-end; with Redis: sessions survive restart, counters persist
