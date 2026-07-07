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

### Requirement: Gateway records token usage and estimated cost
The system SHALL persist token usage and estimated cost for completed provider pass-through requests when usage database configuration is enabled.

#### Scenario: Successful non-streaming response records usage and cost
- **WHEN** an authenticated provider pass-through request receives a successful non-streaming upstream response with provider-native usage metadata and matching pricing configuration
- **THEN** the gateway MUST persist request id, provider id, protocol, path, response status, latency, requested model, upstream model, token counts, estimated cost, currency, and usage source.

#### Scenario: Usage metadata is missing
- **WHEN** a provider pass-through response does not expose token usage metadata
- **THEN** the gateway MUST persist request metadata with usage source unavailable and MUST NOT estimate cost.

#### Scenario: Pricing is missing
- **WHEN** token usage is available but no configured pricing entry matches the selected provider and upstream model
- **THEN** the gateway MUST persist token counts and MUST leave estimated cost unavailable.

### Requirement: Gateway estimates cost with cache-aware pricing
The system SHALL calculate estimated cost from configured provider/model pricing and MUST account for cached token classes separately from regular input tokens. The system MUST subtract from regular input tokens only cache token classes that the provider reports as a subset of input tokens, MUST NOT subtract cache token classes the provider reports independently of input tokens, and MUST NOT produce a negative estimated cost.

#### Scenario: OpenAI-compatible cached prompt tokens are priced separately
- **WHEN** OpenAI-compatible usage metadata includes cached prompt tokens
- **THEN** the gateway MUST subtract cached prompt tokens from regular input tokens before applying regular input pricing and MUST apply cached input pricing to cached prompt tokens when configured.

#### Scenario: Anthropic-compatible cache tokens are priced separately
- **WHEN** Anthropic-compatible usage metadata includes cache creation or cache read input tokens
- **THEN** the gateway MUST NOT subtract cache creation or cache read input tokens from regular input tokens, because the provider reports them independently of input tokens, and MUST apply cache write and cache read pricing to those token classes when configured.

#### Scenario: Cache usage lacks cache pricing
- **WHEN** usage metadata includes cache token counts but the matching pricing entry lacks the required cache price
- **THEN** the gateway MUST persist token counts and MUST leave estimated cost unavailable rather than charging cache tokens as regular input tokens.

#### Scenario: Cache read tokens exceed reported input tokens
- **WHEN** usage metadata reports cache read or cache creation tokens that exceed the reported input tokens
- **THEN** the gateway MUST estimate a non-negative cost and MUST NOT reduce regular input tokens below zero.

### Requirement: Gateway observes streaming usage without rewriting streams
The system SHALL observe provider-native streaming responses for usage metadata while forwarding stream bytes to clients unchanged. The system SHALL parse SSE events incrementally as bytes arrive rather than buffering the full response body, and SHALL merge usage fields reported across separate stream events into a single usage result.

#### Scenario: OpenAI-compatible stream exposes final usage
- **WHEN** an OpenAI-compatible streaming response includes final usage metadata
- **THEN** the gateway MUST persist token usage and estimated cost after the stream completes without rewriting SSE event payloads.

#### Scenario: Anthropic-compatible stream reports input and output tokens across events
- **WHEN** an Anthropic-compatible streaming response reports input tokens in a `message_start` event and output tokens in a `message_delta` event
- **THEN** the gateway MUST persist both the input tokens from the `message_start` usage and the output tokens from the `message_delta` usage after the stream completes without rewriting SSE event payloads.

#### Scenario: Usage fields split across events are merged
- **WHEN** a streaming response reports different token classes in separate SSE events
- **THEN** the gateway MUST merge the reported token classes into a single usage result rather than replacing earlier fields with only the latest event.

#### Scenario: Usage payload spans chunk boundaries
- **WHEN** a single SSE usage line is delivered across more than one stream chunk
- **THEN** the gateway MUST reassemble the line before parsing and MUST persist the usage it reports.

#### Scenario: Streaming usage cannot be parsed
- **WHEN** a streaming response is missing parseable usage metadata or usage parsing fails
- **THEN** the gateway MUST preserve the client stream and persist request metadata with usage source unavailable.

### Requirement: Gateway configures usage persistence and pricing externally
The system SHALL read usage database connectivity and provider/model pricing from gateway configuration.

#### Scenario: Usage database configuration is absent
- **WHEN** the gateway starts without usage database configuration
- **THEN** the gateway MUST start with usage persistence disabled and continue proxying provider requests.

#### Scenario: Usage database configuration is enabled
- **WHEN** the gateway starts with enabled usage database configuration
- **THEN** the gateway MUST initialize PostgreSQL persistence through SeaORM for usage records.

#### Scenario: Pricing configuration is loaded
- **WHEN** the gateway configuration contains pricing entries
- **THEN** the gateway MUST use those entries for cost estimation and MUST NOT rely on hard-coded provider model prices.

#### Scenario: Pricing falls back through explicit wildcards
- **WHEN** no exact pricing entry matches the selected provider id and upstream model
- **THEN** the gateway MUST look for pricing in this order: selected provider id with `*` model, `*` provider with upstream model, and `*` provider with `*` model.

#### Scenario: Example usage configuration is documented
- **WHEN** a user reviews the gateway configuration documentation or sample configuration
- **THEN** the system MUST include commented examples for PostgreSQL usage database settings and provider/model pricing fields.

### Requirement: Gateway protects request and response content in usage records
The system SHALL NOT persist prompt, message, or completion content as part of usage recording.

#### Scenario: Prompt body is excluded from persisted usage
- **WHEN** a provider pass-through request includes prompt or message content
- **THEN** persisted usage records MUST NOT include the full request body or prompt/message content.

#### Scenario: Completion body is excluded from persisted usage
- **WHEN** a provider pass-through response includes generated completion content
- **THEN** persisted usage records MUST NOT include the full response body or completion content.

### Requirement: Gateway keeps usage failures out of the proxy response path
The system SHALL NOT fail or alter client responses because usage parsing, cost calculation, or persistence fails.

#### Scenario: Usage persistence write fails
- **WHEN** the gateway cannot write a usage record after receiving an upstream response
- **THEN** the gateway MUST log the usage persistence failure and MUST still forward the upstream response to the client.

#### Scenario: Usage parsing fails
- **WHEN** provider usage metadata cannot be parsed
- **THEN** the gateway MUST record usage source unavailable when possible and MUST still forward the upstream response to the client.

### Requirement: Gateway reserves the /admin path prefix
The system SHALL treat `/admin` and all subpaths as gateway-owned path space — served by the admin console when enabled, `404 Not Found` when disabled — and MUST NOT forward requests under `/admin` to any upstream provider.

#### Scenario: Admin path is never proxied
- **WHEN** a client requests any path under `/admin`, regardless of configured routes or authentication
- **THEN** the gateway MUST handle the request itself and MUST NOT contact any upstream provider.

#### Scenario: Non-admin paths are unaffected
- **WHEN** a client requests a path outside `/admin`, `/healthz`, and `/reload` that matches a configured route
- **THEN** the gateway MUST proxy it to the route's provider chain exactly as before this change.

