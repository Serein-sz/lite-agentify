## Why

Clients currently need to know the concrete upstream model names configured behind the gateway. This leaks provider implementation details and makes it harder to switch providers, add failover, or standardize client configuration around stable public model names.

## What Changes

- Add configurable model aliases so clients can send a stable public model name while the gateway forwards the provider-specific upstream model name.
- Support provider-specific alias mappings so the same public model can resolve to different upstream model names across a failover chain.
- Preserve existing routing, authentication, protocol boundaries, and failover semantics.
- Return a clear gateway error when a request uses an unknown aliased model for a provider that requires alias resolution.
- No breaking changes to existing configurations that do not define model aliases.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `llm-gateway`: Adds provider-specific model alias resolution and request model rewriting before upstream forwarding.

## Impact

- Affected code: gateway configuration parsing, runtime provider model, proxy request body handling, and route/failover tests.
- Affected APIs: provider configuration gains an optional model alias map; client request schema remains protocol-native.
- Dependencies: no new external service dependency expected.
