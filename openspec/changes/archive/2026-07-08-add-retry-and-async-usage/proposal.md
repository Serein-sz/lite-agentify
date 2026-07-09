## Why

Two gaps in the current gateway hurt real deployments:

1. **Rate-limit failures give up too early.** The failover chain only advances on a transport error or HTTP 5xx; HTTP 429 and 529 (provider overloaded) are forwarded straight to the client. For upstream LLM providers these are the *most common* recoverable errors, and they are usually transient — a short wait and a retry against the same provider often succeeds. Today the multi-provider chain does nothing for the failure mode operators hit most.
2. **Usage persistence sits on the proxy hot path.** For non-streaming responses the gateway `await`s a Postgres `INSERT` before returning the body to the client, so every non-streaming request pays a database round-trip. A DB slowdown directly slows proxying.

This change makes the failover chain retry rate-limited providers with backoff before advancing, and moves all usage persistence off the hot path onto a buffered background writer.

## What Changes

- Add a `[retry]` configuration section (hot-reloadable) controlling per-provider retry of rate-limit responses: retryable status set (default `{429, 529}`), max attempts per provider, and backoff delays.
- **Failover semantics change (429/529):** when a provider returns a retryable status, the gateway now waits (backoff, honoring a capped `Retry-After`) and retries the *same* provider up to the configured attempt count, and only advances to the next provider once attempts are exhausted. Other 4xx and all 2xx/3xx are still forwarded immediately; transport errors and 5xx still fail over immediately (unchanged). This reverses the prior explicit non-goal that treated 429 as non-retryable.
- **Usage persistence moves off the hot path:** both streaming and non-streaming usage records are handed to a bounded in-process channel and written by a background task that batches inserts (by count or interval). The proxy never awaits a database write. A full channel drops the record with a warning rather than blocking.
- **Graceful shutdown:** on shutdown the gateway stops accepting connections, flushes the pending usage batch, then exits.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `llm-gateway`: failover treats configured rate-limit statuses (429/529) as retryable with bounded backoff against the same provider before advancing the chain; usage persistence becomes an asynchronous, batched background write that never blocks the proxy response, with a graceful-shutdown flush.

## Impact

- Config format (additive, non-breaking): new optional `[retry]` section; absent config uses defaults. Hot-reloadable alongside existing fields.
- Affected code:
  - `src/config.rs`, `src/model.rs` — `RetryConfig` and its validation/defaults.
  - `src/proxy/router.rs` — failover loop gains per-provider retry + backoff for retryable statuses; usage recording no longer awaited inline.
  - `src/usage/recorder.rs` — `SeaOrmUsageRecorder` becomes a channel producer; new background batch-writer task; `insert_many` batching; shutdown flush.
  - `src/main.rs` — spawn the writer task, wire graceful shutdown (`axum::serve(...).with_graceful_shutdown(...)`), flush on signal.
  - `src/proxy/tests.rs`, `src/usage/*` tests — retry/backoff behavior and async-write fixtures.
- New dependency consideration: backoff jitter needs a small RNG; `rand` is already in the tree via transitive deps but not a direct dependency — add it as a direct dependency, or derive jitter from `uuid`/time to avoid a new direct dep (decided in design).
- Docs: README gains a `[retry]` section and an updated usage-recording note (eventual-consistency window); the `usage_records` DDL gains indexes (see separate indexing item).
- Out of scope: per-provider or per-key rate limiting/quota, circuit breaking / health memory across requests, a total cross-attempt time budget, upstream request timeouts (deferred), retrying transport errors or 5xx against the *same* provider (those still fail over immediately as today).
