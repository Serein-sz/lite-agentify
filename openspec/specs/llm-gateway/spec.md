# llm-gateway Specification

## Purpose
TBD - created by archiving change add-llm-gateway-mvp. Update Purpose after archive.
## Requirements
### Requirement: Gateway authenticates client requests
The system SHALL require API key authentication for provider pass-through endpoints, resolving the presented bearer token against database-backed API keys via an in-process snapshot keyed by SHA-256 hash. Requests presenting a key that is revoked, or whose owning user is disabled, MUST be rejected.

#### Scenario: Request with valid gateway key is accepted
- **WHEN** a client sends a provider pass-through request with `Authorization: Bearer <active-api-key>` belonging to an active user
- **THEN** the gateway MUST continue request routing and proxy processing, attributing the request to the key's owning user.

#### Scenario: Request without valid gateway key is rejected
- **WHEN** a client sends a provider pass-through request without a valid API key, or with a revoked key, or with a key whose owning user is disabled
- **THEN** the gateway MUST reject the request before contacting any upstream provider.

### Requirement: Gateway separates upstream credentials from client credentials
The system SHALL use configured upstream provider credentials when forwarding requests and MUST NOT forward the client gateway credential to upstream providers.

#### Scenario: OpenAI-compatible request is forwarded with provider credential
- **WHEN** an authenticated client sends an OpenAI-compatible pass-through request
- **THEN** the gateway MUST send the configured OpenAI-compatible upstream credential to the matched provider.

#### Scenario: Anthropic-compatible request is forwarded with provider credential
- **WHEN** an authenticated client sends an Anthropic-compatible pass-through request
- **THEN** the gateway MUST send the configured Anthropic upstream credential and required Anthropic version header to the matched provider.

### Requirement: Gateway does not convert between provider protocols
The system MUST preserve protocol-native request and response formats and MUST NOT translate OpenAI-compatible requests into Anthropic-compatible requests or Anthropic-compatible requests into OpenAI-compatible requests. Deployment chains are filtered to the request endpoint's protocol family rather than translated.

#### Scenario: OpenAI-compatible request never falls through to Anthropic conversion
- **WHEN** an authenticated OpenAI-compatible request resolves to a model whose only deployments are Anthropic-protocol providers
- **THEN** the gateway MUST return a resolution error instead of converting the request to Anthropic-compatible format.

#### Scenario: Anthropic-compatible request never falls through to OpenAI conversion
- **WHEN** an authenticated Anthropic-compatible request resolves to a model whose only deployments are OpenAI-protocol providers
- **THEN** the gateway MUST return a resolution error instead of converting the request to OpenAI-compatible format.

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
The system SHALL record operational metadata for provider pass-through requests without logging prompt or completion bodies by default, including the authenticated user id and API key id for every proxied request.

#### Scenario: Completed provider request records metadata
- **WHEN** a provider pass-through request completes
- **THEN** the gateway MUST record request id, provider id, protocol, path, response status, latency, and the user id and API key id that made the request.

#### Scenario: Prompt body is not logged by default
- **WHEN** a provider pass-through request includes prompt or message content
- **THEN** the gateway MUST NOT log the full request body by default.

### Requirement: Gateway routes support an ordered provider failover chain
The system SHALL attempt a resolved model's protocol-filtered deployments in catalog order, where order expresses priority, until one returns a non-failover response or the chain is exhausted.

#### Scenario: Primary provider success skips fallback providers
- **WHEN** an authenticated request resolves to a deployment chain `[primary, fallback]` and the primary returns a non-failover response
- **THEN** the gateway MUST forward the primary response to the client and MUST NOT contact the fallback provider.

#### Scenario: Primary transport failure falls over to next provider
- **WHEN** an authenticated request resolves to a deployment chain `[primary, fallback]` and the primary request fails with a transport error
- **THEN** the gateway MUST retry the same request against the fallback deployment and forward the fallback response.

#### Scenario: Primary 5xx response falls over to next provider
- **WHEN** an authenticated request resolves to a deployment chain `[primary, fallback]` and the primary returns an HTTP 5xx status
- **THEN** the gateway MUST retry the same request against the fallback deployment and forward the fallback response.

