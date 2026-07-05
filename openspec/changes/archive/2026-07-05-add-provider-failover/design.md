## Context

The gateway currently binds each route to exactly one provider via `RouteConfig.provider: String`. When that provider returns an error or is unreachable, the request fails with `502 Bad Gateway` — there is no automatic recovery. Operators who configure multiple equivalent providers (e.g. OpenAI direct + Azure OpenAI) have no way to express "prefer the first, fall back to the second."

Relevant current-state facts:
- `proxy()` in `router.rs` reads the request body into `Bytes`, matches a single `(&Route, &Provider)`, sends one upstream request, and forwards the result.
- `match_route()` in `state.rs` returns the first matching route's provider (array order is the only implicit priority today).
- `upstream.send()` returns a streaming `Body`; the upstream `status` and headers are available *before* any body bytes are forwarded to the client.
- The gateway is a pure pass-through — it never converts between the OpenAI and Anthropic protocols.

## Goals / Non-Goals

**Goals:**
- Allow a route to list an ordered chain of providers, where order expresses priority.
- On a hard failure of a provider (transport error or HTTP 5xx), automatically retry the next provider in the chain.
- Preserve streaming safety: never switch providers after any response body has been sent to the client.
- Validate at startup that a failover chain is internally consistent (providers exist, share one protocol).

**Non-Goals:**
- Weighted or load-balanced distribution across providers (failover only — the first healthy provider always wins).
- Treating HTTP 429 or any 4xx as a failover trigger (these are passed through to the client unchanged).
- Circuit breaking / health memory across requests (each request re-tries the full chain from the top).
- A total time budget across attempts.
- Cross-protocol failover (a chain mixing OpenAI and Anthropic providers is a configuration error).

## Decisions

### Decision 1: Route holds an ordered `providers` list (config format A)

`RouteConfig.provider: String` becomes `RouteConfig.providers: Vec<String>`, and `Route.provider_id: String` becomes `Route.provider_ids: Vec<String>`. Array order *is* the priority order.

```toml
[[routes]]
path_prefix = "/v1/chat/completions"
providers = ["openai-primary", "openai-azure"]  # order = priority
model_prefix = "gpt-"                             # unchanged, optional
```

**Rationale:** A single ordered list is the most direct representation of "priority chain." A single-provider route is simply a chain of length one, so the runtime path is uniform.

**Alternative considered:** Keeping `provider: String` and adding a separate `fallbacks: Vec<String>` field (backward compatible). Rejected in favor of a cleaner single-field model; this makes the change **BREAKING** for existing configs, which is accepted.

### Decision 2: Failover trigger = transport error OR HTTP 5xx only

The classification when a provider responds (or fails to):

| Outcome | Action |
|---|---|
| Transport error (connect refused, timeout, TLS) | try next provider |
| HTTP 5xx | try next provider |
| HTTP 2xx / 3xx / 4xx (incl. 429) | stop, forward this response to client |

**Rationale:** Only "the server never answered" or "the server crashed (5xx)" indicate a provider-level problem worth escaping. 4xx (including 401 bad key and 429 rate limit) reflects the request or account state — switching providers would not help and could mask real errors. This keeps the trigger purely status-code-based, which is what makes streaming safe (Decision 3).

**Alternative considered:** Treating 429 as failover-eligible. Rejected per product decision — rate limits pass through so clients can honor `Retry-After`.

### Decision 3: Failover decision happens before any body is forwarded

`upstream.send()` yields `status` + headers before streaming body bytes. The proxy loop inspects `status` first; if it is a failover trigger and another provider remains, it discards the response (client has received nothing yet) and tries the next provider. Only once a non-failover status is seen does the loop begin writing status/headers/body to the client.

**Rationale:** This is the invariant that makes failover safe for streaming responses. Because the trigger is status-code-only (Decision 2), no body inspection is ever required, so we never need to buffer or "un-send" a stream.

### Decision 4: Body is replayed by cloning `Bytes`

The request body is already fully buffered as `Bytes` before routing. Each attempt sends `body.clone()`, which is a cheap refcount bump. The retry ceiling is the chain length — each provider is tried at most once, no looping.

### Decision 5: Startup validation enforces chain consistency

`from_config()` validates each route's chain:
- The `providers` list MUST be non-empty.
- Every referenced provider id MUST exist (previously a missing provider produced a `warn` skip; now a route is skipped only if *all* of its providers are missing, and a chain with *some* missing providers is a hard error to avoid silent priority gaps).
- All providers in a chain MUST share the same `Protocol`; a mixed-protocol chain fails with `bail!`.

**Rationale:** A pass-through gateway cannot failover across protocols (no translation). Catching this at startup turns a confusing runtime 502 into a clear configuration error.

## Risks / Trade-offs

- **Breaking config change** → Existing `provider = "x"` configs stop parsing. Mitigation: documented in proposal as BREAKING; migration is mechanical (`provider = "x"` → `providers = ["x"]`).
- **Wasted latency on a slow-failing primary** → Each request pays the primary's timeout before trying the fallback, every time (no circuit breaker). Mitigation: accepted as a non-goal; transport timeout bounds the wait per attempt.
- **Misclassified 5xx** → A provider that returns 5xx for a client-caused condition would trigger unnecessary failover. Mitigation: acceptable — 5xx is by definition a server-side signal, and the fallback simply gets the same request.
- **Full chain exhausted** → If every provider fails, the client still gets `502 Bad Gateway`, same as today. Logs record each failed attempt for diagnosis.
