## Why

Applications need a single controlled egress point for LLM calls while preserving native provider protocol behavior. This change introduces an MVP LLM gateway that supports OpenAI-compatible and Anthropic-compatible APIs as protocol-specific pass-through routes, avoiding premature schema unification or cross-protocol conversion.

## What Changes

- Add a configuration-driven LLM gateway capability for declaring providers and routes.
- Support OpenAI-compatible pass-through endpoints such as chat completions, responses, and model listing.
- Support Anthropic-compatible pass-through endpoints such as messages and model listing where supported by the upstream provider.
- Add gateway-level bearer token authentication that is separate from upstream provider credentials.
- Preserve streaming responses by proxying provider-native SSE/event payloads without protocol conversion.
- Add basic gateway-owned health and observability endpoints for readiness, request metadata logging, and operational checks.
- Exclude protocol conversion, provider fallback, billing, prompt storage, management UI, and complex multi-tenant policy from the MVP.

## Capabilities

### New Capabilities

- `llm-gateway`: Defines provider configuration, request routing, protocol pass-through behavior, gateway authentication, streaming proxy behavior, and basic operational visibility for the MVP LLM gateway.

### Modified Capabilities

None.

## Impact

- Adds new HTTP gateway APIs for OpenAI-compatible, Anthropic-compatible, and gateway-owned health/observability endpoints.
- Adds configuration for providers, upstream credentials, route matching, and gateway API keys.
- Introduces outbound HTTP proxy behavior for JSON and streaming LLM requests.
- May add dependencies for HTTP serving, HTTP client proxying, configuration loading, structured logging, and metrics.