#### Scenario: Exhausted failover chain returns a gateway error
- **WHEN** every deployment in the resolved chain fails with a transport error or HTTP 5xx status
- **THEN** the gateway MUST return a gateway error response after the last attempt.

### Requirement: Gateway retries rate-limited providers with bounded backoff before failing over
The system SHALL treat a configurable set of upstream statuses (default HTTP 429 and 529) as retryable, and on such a response SHALL wait a bounded backoff delay and retry the **same** provider up to a configured maximum number of attempts before advancing to the next provider in the failover chain. The retryable-status check takes precedence, so a status that is both a server error and in the retryable set (e.g. HTTP 529) is retried rather than failed over. All other HTTP 2xx, 3xx, and non-retryable 4xx responses MUST be forwarded to the client immediately without retry or failover. Transport errors and non-retryable HTTP 5xx responses MUST fail over to the next provider immediately without same-provider retry.

The backoff delay MUST be bounded by a configured maximum. When the upstream response includes a `Retry-After` header, the system SHALL respect it but MUST cap the wait at the configured maximum so an upstream cannot force an unbounded wait. Backoff MUST apply full jitter across concurrent requests.

#### Scenario: Retryable status is retried on the same provider before failover
- **WHEN** an authenticated request matches a route whose provider chain is `[primary, fallback]` and the primary returns a retryable status (e.g. HTTP 429)
- **THEN** the gateway MUST wait a bounded backoff delay and retry the same primary provider, and MUST advance to the fallback provider only after the configured maximum retry attempts on the primary are exhausted.

#### Scenario: Retry succeeds on the same provider
- **WHEN** a provider returns a retryable status on the first attempt and a success (HTTP 2xx) on a subsequent retry attempt
- **THEN** the gateway MUST forward the successful response to the client and MUST NOT contact any further provider.

#### Scenario: Single-provider chain retries then returns the last retryable response
- **WHEN** an authenticated request matches a route with a single-provider chain and that provider returns a retryable status on every attempt up to the configured maximum
- **THEN** the gateway MUST forward the last retryable response to the client after exhausting retries.

#### Scenario: Retry-After is honored but capped
- **WHEN** a retryable response includes a `Retry-After` value larger than the configured maximum backoff
- **THEN** the gateway MUST wait no longer than the configured maximum backoff before retrying.

#### Scenario: Non-retryable server error still fails over immediately without same-provider retry
- **WHEN** an authenticated request matches a route whose provider chain is `[primary, fallback]` and the primary returns a non-retryable HTTP 5xx status (e.g. HTTP 500)
- **THEN** the gateway MUST advance to the fallback provider immediately and MUST NOT retry the primary provider.

### Requirement: Gateway configures retry behavior externally
The system SHALL read retry behavior from an optional `[retry]` configuration section, and this section SHALL be hot-reloadable alongside other hot-reloadable fields. When the section is absent the gateway MUST apply built-in defaults.

#### Scenario: Retry configuration is absent
- **WHEN** the gateway starts or reloads without a `[retry]` configuration section
- **THEN** the gateway MUST apply default retry behavior (default retryable statuses, attempt count, and backoff bounds) and continue serving.

#### Scenario: Retry configuration is provided
- **WHEN** the gateway configuration contains a `[retry]` section specifying retryable statuses, maximum attempts, or backoff bounds
- **THEN** the gateway MUST use those values for subsequent requests without requiring a restart.

### Requirement: Gateway decides failover before forwarding any response body
The system SHALL determine whether to fail over based on the upstream response status before forwarding any response body bytes to the client, so that streaming responses are never interrupted by a provider switch.

#### Scenario: Streaming response is only forwarded after failover decision
- **WHEN** an authenticated request that expects a streaming response matches a route with a failover chain
- **THEN** the gateway MUST select the responding provider based on status before streaming body bytes, and once body bytes are forwarded MUST NOT switch providers.

### Requirement: Gateway limits alias rewriting to protocol-native model fields
The system SHALL only rewrite the top-level string `model` field in JSON request bodies — replacing the public catalog name with the attempted deployment's upstream model name — and MUST NOT otherwise transform protocol-native request or response schemas.

#### Scenario: Only top-level model field changes
- **WHEN** an authenticated request body contains a top-level string `model` value resolved through the catalog
- **THEN** the gateway MUST preserve all other request body fields unchanged when forwarding upstream.

