# llm-gateway Specification

## Purpose
TBD - created by archiving change add-llm-gateway-mvp. Update Purpose after archive.
## Requirements
### Requirement: Gateway authenticates client requests
The system SHALL require configured gateway bearer token authentication for provider pass-through endpoints.

#### Scenario: Request with valid gateway key is accepted
- **WHEN** a client sends a provider pass-through request with `Authorization: Bearer <configured-gateway-key>`
- **THEN** the gateway MUST continue request routing and proxy processing.

#### Scenario: Request without valid gateway key is rejected
- **WHEN** a client sends a provider pass-through request without a configured gateway bearer token
- **THEN** the gateway MUST reject the request before contacting any upstream provider.

### Requirement: Gateway separates upstream credentials from client credentials
The system SHALL use configured upstream provider credentials when forwarding requests and MUST NOT forward the client gateway credential to upstream providers.

#### Scenario: OpenAI-compatible request is forwarded with provider credential
- **WHEN** an authenticated client sends an OpenAI-compatible pass-through request
- **THEN** the gateway MUST send the configured OpenAI-compatible upstream credential to the matched provider.

#### Scenario: Anthropic-compatible request is forwarded with provider credential
- **WHEN** an authenticated client sends an Anthropic-compatible pass-through request
- **THEN** the gateway MUST send the configured Anthropic upstream credential and required Anthropic version header to the matched provider.

### Requirement: Gateway routes OpenAI-compatible requests by configured route rules
The system SHALL route OpenAI-compatible paths to configured OpenAI-compatible provider chains without changing the OpenAI-compatible request or response schema.

#### Scenario: Chat completions request matches OpenAI-compatible route
- **WHEN** an authenticated client sends `POST /v1/chat/completions` and a configured route matches that path
- **THEN** the gateway MUST forward the request body to the first available OpenAI-compatible provider in the matched route's chain.

#### Scenario: Responses request matches OpenAI-compatible route
- **WHEN** an authenticated client sends `POST /v1/responses` and a configured route matches that path
- **THEN** the gateway MUST forward the request body to the first available OpenAI-compatible provider in the matched route's chain.

#### Scenario: Models request matches OpenAI-compatible route
- **WHEN** an authenticated client sends `GET /v1/models` and a configured OpenAI-compatible route matches that path
- **THEN** the gateway MUST forward the request to the first available OpenAI-compatible provider in the matched route's chain.

### Requirement: Gateway routes Anthropic-compatible requests by configured route rules
The system SHALL route Anthropic-compatible paths to configured Anthropic-compatible provider chains without changing the Anthropic-compatible request or response schema.

#### Scenario: Messages request matches Anthropic-compatible route
- **WHEN** an authenticated client sends `POST /v1/messages` and a configured route matches that path
- **THEN** the gateway MUST forward the request body to the first available Anthropic-compatible provider in the matched route's chain.

#### Scenario: Models request matches Anthropic-compatible route
- **WHEN** an authenticated client sends `GET /v1/models` and a configured Anthropic-compatible route matches that path
- **THEN** the gateway MUST forward the request to the first available Anthropic-compatible provider in the matched route's chain.

### Requirement: Gateway does not convert between provider protocols
The system MUST preserve protocol-native request and response formats and MUST NOT translate OpenAI-compatible requests into Anthropic-compatible requests or Anthropic-compatible requests into OpenAI-compatible requests.

#### Scenario: OpenAI-compatible request never falls through to Anthropic conversion
- **WHEN** an authenticated OpenAI-compatible request has no matching OpenAI-compatible route
- **THEN** the gateway MUST return a route error instead of converting the request to Anthropic-compatible format.

#### Scenario: Anthropic-compatible request never falls through to OpenAI conversion
- **WHEN** an authenticated Anthropic-compatible request has no matching Anthropic-compatible route
- **THEN** the gateway MUST return a route error instead of converting the request to OpenAI-compatible format.

### Requirement: Gateway preserves provider-native streaming responses
The system SHALL proxy streaming responses from upstream providers without rewriting provider-native stream events.

#### Scenario: OpenAI-compatible streaming request returns native stream
- **WHEN** an authenticated OpenAI-compatible request asks the upstream provider for a streaming response
- **THEN** the gateway MUST stream the upstream response body to the client without rewriting SSE event payloads.

#### Scenario: Anthropic-compatible streaming request returns native stream
- **WHEN** an authenticated Anthropic-compatible request asks the upstream provider for a streaming response
- **THEN** the gateway MUST stream the upstream response body to the client without rewriting SSE event payloads.

### Requirement: Gateway exposes health endpoint
The system SHALL expose a gateway-owned health endpoint that does not require provider protocol routing.

#### Scenario: Health endpoint reports service availability
- **WHEN** a client sends `GET /healthz`
- **THEN** the gateway MUST return a successful response when the HTTP service is running.

### Requirement: Gateway records request metadata
The system SHALL record operational metadata for provider pass-through requests without logging prompt or completion bodies by default.

#### Scenario: Completed provider request records metadata
- **WHEN** a provider pass-through request completes
- **THEN** the gateway MUST record request id, provider id, protocol, path, response status, and latency.

#### Scenario: Prompt body is not logged by default
- **WHEN** a provider pass-through request includes prompt or message content
- **THEN** the gateway MUST NOT log the full request body by default.

### Requirement: Gateway routes support an ordered provider failover chain
The system SHALL allow a route to reference an ordered list of providers, where list order expresses priority, and SHALL attempt providers in that order until one returns a non-failover response or the chain is exhausted.

