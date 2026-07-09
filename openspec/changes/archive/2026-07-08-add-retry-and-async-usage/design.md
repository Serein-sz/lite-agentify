## Context

The gateway is a single-process pass-through proxy with a config snapshot held in an `ArcSwap`, an ordered provider failover chain per route, and optional Postgres usage persistence via SeaORM. Two current-state facts drive this change:

- `proxy()` / `ProxyContext::attempt_provider` in `router.rs` classify each upstream attempt as `Forward` (2xx/3xx/4xx incl. 429), `Failover` (transport error or 5xx), or `AliasMissing`. Each provider is tried **at most once** per request; the retry ceiling is the chain length.
- `record_usage` is `await`ed inline. For non-streaming responses this happens *before* the body is returned to the client (`router.rs` `forward_upstream_response`), so a slow `INSERT` slows the proxied response. For streaming responses it runs after the stream completes, off the client's critical path but still on the request task.

Operators run same-protocol failover chains (e.g. `x100labs → anyrouter → k40`) against upstream LLM providers whose dominant recoverable error is rate limiting (429) and overload (Anthropic 529), not 5xx. The failover chain therefore does little for the failure mode they hit most.

## Goals / Non-Goals

**Goals:**
- Retry a provider that returns a configured retryable status (default `{429, 529}`) with bounded backoff before advancing to the next provider in the chain.
- Honor a provider's `Retry-After` when present, capped to a configured maximum so a hostile/large value cannot hang the request.
- Move all usage persistence (streaming and non-streaming) off the request task onto a bounded background writer that batches inserts.
- Never block or slow the proxy because of usage persistence; a saturated writer drops records with a warning.
- Flush pending usage on graceful shutdown.
- Keep the `[retry]` config hot-reloadable, consistent with existing hot-reload fields.

**Non-Goals:**
- Per-provider / per-key rate limiting or quota (a *product* feature, not this change).
- Circuit breaking or cross-request health memory — each request still re-tries the full chain from the top.
- A total cross-attempt time budget for a request.
- Upstream request/connection timeouts (a real gap, explicitly deferred — see Risks).
- Retrying transport errors or non-retryable 5xx against the *same* provider; those still fail over immediately to the next provider, unchanged. (A 5xx that is *also* in the configured retryable set — e.g. 529 — is retried against the same provider; the retryable-status check takes precedence over the generic 5xx failover.)
- At-least-once / durable usage delivery. Usage remains best-effort; a dropped or crash-lost record is acceptable, exactly as a failed inline write is today.

## Decisions

### Decision 1: Retryable statuses trigger same-provider backoff retry inside `attempt_provider`

The failover loop keeps its existing `Forward | Failover | AliasMissing` outcomes; the retry lives *inside* `attempt_provider`, which builds the upstream URI/headers once and loops the send up to `max_attempts` for the same provider. The classification each send, in precedence order:

```
for provider in chain (index tells is_last):
    build uri + headers once
    for attempt in 0..max_attempts:      # attempt 0 is the first try
        send(provider)
        match result:
            transport error -> return Failover        # advance chain, unchanged
            retryable status (in configured set, e.g. 429/529):
                if attempt is last:
                    if is_last provider -> return Forward(real response)   # nowhere to fail over
                    else                -> return Failover                 # advance chain
                sleep(backoff(attempt, Retry-After)); continue             # retry SAME provider
            5xx (not retryable) -> return Failover     # advance chain, unchanged
            else (2xx/3xx/non-retryable 4xx) -> return Forward(real response)
# chain exhausted -> last_error or 502, unchanged
```

The retryable-status check is evaluated **before** the generic 5xx check, so a 5xx that is also in the retryable set (e.g. 529) is retried against the same provider rather than failing over.

**Rationale:** Keeping the retry inside `attempt_provider` avoids threading a new `Retryable` outcome and per-provider attempt state through the chain loop; the URI/headers are built once and only the send repeats. Retry is distinct from failover because the action (sleep + retry same target) and the trigger (rate limit, transient) differ from a hard provider failure (advance immediately). Non-retryable 5xx and transport errors keep their immediate-failover behavior because a crashed or unreachable provider will not recover within a request's backoff window, whereas a 429 usually will.

**Interaction with failover:** exhausting retries on a provider is exactly the point where the old behavior (advance to next provider) resumes. A single-provider chain simply retries that one provider up to the cap, then returns the last retryable response to the client — strictly better than today's immediate passthrough for a chain of length one.

### Decision 2: Backoff is bounded, jittered, and `Retry-After`-aware

The delay before retry attempt *n* (n = 0 for the first retry, i.e. after the initial failed try):

```
capped_exp   = min(base_delay * multiplier^n, max_delay)
retry_after  = min(parsed Retry-After, max_delay)     # if header present
target       = max(capped_exp, retry_after)           # respect the server, but our exp floor still applies
delay        = random_between(0, target)              # full jitter
```

Defaults (per the "wait a bit, try several times" intent — gentle, not steep): `base_delay = 1s`, `multiplier = 2`, `max_delay = 10s`, `max_retries = 3` (so up to 4 attempts per provider). `Retry-After` is parsed in both delta-seconds and HTTP-date forms; anything above `max_delay` is truncated to `max_delay` (a provider cannot force us to hang). Full jitter (random in `[0, target]`) prevents a thundering herd of concurrent requests all waking against the same rate-limited provider at the same instant.

**Rationale:** Capping every wait at `max_delay` is what makes retry safe *without* the deferred upstream timeout — no single wait, even an adversarial `Retry-After: 3600`, can exceed `max_delay`. Full jitter is the standard AWS-recommended approach for correlated retries.

**Alternative considered:** honoring `Retry-After` exactly (uncapped). Rejected — without an upstream request timeout (out of scope) an uncapped wait reintroduces the hang this cap exists to prevent.

