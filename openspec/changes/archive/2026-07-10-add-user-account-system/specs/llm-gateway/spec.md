# llm-gateway Specification (delta)

## MODIFIED Requirements

### Requirement: Gateway authenticates client requests
The system SHALL require API key authentication for provider pass-through endpoints, resolving the presented bearer token against database-backed API keys via an in-process snapshot keyed by SHA-256 hash. Requests presenting a key that is revoked, or whose owning user is disabled, MUST be rejected.

#### Scenario: Request with valid gateway key is accepted
- **WHEN** a client sends a provider pass-through request with `Authorization: Bearer <active-api-key>` belonging to an active user
- **THEN** the gateway MUST continue request routing and proxy processing, attributing the request to the key's owning user.

#### Scenario: Request without valid gateway key is rejected
- **WHEN** a client sends a provider pass-through request without a valid API key, or with a revoked key, or with a key whose owning user is disabled
- **THEN** the gateway MUST reject the request before contacting any upstream provider.

### Requirement: Gateway records request metadata
The system SHALL record operational metadata for provider pass-through requests without logging prompt or completion bodies by default, including the authenticated user id and API key id for every proxied request.

#### Scenario: Completed provider request records metadata
- **WHEN** a provider pass-through request completes
- **THEN** the gateway MUST record request id, provider id, protocol, path, response status, latency, and the user id and API key id that made the request.

#### Scenario: Prompt body is not logged by default
- **WHEN** a provider pass-through request includes prompt or message content
- **THEN** the gateway MUST NOT log the full request body by default.