#### Scenario: Primary provider success skips fallback providers
- **WHEN** an authenticated request matches a route whose provider chain is `[primary, fallback]` and the primary returns a non-failover response
- **THEN** the gateway MUST forward the primary response to the client and MUST NOT contact the fallback provider.

#### Scenario: Primary transport failure falls over to next provider
- **WHEN** an authenticated request matches a route whose provider chain is `[primary, fallback]` and the primary request fails with a transport error
- **THEN** the gateway MUST retry the same request against the fallback provider and forward the fallback response.

#### Scenario: Primary 5xx response falls over to next provider
- **WHEN** an authenticated request matches a route whose provider chain is `[primary, fallback]` and the primary returns an HTTP 5xx status
- **THEN** the gateway MUST retry the same request against the fallback provider and forward the fallback response.

#### Scenario: Exhausted failover chain returns a gateway error
- **WHEN** an authenticated request matches a route and every provider in the chain fails with a transport error or HTTP 5xx status
- **THEN** the gateway MUST return a gateway error response after the last provider attempt.

### Requirement: Gateway limits failover to transport errors and server errors
The system SHALL trigger failover to the next provider ONLY on a transport error or an HTTP 5xx response, and MUST forward any HTTP 2xx, 3xx, or 4xx response (including HTTP 429) to the client without trying another provider.

#### Scenario: Client error response is not retried on another provider
- **WHEN** an authenticated request matches a route whose provider chain has more than one provider and the first provider returns an HTTP 4xx status
- **THEN** the gateway MUST forward that 4xx response to the client and MUST NOT contact any further provider.

#### Scenario: Rate limit response is not retried on another provider
- **WHEN** an authenticated request matches a route whose provider chain has more than one provider and the first provider returns HTTP 429
- **THEN** the gateway MUST forward the 429 response to the client and MUST NOT contact any further provider.

### Requirement: Gateway decides failover before forwarding any response body
The system SHALL determine whether to fail over based on the upstream response status before forwarding any response body bytes to the client, so that streaming responses are never interrupted by a provider switch.

#### Scenario: Streaming response is only forwarded after failover decision
- **WHEN** an authenticated request that expects a streaming response matches a route with a failover chain
- **THEN** the gateway MUST select the responding provider based on status before streaming body bytes, and once body bytes are forwarded MUST NOT switch providers.

### Requirement: Gateway validates failover chain consistency at startup
The system SHALL validate each route's provider chain at startup, requiring a non-empty chain of existing providers that all share one protocol.

#### Scenario: Route with a mixed-protocol chain is rejected
- **WHEN** a route's provider chain references providers configured with different protocols
- **THEN** the gateway MUST fail startup with a configuration error.

#### Scenario: Route with an empty provider chain is rejected
- **WHEN** a route is configured with an empty provider chain
- **THEN** the gateway MUST fail startup with a configuration error.

### Requirement: Gateway resolves provider-specific model aliases
The system SHALL allow each provider to define model aliases that map public client-facing model names to provider-specific upstream model names.

#### Scenario: Request model is rewritten for the selected provider
- **WHEN** an authenticated client sends a provider pass-through request whose top-level `model` value matches an alias configured for the selected provider
- **THEN** the gateway MUST forward the request to that provider with the top-level `model` value replaced by the configured upstream model name.

#### Scenario: Fallback provider receives its own mapped model
- **WHEN** an authenticated request matches a route with a failover chain and the gateway attempts a fallback provider after the primary fails
- **THEN** the gateway MUST resolve the original client-facing model name against the fallback provider's aliases before forwarding the fallback request.

### Requirement: Gateway preserves existing model pass-through when aliases are absent
The system SHALL preserve current provider pass-through behavior for providers that do not configure model aliases.

#### Scenario: Provider without aliases receives original model
- **WHEN** an authenticated client sends a provider pass-through request to a matched route and the selected provider has no model aliases configured
- **THEN** the gateway MUST forward the request body without changing the top-level `model` value.

### Requirement: Gateway does not expose unresolved upstream model names through alias-enabled providers
The system SHALL NOT forward a request to a provider with configured model aliases unless the request's top-level `model` value resolves through that provider's alias map.

#### Scenario: Provider without requested alias is skipped before upstream contact
- **WHEN** an authenticated request matches a route and a provider in the route chain has model aliases configured but does not define the requested model alias
- **THEN** the gateway MUST NOT contact that provider for the request.

#### Scenario: Later provider can serve requested alias
- **WHEN** an authenticated request matches a route whose first provider cannot resolve the requested model alias and a later provider can resolve it
- **THEN** the gateway MUST forward the request to the later provider with that provider's configured upstream model name.

#### Scenario: No provider can resolve requested alias
- **WHEN** an authenticated request matches a route but every alias-enabled provider in the route chain lacks the requested model alias
- **THEN** the gateway MUST return a gateway error without contacting those providers.

### Requirement: Gateway limits alias rewriting to protocol-native model fields
The system SHALL only rewrite the top-level string `model` field in JSON request bodies and MUST NOT otherwise transform protocol-native request or response schemas.

#### Scenario: Only top-level model field changes
- **WHEN** an authenticated request body contains a top-level string `model` value that resolves through the selected provider's aliases
- **THEN** the gateway MUST preserve all other request body fields unchanged when forwarding upstream.

#### Scenario: Responses remain provider-native
- **WHEN** an upstream provider returns a response to a request that used model alias resolution
- **THEN** the gateway MUST forward the provider-native response without rewriting response body model fields.

