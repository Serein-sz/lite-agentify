# redis-hot-state Specification

## Purpose
TBD - created by syncing change add-credit-quota. Update Purpose after archive.

## Requirements
### Requirement: Redis is an optional hot-state backend
The system SHALL accept an optional `[redis]` config section with a connection URL treated as a secret (masked in config reads, revealable via the single-secret endpoint, restart-only like `database`). When configured, spend counters, sessions, and login-lockout state SHALL be stored in Redis; when absent, all three SHALL use the in-process implementations with identical externally observable semantics apart from restart persistence.

#### Scenario: Gateway runs identically without Redis
- **WHEN** the gateway runs without a `[redis]` section
- **THEN** quota enforcement, sessions, and lockout MUST function with in-memory state and no Redis connection attempts.

#### Scenario: Redis URL is masked
- **WHEN** an authenticated admin reads the config
- **THEN** the `redis.url` value MUST be masked like `database.url`.

### Requirement: Spend counters use Redis atomic operations when configured
The system SHALL, when Redis is configured, keep per-user and per-key spend counters in Redis using atomic increments, so counter state survives gateway restarts and is shared by all gateway processes pointed at the same Redis.

#### Scenario: Counters survive a gateway restart
- **WHEN** the gateway restarts with Redis configured
- **THEN** spend counters MUST retain their pre-restart values (subject to normal reconciliation) rather than restarting from zero before the seed completes.

### Requirement: Redis unavailability degrades without failing requests
The system SHALL treat Redis outages as degradation, never as request failure: counter operations fall back to an in-memory shadow (seeded from the last known values) with a logged warning, a background probe reconnects and re-seeds from PostgreSQL-derived truth, and session lookups that cannot reach Redis are treated as unauthenticated (`401`) rather than erroring. Proxied requests MUST NOT fail or block because Redis is unreachable.

#### Scenario: Proxying continues through a Redis outage
- **WHEN** Redis becomes unreachable while the gateway is serving
- **THEN** proxied requests MUST continue to be served with quota enforced against the in-memory shadow, and a warning MUST be logged.

#### Scenario: Recovery re-seeds Redis
- **WHEN** Redis becomes reachable again after an outage
- **THEN** the gateway MUST re-seed the Redis counters from reconciled truth and resume using Redis.

#### Scenario: Session check during outage fails closed
- **WHEN** a console request presents a session cookie while Redis is unreachable
- **THEN** the gateway MUST respond `401` rather than granting access or returning a server error.

### Requirement: A configuration-refresh channel is reserved for multi-instance use
The system SHALL publish a notification to a documented Redis channel after snapshot-affecting database mutations when Redis is configured, and SHALL subscribe to that channel and treat received notifications as a snapshot-rebuild trigger. Single-instance behavior is self-notification; no cross-instance coordination beyond this channel is implemented.

#### Scenario: Mutation publishes a refresh notification
- **WHEN** an admin mutation triggers a snapshot rebuild with Redis configured
- **THEN** the gateway MUST publish to the refresh channel, and a subscribed gateway process MUST rebuild its snapshot upon receiving it.
