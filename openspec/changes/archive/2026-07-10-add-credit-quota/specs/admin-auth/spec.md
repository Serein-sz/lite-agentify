# admin-auth Specification (delta)

## ADDED Requirements

### Requirement: Sessions persist across restarts when Redis is configured
The system SHALL store sessions in Redis with their time-to-live when Redis is configured, so established sessions survive gateway process restarts; without Redis, sessions remain in-memory and restart behavior is unchanged. Login lockout state SHALL use the same backend selection.

#### Scenario: Session survives gateway restart with Redis
- **WHEN** the gateway restarts while Redis is configured and a client presents a session cookie issued before the restart and still within its TTL
- **THEN** the request MUST be authorized without a new login.

#### Scenario: In-memory sessions still die on restart
- **WHEN** the gateway restarts without Redis configured
- **THEN** previously issued session cookies MUST be rejected with `401`, as before.
