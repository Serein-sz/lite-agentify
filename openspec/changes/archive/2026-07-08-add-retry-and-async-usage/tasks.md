## 1. Retry configuration

- [x] 1.1 Add `RetryConfig` to `src/config.rs`: optional `[retry]` section with `retryable_statuses` (default `[429, 529]`), `max_attempts` (default 4 attempts per provider), `base_delay_ms` (default 1000), `max_delay_ms` (default 8000). All fields `#[serde(default)]` so an absent section yields defaults.
- [x] 1.2 Add the resolved retry settings to `src/model.rs` (or a dedicated type) and thread them into `GatewayState` so they are part of the hot-reloadable snapshot.
- [x] 1.3 Validate `RetryConfig` in `state.rs` `from_config`: `max_attempts >= 1`, `base_delay_ms <= max_delay_ms`, retryable status codes in `100..=599`.

## 2. Per-provider retry with backoff

- [x] 2.1 In `router.rs`, add backoff computation: linear-ish/exponential delay from `base_delay_ms` growing to `max_delay_ms`, with full jitter (random in `[0, delay]`).
- [x] 2.2 Parse `Retry-After` (delta-seconds form) from the upstream response and cap the effective wait at `max_delay_ms`.
- [x] 2.3 Change `attempt_provider` / the failover loop so a retryable status (in the configured set) sleeps then retries the **same** provider up to `max_attempts`, and advances to the next provider only when attempts are exhausted.
- [x] 2.4 Keep transport-error and 5xx handling as immediate failover (no same-provider retry); keep non-retryable 4xx / 2xx / 3xx as immediate forward.
- [x] 2.5 When a single-provider chain exhausts retries, forward the last retryable response to the client (not a synthesized 502).
- [x] 2.6 Log each retry attempt (provider id, attempt number, status, delay) and each failover.

## 3. Asynchronous batched usage writer

- [x] 3.1 Introduce a bounded `mpsc` channel (capacity 1024) between the recorder producer and a background writer task.
- [x] 3.2 `SeaOrmUsageRecorder::record` becomes a non-blocking `try_send`; on a full channel, drop and `warn` (never block the proxy).
- [x] 3.3 Implement the background writer: accumulate records and flush on batch size (128) or interval (1s), whichever comes first, using `insert_many`.
- [x] 3.4 On a batch write failure, `warn` and drop that batch (no unbounded retry); keep the writer alive for subsequent batches.
- [x] 3.5 Update `router.rs` `record_usage` call sites (both streaming finish and non-streaming) so neither awaits a DB round-trip — enqueue only.
- [x] 3.6 Keep `NoopUsageRecorder` behavior unchanged (no channel, no task) when persistence is disabled.

## 4. Graceful shutdown

- [x] 4.1 In `main.rs`, build a shutdown signal future (Ctrl-C / SIGTERM) and pass it to `axum::serve(...).with_graceful_shutdown(...)`.
- [x] 4.2 On shutdown, close the usage channel and await the writer task so the final batch is flushed before exit.
- [x] 4.3 Ensure shutdown completes cleanly when persistence is disabled (no writer task to join).

## 5. Database indexes

- [x] 5.1 Update the `usage_records` DDL in `README.md` to add indexes: `created_at DESC`, `(provider_id, created_at DESC)`, and `upstream_model`.
- [x] 5.2 Create the same indexes on the deployed database (`CREATE INDEX IF NOT EXISTS ...`) so existing data benefits.

## 6. Documentation

- [x] 6.1 Add a `[retry]` section to `README.md` documenting fields, defaults, and the 429/529 retry-then-failover behavior.
- [x] 6.2 Note the usage-recording eventual-consistency window (records may lag the dashboard by up to the flush interval) and the graceful-shutdown flush.
- [x] 6.3 Add `[retry]` to the hot-reloadable fields list and confirm `usage_database` remains restart-only.

## 7. Tests

- [x] 7.1 Retry: primary returns 429 then 200 on retry → success forwarded, fallback never contacted.
- [x] 7.2 Retry: primary returns 429 for all attempts → advances to fallback after `max_attempts`.
- [x] 7.3 Retry: single-provider chain returns 429 for all attempts → last 429 forwarded to client.
- [x] 7.4 Retry: 529 is treated as retryable like 429.
- [x] 7.5 Retry: 5xx still fails over immediately with no same-provider retry; non-retryable 4xx still forwarded immediately.
- [x] 7.6 Backoff: `Retry-After` larger than `max_delay_ms` is capped (assert bounded wait, e.g. via injected clock or by asserting the computed delay).
- [x] 7.7 Config: absent `[retry]` yields defaults; invalid values (`max_attempts = 0`, `base > max`) fail startup validation.
- [x] 7.8 Async usage: a recorded request returns to the client without awaiting the write; the record is observable after a flush.
- [x] 7.9 Async usage: a saturated channel drops with a warning and does not block or fail the response.
- [x] 7.10 Shutdown: pending buffered records are flushed on graceful shutdown.

## 8. Verification

- [x] 8.1 Run `cargo build` and `cargo test`, fixing any failures.
- [x] 8.2 Run `openspec validate add-retry-and-async-usage --strict` and resolve any issues.