#### Scenario: Responses remain provider-native
- **WHEN** an upstream provider returns a response to a request whose model was rewritten to the deployment's upstream name
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
The system SHALL read database connectivity from gateway configuration (the mandatory `database` section) and provider/model pricing from the database's pricing rules. The gateway MUST NOT rely on hard-coded provider model prices.

#### Scenario: Database configuration is required
- **WHEN** the gateway starts
- **THEN** it MUST initialize PostgreSQL persistence through SeaORM using the configured `database` section, and MUST fail startup when it is absent or unreachable.

#### Scenario: Pricing rules are loaded from the database
- **WHEN** the gateway builds its snapshot and pricing rules exist in the database
- **THEN** it MUST use those rules for cost estimation and MUST NOT rely on hard-coded provider model prices.

#### Scenario: Pricing falls back through explicit wildcards
- **WHEN** no exact pricing rule matches the selected provider id and upstream model
- **THEN** the gateway MUST look for pricing in this order: selected provider id with `*` model, `*` provider with upstream model, and `*` provider with `*` model.

#### Scenario: Example configuration is documented
- **WHEN** a user reviews the gateway configuration documentation or sample configuration
- **THEN** the system MUST include examples for the PostgreSQL `database` section and document that pricing is managed through the admin console or API.

### Requirement: Gateway protects request and response content in usage records
The system SHALL NOT persist prompt, message, or completion content as part of usage recording.

#### Scenario: Prompt body is excluded from persisted usage
- **WHEN** a provider pass-through request includes prompt or message content
- **THEN** persisted usage records MUST NOT include the full request body or prompt/message content.

#### Scenario: Completion body is excluded from persisted usage
- **WHEN** a provider pass-through response includes generated completion content
- **THEN** persisted usage records MUST NOT include the full response body or completion content.

### Requirement: Gateway keeps usage failures out of the proxy response path
The system SHALL NOT fail, alter, or delay client responses because usage parsing, cost calculation, or persistence fails. Usage records MUST be handed to a background writer that persists them asynchronously; the proxy response path MUST NOT await a usage database write.

#### Scenario: Usage record is persisted asynchronously
- **WHEN** the gateway produces a usage record for a proxied request
- **THEN** the gateway MUST enqueue the record for background persistence and MUST return the upstream response to the client without awaiting the database write.

#### Scenario: Usage buffer is saturated
- **WHEN** the gateway produces a usage record but the background persistence buffer is full
- **THEN** the gateway MUST drop the record with a logged warning and MUST still forward the upstream response to the client.

#### Scenario: Usage persistence write fails
- **WHEN** the background writer cannot persist a usage record or batch
- **THEN** the gateway MUST log the usage persistence failure and MUST NOT affect any client response.

#### Scenario: Usage parsing fails
- **WHEN** provider usage metadata cannot be parsed
- **THEN** the gateway MUST record usage source unavailable when possible and MUST still forward the upstream response to the client.

### Requirement: Gateway flushes pending usage on graceful shutdown
The system SHALL support graceful shutdown: on receiving a shutdown signal it MUST stop accepting new connections, flush any pending buffered usage records to the database, and then exit.

#### Scenario: Pending usage is flushed on shutdown
- **WHEN** the gateway receives a shutdown signal with usage records still buffered for background persistence
- **THEN** the gateway MUST attempt to flush the pending records before the process exits.

#### Scenario: Shutdown with persistence disabled
- **WHEN** the gateway receives a shutdown signal and usage persistence is disabled
- **THEN** the gateway MUST shut down cleanly without error.

### Requirement: Gateway reserves the /admin path prefix
The system SHALL treat `/admin` and all subpaths as gateway-owned path space — served by the admin console when enabled, `404 Not Found` when disabled — and MUST NOT forward requests under `/admin` to any upstream provider.

#### Scenario: Admin path is never proxied
- **WHEN** a client requests any path under `/admin`, regardless of configured routes or authentication
- **THEN** the gateway MUST handle the request itself and MUST NOT contact any upstream provider.

#### Scenario: Non-admin paths are unaffected
- **WHEN** a client requests a path outside `/admin`, `/healthz`, and `/reload` that matches a configured route
- **THEN** the gateway MUST proxy it to the route's provider chain exactly as before this change.

