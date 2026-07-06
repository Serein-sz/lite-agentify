## Context

The gateway currently authenticates requests, routes them through provider chains, rewrites provider-specific model aliases, preserves provider-native responses, and logs operational metadata through tracing. The existing metadata requirement does not provide durable records, token counts, or cost estimates.

This change adds an optional usage recording pipeline. Usage recording must fit the current proxy behavior: request and response schemas stay provider-native, streaming responses remain transparent, and usage failures must not break client traffic.

## Goals / Non-Goals

**Goals:**
- Persist token usage and estimated cost to PostgreSQL through SeaORM when usage database configuration is present.
- Read database connectivity and model pricing from gateway configuration.
- Support OpenAI-compatible and Anthropic-compatible usage formats.
- Account for cached token pricing without double-counting regular input tokens.
- Observe provider-native streaming usage metadata while forwarding stream bytes unchanged.
- Keep prompt, message, and completion content out of persisted records.
- Provide commented example configuration for usage database and pricing fields.

**Non-Goals:**
- Provide a billing UI or reporting API.
- Guarantee exact cost when provider usage metadata is absent or provider pricing is not configured.
- Estimate tokens from prompt text or completion text.
- Introduce request rejection when usage recording or usage persistence fails.
- Automatically fetch live pricing from provider websites.

## Decisions

### Use SeaORM with PostgreSQL as an optional persistence layer

Usage persistence will be enabled only when configuration includes an enabled usage database section. The gateway will initialize a SeaORM database connection for PostgreSQL and store records in a `usage_records` table.

Alternatives considered:
- Tracing-only usage records: lowest implementation cost, but not durable or queryable.
- JSONL files: simple local persistence, but weak for aggregation, concurrency, and production deployment.
- SQLite: lighter than PostgreSQL, but the requested deployment target is PostgreSQL.

### Store one final usage record per proxied request

The first durable model is request-level, not attempt-level. It records the provider that produced the final forwarded response, status, latency, requested model, upstream model, usage source, token counts, and estimated cost. Provider attempts skipped due to alias mismatch or failed during failover remain observable through existing logs and may be added as a later table.

Alternatives considered:
- Attempt-level records for every provider call: better failover analytics, but more schema and lifecycle complexity for the first cost tracking version.
- Aggregated daily totals only: useful for reporting, but loses auditability and makes pricing changes harder to explain.

### Treat pricing as deployment configuration

Pricing is read from TOML configuration by `provider_id` and upstream model name. Each pricing entry includes currency, input price, output price, and optional cached input, cache read, and cache write prices per 1 million tokens.

Model prices change independently of gateway releases, so hard-coded prices are excluded. If a response includes usage tokens but no matching pricing entry exists, the gateway records token counts and leaves estimated cost unavailable.

### Normalize provider usage into internal token classes

Provider-native usage fields will be parsed into internal classes:
- regular input tokens
- output tokens
- cached input tokens
- cache read tokens
- cache write tokens
- total tokens

OpenAI-compatible cached prompt tokens are subtracted from regular input before cost calculation. Anthropic-compatible cache creation and cache read tokens are subtracted from regular input before cost calculation. This prevents double-counting cache tokens as both normal input and cache usage.

### Wrap streaming bodies with a passive usage observer

For streaming responses, the gateway will forward every chunk unchanged while a side observer inspects provider-native SSE events for usage metadata. When the stream ends, the observer records usage if available. Parse errors or missing usage metadata produce a usage record with `usage_source = unavailable` and no estimated cost.

Alternatives considered:
- Buffer full streams before forwarding: easier parsing, but breaks streaming behavior and latency.
- Estimate tokens locally: not reliable enough for cost accounting.

### Keep persistence failures out of the client path

Usage extraction, cost calculation, and database writes are best-effort for this change. Failures are logged and must not change the upstream response status or body.

## Risks / Trade-offs

- Stream parsing can miss usage metadata if providers use unexpected event shapes -> record `usage_source = unavailable` and keep the response intact.
- PostgreSQL outages can drop usage records -> log write failures; a future change can add buffering or strict accounting mode.
- Pricing configuration can become stale -> store pricing source/version fields and require operators to update config.
- Numeric cost precision matters -> use decimal storage in PostgreSQL and avoid floating point for persisted cost.
- Response body parsing for non-streaming responses can conflict with transparent proxying if implemented by consuming the body incorrectly -> parse from buffered bytes and rebuild the response body unchanged.

## Migration Plan

1. Add configuration fields for usage database and pricing, with commented examples in project documentation or sample config comments.
2. Add SeaORM entities and migration/schema setup for `usage_records`.
3. Initialize the optional usage recorder during gateway startup when database configuration is enabled.
4. Add response usage parsers and cost calculator tests before wiring them into routing.
5. Wire non-streaming and streaming usage recording into the proxy path.
6. Deploy with usage database disabled by default, then enable PostgreSQL and pricing config in environments that need cost tracking.

Rollback is to disable the usage database configuration. The proxy path remains functional without usage persistence.

## Open Questions

- Should the first implementation include an explicit migration command, automatic migration on startup, or documented SQL only?
- Should a later change add strict mode where database connection failure prevents startup for billing-critical deployments?
- Should attempt-level failover records be added after request-level cost recording is stable?
