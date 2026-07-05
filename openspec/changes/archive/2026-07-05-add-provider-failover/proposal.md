## Why

A single upstream provider is a single point of failure: when the primary provider is unreachable or returns server errors, every matched request fails even if an equivalent provider is available. Operators want to configure multiple same-protocol providers per route and have the gateway automatically fall back to the next one when the primary is down.

## What Changes

- **BREAKING**: `RouteConfig` replaces the single `provider: String` field with `providers: Vec<String>`, where array order defines failover priority (first = primary). Existing configs must change `provider = "x"` to `providers = ["x"]`.
- Routes now resolve to an ordered chain of candidate providers instead of a single provider.
- At request time, the gateway tries candidates in priority order, advancing to the next candidate only when the current one **fails to produce a usable response** (connection failure, timeout, or HTTP 5xx). Any other outcome (2xx, 4xx, 429) is forwarded to the client and stops the chain.
- Failover decisions are made from the upstream response status **before** any response body is streamed to the client, so streaming responses remain safe (no partial-then-retry).
- Startup validation is strengthened: every provider id in a route chain must exist, all providers in a chain must share the same protocol, and a route with an empty (or fully unresolved) chain is rejected/skipped.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `llm-gateway`: Routing changes from single-provider selection to an ordered provider chain with automatic failover on unusable responses; startup validation gains chain existence and protocol-consistency rules.

## Impact

- Config format (BREAKING): `routes[].provider` → `routes[].providers`.
- Affected code: `src/gateway/config.rs` (RouteConfig), `src/gateway/model.rs` (Route), `src/gateway/state.rs` (validation + route matching returns a chain), `src/gateway/router.rs` (proxy loop performs failover), `src/gateway/tests.rs` (config fixtures + new failover tests).
- No new dependencies. Request body is already `Bytes`, so cheap cloning enables replay across candidates.
- Out of scope: weighted/load-balanced distribution, circuit breaking / health memory, cross-protocol translation, total-timeout budgets, treating 429 as failover-eligible.
