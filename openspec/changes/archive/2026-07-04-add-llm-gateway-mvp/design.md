## Context

The repository is currently a single Rust crate with a GPUI application and a small Axum HTTP server. The MVP LLM gateway can build on the existing Axum/Tokio stack and add focused HTTP proxy behavior without introducing a separate service, workspace split, or protocol abstraction layer.

The gateway must support OpenAI-compatible and Anthropic-compatible clients without converting between their request or response formats. Its value is a controlled, observable, configurable egress point for LLM traffic: gateway authentication, route selection, upstream credential isolation, native streaming pass-through, and basic health/operational visibility.

## Goals / Non-Goals

**Goals:**

- Provide OpenAI-compatible and Anthropic-compatible HTTP pass-through routes.
- Keep OpenAI and Anthropic protocol handling separate and explicit.
- Configure providers, upstream base URLs, upstream credentials, gateway keys, and route rules from `~/.config/lite-agentify` outside request handlers.
- Support normal JSON responses and provider-native streaming responses.
- Log request metadata useful for operations without storing prompt or completion content by default.
- Expose simple gateway-owned health endpoints.

**Non-Goals:**

- No OpenAI-to-Anthropic or Anthropic-to-OpenAI conversion.
- No generic chat/message schema shared across providers.
- No provider fallback, load balancing, retries, model remapping, prompt cache, billing, budget tracking, or management UI in the MVP.
- No durable request storage or full prompt/completion audit log.
- No complex multi-tenant permissions beyond gateway API key validation.

## Decisions

1. Use route-level protocol pass-through rather than a unified model abstraction.

   Requests will match configured gateway paths and forward to a provider whose declared protocol matches that path family. The proxy will preserve request body and upstream response body, including streaming event payloads. This avoids early loss of provider-specific semantics such as Anthropic message metadata or OpenAI-compatible response fields.

   Alternative considered: normalize all requests into an internal `GenericChatRequest`. Rejected for MVP because it creates hidden conversion requirements and conflicts with the explicit goal of not translating protocols.

2. Use static configuration as the source of truth for providers and routes.

   The MVP should load provider definitions, route rules, listen address, gateway API keys, and upstream provider credentials directly from `~/.config/lite-agentify/llm-gateway.toml` by default on every platform. Request handlers consume resolved runtime state rather than hard-coded providers.

   Alternative considered: hard-code OpenAI and Anthropic upstreams directly in handlers. Rejected because it would make adding OpenAI-compatible providers harder and would mix routing policy with transport code.

3. Replace only gateway/upstream authentication headers during proxying.

   The gateway validates client `Authorization: Bearer <gateway-key>`, removes gateway credentials from the outbound request, and attaches the provider-specific upstream authorization headers. Anthropic requests also attach the configured protocol version header when required.

   Alternative considered: forward all incoming headers unchanged. Rejected because it could leak gateway credentials upstream and makes provider credential isolation unreliable.

4. Treat streaming as byte/event pass-through.

   The gateway should detect or preserve streaming requests and forward the upstream response as a streaming body with provider-native headers where safe. It must not parse, merge, or rewrite SSE event payloads for MVP behavior.

   Alternative considered: parse stream chunks for usage or logging. Rejected for MVP because it increases protocol coupling and can break client compatibility.

5. Keep observability metadata-only by default.

   Logs should include request id, matched route, provider id, protocol, model if discoverable from JSON request metadata, response status, latency, and error class. Prompt/completion bodies are not logged by default.

   Alternative considered: store full request/response payloads for debugging. Rejected because it creates privacy and security risk beyond the MVP need.

## Risks / Trade-offs

- Provider-specific header requirements may drift over time -> Keep provider header behavior isolated behind protocol-specific request decoration and document supported MVP headers.
- Pure pass-through means clients must use the correct protocol path for the selected provider -> Return clear 404/400 errors when no route matches rather than silently converting.
- Static configuration requires restart for route/provider changes -> Accept for MVP; later changes can introduce reload or admin APIs.
- Streaming pass-through can make usage extraction incomplete -> Prefer compatibility over metrics richness; record usage only when available without parsing stream payloads.
- Running the gateway inside the same executable as the GPUI app may blur product boundaries -> Keep gateway code in separate modules so it can be split later if needed.

## Migration Plan

1. Add configuration structures and loading for gateway settings.
2. Add provider and route matching modules.
3. Replace the current demo HTTP route with gateway-owned health endpoints and provider pass-through routes.
4. Add proxy transport for JSON and streaming bodies.
5. Add focused tests for auth, route matching, header decoration, no-conversion behavior, and health endpoints.

Rollback is straightforward during MVP development: disable gateway startup or restore the previous minimal Axum router if gateway configuration fails.

## Open Questions

- Should the MVP expose `GET /v1/models` for Anthropic only when the configured upstream supports it, or return a gateway-level unsupported response?
- Should later versions support secret references or external secret stores in addition to direct configuration values?
- Should the desktop GPUI window remain active by default when the gateway is used as a server, or should there be a server-only mode?
