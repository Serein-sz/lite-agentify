# admin-ui Specification (delta)

## ADDED Requirements

### Requirement: Console is partitioned by role
The console SHALL render views according to the session's role: `user`-role sessions see key management, their own usage dashboard, and password change; `admin`-role sessions additionally see user management, all-users usage, and the config editor. Views a role cannot access MUST NOT be reachable through navigation, and direct navigation to them MUST be denied client-side with the API enforcing `403` server-side.

#### Scenario: User-role navigation
- **WHEN** a `user`-role account logs into the console
- **THEN** the console MUST show key management, own usage, and password change, and MUST NOT offer user management or the config editor.

#### Scenario: Admin-role navigation
- **WHEN** an `admin`-role account logs into the console
- **THEN** the console MUST offer user management, usage across all users, key management, and the config editor.

### Requirement: Console provides self-service key management
The console SHALL provide a key management view where the user creates a key (with a name), sees the plaintext exactly once in the creation result with a copy affordance and a warning that it cannot be shown again, and lists and revokes keys (prefix, name, status, created and last-used timestamps).

#### Scenario: Key creation shows plaintext once
- **WHEN** a user creates a key in the console
- **THEN** the console MUST display the full key once with a copy button and warn it will not be shown again, and the subsequent key list MUST show only the prefix and metadata.

#### Scenario: Key revocation
- **WHEN** a user revokes one of their keys in the console
- **THEN** the console MUST ask for confirmation, then show the key as revoked.

### Requirement: Console provides admin user management
The console SHALL provide an admin-only user management view listing users with role and status, supporting user creation (username, initial password, role), disable/enable, and password reset.

#### Scenario: Admin creates a user
- **WHEN** an admin creates a user in the console with a username, initial password, and role
- **THEN** the console MUST show the new user in the list as active.

#### Scenario: Admin disables a user
- **WHEN** an admin disables a user in the console
- **THEN** the console MUST ask for confirmation and then show the user as disabled.

## MODIFIED Requirements

### Requirement: Unauthenticated console access lands on login
The console SHALL present a login view with username and password fields to unauthenticated visitors, and after successful login SHALL present the role-appropriate landing view; API responses of `401` SHALL return the user to the login view.

#### Scenario: Visit without session
- **WHEN** a visitor without a valid session opens `/admin`
- **THEN** the console MUST show the login view with username and password fields and MUST NOT render usage or config data.

#### Scenario: Session expiry during use
- **WHEN** a user's session expires and a console API call returns `401`
- **THEN** the console MUST return to the login view.
