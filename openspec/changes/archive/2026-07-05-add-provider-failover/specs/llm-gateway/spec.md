## ADDED Requirements

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

## MODIFIED Requirements

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
