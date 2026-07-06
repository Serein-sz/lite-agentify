## ADDED Requirements

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
The system SHALL calculate estimated cost from configured provider/model pricing and MUST account for cached token classes separately from regular input tokens.

#### Scenario: OpenAI-compatible cached prompt tokens are priced separately
- **WHEN** OpenAI-compatible usage metadata includes cached prompt tokens
- **THEN** the gateway MUST subtract cached prompt tokens from regular input tokens before applying regular input pricing and MUST apply cached input pricing to cached prompt tokens when configured.

#### Scenario: Anthropic-compatible cache tokens are priced separately
- **WHEN** Anthropic-compatible usage metadata includes cache creation or cache read input tokens
- **THEN** the gateway MUST subtract cache creation and cache read input tokens from regular input tokens before applying regular input pricing and MUST apply cache write and cache read pricing to those token classes when configured.

#### Scenario: Cache usage lacks cache pricing
- **WHEN** usage metadata includes cache token counts but the matching pricing entry lacks the required cache price
- **THEN** the gateway MUST persist token counts and MUST leave estimated cost unavailable rather than charging cache tokens as regular input tokens.

### Requirement: Gateway observes streaming usage without rewriting streams
The system SHALL observe provider-native streaming responses for usage metadata while forwarding stream bytes to clients unchanged.

#### Scenario: OpenAI-compatible stream exposes final usage
- **WHEN** an OpenAI-compatible streaming response includes final usage metadata
- **THEN** the gateway MUST persist token usage and estimated cost after the stream completes without rewriting SSE event payloads.

#### Scenario: Anthropic-compatible stream exposes usage events
- **WHEN** an Anthropic-compatible streaming response includes provider-native usage metadata
- **THEN** the gateway MUST persist token usage and estimated cost after the stream completes without rewriting SSE event payloads.

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
