## 1. Configuration

- [x] 1.1 Define gateway configuration structs for listen address, gateway keys, providers, routes, and protocol-specific provider options.
- [x] 1.2 Implement configuration loading from `~/.config/lite-agentify/llm-gateway.toml` with direct upstream credential values.
- [x] 1.3 Add validation for missing gateway keys, duplicate provider ids, unsupported protocols, invalid upstream base URLs, and routes that reference unknown providers.

## 2. Routing and Authentication

- [x] 2.1 Create a provider model that distinguishes OpenAI-compatible and Anthropic-compatible protocols without introducing a shared chat schema.
- [x] 2.2 Implement route matching by request path and optional model prefix while enforcing protocol compatibility.
- [x] 2.3 Implement gateway bearer token validation for provider pass-through endpoints.
- [x] 2.4 Ensure unauthenticated provider requests are rejected before any upstream request is made.
- [x] 2.5 Ensure unmatched protocol routes return a route error instead of attempting cross-protocol conversion.

## 3. Upstream Proxy

- [x] 3.1 Implement request forwarding that preserves method, path, query string, and body for matched upstream providers.
- [x] 3.2 Replace client gateway credentials with configured provider credentials on outbound requests.
- [x] 3.3 Add Anthropic-specific outbound headers, including the configured Anthropic version header.
- [x] 3.4 Preserve safe client headers needed for content negotiation while excluding hop-by-hop and credential-leaking headers.
- [x] 3.5 Proxy non-streaming JSON responses with upstream status and response body preserved.
- [x] 3.6 Proxy streaming responses without rewriting provider-native SSE event payloads.

## 4. Gateway Endpoints and Observability

- [x] 4.1 Add `GET /healthz` as a gateway-owned health endpoint.
- [x] 4.2 Add request id assignment or propagation for provider pass-through requests.
- [x] 4.3 Log provider request metadata including request id, provider id, protocol, path, response status, and latency.
- [x] 4.4 Avoid logging full prompt, message, completion, or response bodies by default.

## 5. Integration with Existing App

- [x] 5.1 Refactor the current Axum server setup into gateway-focused modules while preserving GPUI application startup.
- [x] 5.2 Replace the demo root route with the configured gateway router and health endpoint.
- [x] 5.3 Document an MVP configuration in `~/.config/lite-agentify/llm-gateway.toml` for one OpenAI-compatible provider and one Anthropic-compatible provider.

## 6. Verification

- [x] 6.1 Add unit tests for configuration validation and configured credential use.
- [x] 6.2 Add unit tests for route matching, protocol compatibility, and no-conversion route errors.
- [x] 6.3 Add handler/proxy tests proving valid gateway keys are accepted and missing or invalid keys are rejected before upstream contact.
- [x] 6.4 Add proxy tests proving outbound OpenAI-compatible and Anthropic-compatible credential headers are attached correctly.
- [x] 6.5 Add streaming proxy test proving SSE payload bytes are forwarded without rewriting.
- [x] 6.6 Run `cargo check` and the relevant test suite.
