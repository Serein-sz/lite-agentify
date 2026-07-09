## ADDED Requirements

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

### Requirement: Gateway flushes pending usage on graceful shutdown
The system SHALL support graceful shutdown: on receiving a shutdown signal it MUST stop accepting new connections, flush any pending buffered usage records to the database, and then exit.

#### Scenario: Pending usage is flushed on shutdown
- **WHEN** the gateway receives a shutdown signal with usage records still buffered for background persistence
- **THEN** the gateway MUST attempt to flush the pending records before the process exits.

#### Scenario: Shutdown with persistence disabled
- **WHEN** the gateway receives a shutdown signal and usage persistence is disabled
- **THEN** the gateway MUST shut down cleanly without error.

## MODIFIED Requirements

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

## REMOVED Requirements

### Requirement: Gateway limits failover to transport errors and server errors
**Reason:** Superseded by "Gateway retries rate-limited providers with bounded backoff before failing over". The prior requirement forwarded HTTP 429 straight to the client and never retried; the new behavior retries configured rate-limit statuses (429/529) on the same provider with bounded backoff before advancing the chain. Transport-error and 5xx failover behavior is preserved under the new requirement.

**Migration:** No config migration required — retry defaults apply automatically. Clients that previously received an immediate 429 will now receive it only after the gateway exhausts bounded retries; this is a behavioral change, not an API change.
