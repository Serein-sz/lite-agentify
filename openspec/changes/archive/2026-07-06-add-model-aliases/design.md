## Context

The gateway currently authenticates client requests, matches a route by path and optional `model_prefix`, then forwards the original request body to each provider in the route's ordered failover chain. The `model` field is only inspected for route matching in `GatewayState::match_route`; it is not rewritten before forwarding.

This means client configuration must use upstream model identifiers directly. That exposes provider choices and makes failover across providers awkward when equivalent models have different names.

## Goals / Non-Goals

**Goals:**

- Allow stable public model names at the gateway boundary.
- Resolve public model names to provider-specific upstream model names.
- Support different mappings per provider in the same failover chain.
- Keep existing behavior unchanged when no aliases are configured.
- Preserve protocol-native request and response shapes except for rewriting the top-level request `model` string.

**Non-Goals:**

- Do not translate between OpenAI-compatible and Anthropic-compatible schemas.
- Do not synthesize or rewrite `/v1/models` or other model-list responses in this change.
- Do not introduce dynamic model discovery or remote configuration.
- Do not support aliasing nested payload fields or response bodies.

## Decisions

### Provider-scoped alias maps

Add an optional `model_aliases` map to each provider configuration and runtime provider model. The map keys are public model names accepted from clients, and values are upstream model names sent to that provider.

Alternative considered: a route-level alias map. This is simpler, but it cannot express that the same public model should map to `gpt-4o-mini` for one provider and `deepseek-chat` for another provider in a fallback chain.

Alternative considered: a global alias table. This centralizes public model naming but requires a larger routing model and is more than the current gateway needs.

### Rewrite inside the provider attempt loop

Perform alias resolution after route matching and before each upstream request is sent. Each provider attempt gets its own rewritten body, so fallback providers can receive different upstream model names for the same original client request.

Route matching continues to use the client's public model value. This keeps route rules stable and lets `model_prefix` operate on external names.

### Alias maps are restrictive when configured

If a provider defines aliases and the incoming body has a `model` value that is not present in that provider's alias map, the gateway should not contact that provider for the request. It should continue to the next provider in the chain, and if no provider can resolve the model, return a gateway error.

This preserves implementation hiding: once a provider opts into aliases, clients cannot bypass the public model surface by sending raw upstream names to that provider.

### Preserve pass-through behavior without aliases

Providers without `model_aliases` keep existing pass-through behavior. This avoids breaking existing deployments and allows incremental adoption.

## Risks / Trade-offs

- Unsupported public model can skip a healthy primary provider → mitigate with tests covering provider-specific aliases and exhausted alias resolution.
- Rewriting JSON bodies requires parsing request bodies that are currently opaque → mitigate by only rewriting non-empty JSON objects with a string top-level `model` field and returning a clear gateway error on rewrite failure.
- Existing "no schema conversion" language could be read as forbidding any rewrite → mitigate by specifying alias rewriting as a narrow, protocol-native change to the request's top-level `model` field only.
- `/v1/models` may still expose upstream model names because responses remain pass-through → document as a non-goal and leave public model listing for a future change.
