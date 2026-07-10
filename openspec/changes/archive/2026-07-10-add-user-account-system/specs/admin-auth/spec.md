# admin-auth Specification (delta)

## ADDED Requirements

### Requirement: Sessions carry user identity and role
The system SHALL associate each session with the authenticated user's id and role, and SHALL reject requests to admin-only endpoints from `user`-role sessions with `403 Forbidden`. Self-service endpoints SHALL scope their effects and results to the session's user id.

#### Scenario: Non-admin blocked from admin endpoint
- **WHEN** a `user`-role session calls an admin-only endpoint such as user management or config
- **THEN** the system MUST respond `403 Forbidden` without executing the operation.

#### Scenario: Self-service scoped to the session user
- **WHEN** a `user`-role session lists API keys or usage
- **THEN** the response MUST include only data owned by the session's user.

#### Scenario: Disabled user's session is rejected
- **WHEN** a session's user is disabled after login
- **THEN** subsequent requests with that session MUST receive `401 Unauthorized`.

## MODIFIED Requirements

### Requirement: Login issues a hardened session cookie
The system SHALL verify login attempts by username and password against the user's argon2id hash, rejecting logins for unknown, disabled, or wrongly-authenticated users with an identical `401` response, and on success SHALL issue a session cookie with a random token of at least 128 bits of entropy, marked `HttpOnly` and `SameSite=Strict`, scoped to the `/admin` path, whose session records the user's id and role.

#### Scenario: Successful login
- **WHEN** a client sends `POST /admin/api/login` with a valid username and the correct password for an active user
- **THEN** the gateway MUST respond with success and set an `HttpOnly`, `SameSite=Strict` session cookie scoped to `/admin` whose session carries that user's identity and role.

#### Scenario: Failed login
- **WHEN** a client sends `POST /admin/api/login` with an unknown username, a disabled user's username, or an incorrect password
- **THEN** the gateway MUST respond `401 Unauthorized` with an identical body in all three cases and MUST NOT set a session cookie.

### Requirement: Failed logins are rate limited
The system SHALL reject login attempts for a username for a lockout window after 5 consecutive failed attempts for that username, regardless of the password submitted during the window, and MUST return the same `429` response whether or not the username exists.

#### Scenario: Lockout after consecutive failures
- **WHEN** 5 consecutive login attempts for a username fail and another attempt for that username arrives within the lockout window
- **THEN** the gateway MUST reject it with `429 Too Many Requests` even if the password is correct.

#### Scenario: Lockout expires
- **WHEN** the lockout window for a username has elapsed and a login attempt with the correct password arrives
- **THEN** the gateway MUST accept it and reset that username's failure counter.

#### Scenario: Lockout does not affect other users
- **WHEN** one username is locked out and a different user submits correct credentials
- **THEN** the gateway MUST accept the other user's login.

## REMOVED Requirements

### Requirement: Admin console is enabled only by a configured admin password
**Reason**: The console is now backed by database user accounts, which always exist after bootstrap (startup fails on an empty `users` table without a bootstrap password). The 404-when-unconfigured gating no longer has a meaning.
**Migration**: `admin_password` remains in config solely as the bootstrap seed for the first admin user (see `user-accounts`). The `/admin` prefix is always served.