### Decision 3: Body replay across retries reuses the existing `Bytes` clone

The request body is already buffered as `Bytes` and cloned per provider attempt for failover. Retrying the same provider clones the same `Bytes` again — no new buffering, no change to streaming safety (the retry decision is still made from the upstream *status* before any body is forwarded, identical to the failover invariant). Model-alias resolution already runs per attempt via `body_for_provider`, so a retried attempt rebuilds the same provider body deterministically.

### Decision 4: `SeaOrmUsageRecorder` becomes a channel producer; a background task owns all writes

Replace the inline `record()` `INSERT` with a bounded `tokio::sync::mpsc` channel. `record()` becomes a synchronous `try_send` of the `UsageRecord` and returns immediately:

```
record(rec):  match tx.try_send(rec)
                  Ok            -> Ok(())
                  Err(Full)     -> warn!("usage buffer full, dropping record"); Ok(())
                  Err(Closed)   -> warn!(...); Ok(())     # writer gone (shutdown); never error the caller
```

A dedicated background task drains the channel and batches:

```
loop:
    collect up to BATCH_MAX records, or until BATCH_INTERVAL elapses since the first buffered record
    if batch non-empty: insert_many(batch); on error warn! and drop the batch
    on channel closed + drained: flush remaining, then exit
```

Defaults: channel capacity `1024`, `BATCH_MAX = 128`, `BATCH_INTERVAL = 1s`. `insert_many` is a single multi-row `INSERT`, replacing N single-row inserts.

**Rationale:** `try_send` is the property that guarantees the proxy never blocks or awaits on persistence — the trait method stays `async` for signature compatibility but does no I/O. Dropping on `Full` upholds the existing "usage never affects the response" contract (a dropped record is strictly less harmful than a blocked proxy). Batching turns per-request round-trips into per-interval ones.

**Consequence — eventual consistency:** the dashboard now lags real time by up to `BATCH_INTERVAL` (~1s). Acceptable for a usage dashboard; documented in README.

**Consequence — `record()` no longer surfaces write errors.** The existing spec scenario "usage persistence write fails → log and still forward" is now satisfied structurally: the write happens elsewhere and the proxy path cannot observe it. The batch writer logs failures via the existing `warn_record_error`.

### Decision 5: The writer task and shutdown flush are owned outside the recorder

`recorder_from_config` returns the `Arc<dyn UsageRecorder>` (channel producer) **and** spawns the writer task, returning a shutdown handle (e.g. the `JoinHandle` plus a way to close the channel). `main.rs` wires `axum::serve(listener, app).with_graceful_shutdown(shutdown_signal())`, and after serve returns, drops the producer / closes the channel and `await`s the writer's `JoinHandle` so the final batch flushes before process exit.

```
main:
    recorder, writer = recorder_from_config(...)         # writer spawned here
    serve.with_graceful_shutdown(ctrl_c / SIGTERM)
    # serve returned => no new requests
    drop(recorder producer) / close channel
    writer.await                                          # flush final batch
```

**Rationale:** the recorder shouldn't own process lifecycle; keeping spawn+join at the edges (`main`) matches how the config watcher is already spawned there. `NoopUsageRecorder` returns no writer handle, so the shutdown path is a no-op when persistence is disabled.

**Signal handling:** `tokio::signal::ctrl_c()` plus a Unix `SIGTERM` handler (containers stop via SIGTERM). On Windows only ctrl_c is wired. This is the minimum for the Docker deployment to flush on `docker stop`.

### Decision 6: Jitter RNG source — add `rand` as a direct dependency

Full jitter needs a cheap RNG. `rand` is already compiled (transitive via `argon2`/`sqlx`), so adding it as a *direct* dependency adds no new crate to the build graph. The alternative (deriving pseudo-jitter from `uuid::new_v4` bytes or the system clock nanos) avoids the direct-dep line but is obscure; using `rand` is clearer for a one-line `rng.gen_range(0..=target_ms)`.

**Decision:** add `rand` as a direct dependency.

## Risks / Trade-offs

- **No upstream request timeout (deferred).** Retry mitigates this only for retryable statuses; a provider that accepts the connection and then hangs still blocks the request indefinitely, and the backoff cap does not help there. This is the pre-existing ① gap, consciously out of scope. Mitigation: the `max_delay` cap ensures *retry waits* never compound the problem; a follow-up change should add connect/request timeouts.
- **Retries add latency to genuinely-down rate-limited providers.** A provider hard-stuck at 429 now costs up to `sum(backoff) ≈ 1+2+4+... capped` before the chain advances, versus immediate passthrough today. Mitigation: `max_retries` and `max_delay` bound the total; defaults keep worst-case per-provider wait to a few seconds.
- **Usage records can be lost.** Channel-full drops and crash-before-flush both lose records. Mitigation: accepted — usage is best-effort by existing contract; graceful shutdown flush covers the common (SIGTERM) case, only hard crash loses the in-flight batch.
- **Eventual-consistency window on the dashboard.** Up to `BATCH_INTERVAL`. Mitigation: documented; negligible for usage analytics.
- **Hot-reloading `[retry]` mid-request.** A request reads one config snapshot at start (existing `shared.load()` invariant); retry parameters are read from that snapshot, so an in-flight request completes with consistent retry settings even if a reload swaps them. No special handling needed.

## Migration

- Config is additive: existing configs parse unchanged and get default `[retry]` behavior. No BREAKING marker.
- The `usage_records` DDL gains indexes (`created_at DESC`; `(provider_id, created_at DESC)`; `upstream_model`). New deployments get them from the README DDL; the existing server database needs the indexes added via `CREATE INDEX` (an operational step in tasks, not code).
