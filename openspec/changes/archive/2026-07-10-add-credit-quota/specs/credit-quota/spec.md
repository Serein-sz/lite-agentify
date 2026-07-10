# credit-quota Specification (delta)

## ADDED Requirements

### Requirement: Credit is an append-only per-user ledger
The system SHALL store credit as append-only `credit_grants` rows (user, USD amount, note, granting admin, timestamp), where corrections are negative entries, and SHALL define a user's balance as the sum of grants minus the sum of attributed usage cost. The system MUST NOT store a mutable balance value.

#### Scenario: Grant increases balance
- **WHEN** an admin grants 50 USD to a user with a prior balance of 10 USD
- **THEN** the user's reported balance MUST become 60 USD and the ledger MUST contain the new entry with the granting admin.

#### Scenario: Correction is a negative entry
- **WHEN** an admin records a −20 USD correction
- **THEN** the balance MUST decrease by 20 USD and the original entries MUST remain unchanged in the ledger.

#### Scenario: Only admins grant credit
- **WHEN** a `user`-role session calls the grant endpoint
- **THEN** the system MUST respond `403 Forbidden`.

### Requirement: Exhausted user balance softly rejects requests before upstream contact
The system SHALL check the user's spent counter against the granted total before forwarding any proxied request, rejecting with a protocol-native `402` error naming the balance scope when spent ≥ granted, without contacting any provider. Enforcement is soft: the check reads fast counters, MUST NOT query the database on the request path, MUST NOT delay or fail responses due to counter maintenance, and bounded overdraft from in-flight requests and counter lag is acceptable.

#### Scenario: Request over an exhausted balance is rejected
- **WHEN** a user's cumulative spend has reached their granted total and a new request arrives on any of their keys
- **THEN** the gateway MUST respond `402` with a protocol-native error body and MUST NOT contact any provider.

#### Scenario: Spending resumes after a new grant
- **WHEN** an admin grants additional credit to an exhausted user
- **THEN** subsequent requests MUST be accepted once the grant is reflected (snapshot refresh), without a process restart.

#### Scenario: Enforcement stays off the database
- **WHEN** a proxied request undergoes the balance check
- **THEN** the check MUST be served from in-process or Redis counters and MUST NOT perform a database query.

### Requirement: API keys enforce an optional cumulative spend cap
The system SHALL support an optional USD spend cap per API key; when the key's cumulative attributed cost reaches the cap, requests with that key MUST be rejected with `402` naming the key cap, while the user's other keys remain governed only by the user balance.

#### Scenario: Capped key stops at its cap
- **WHEN** a key with a 5 USD cap has accumulated 5 USD of usage cost and a request arrives with that key
- **THEN** the gateway MUST respond `402` naming the key cap and MUST NOT contact any provider.

#### Scenario: Other keys are unaffected by one key's cap
- **WHEN** one key of a user is capped out and another key of the same user has no cap
- **THEN** requests with the other key MUST proceed while the user balance lasts.

### Requirement: Spend counters are seeded and reconciled from PostgreSQL
The system SHALL maintain per-user and per-key cumulative spend counters incremented asynchronously with each usage record's cost, seeded from PostgreSQL sums at startup, and periodically reconciled by recomputing grants and usage sums and resetting the counters, so counter drift is bounded by the reconciliation interval. Usage rows without a cost value count as zero.

#### Scenario: Boot seeds counters from history
- **WHEN** the gateway starts with existing usage and grant history
- **THEN** the counters MUST reflect the historical sums before the first request is enforced.

#### Scenario: Reconciliation heals drift
- **WHEN** a counter has drifted from the PostgreSQL-derived truth (e.g. lost increments)
- **THEN** the next reconciliation pass MUST reset it to the recomputed value.

### Requirement: Balances are visible to their owners and admins
The system SHALL expose to each user their own balance, granted total, and cumulative spend, and to admins the same per user plus the full ledger, and SHALL expose per-key cumulative spend on key listings.

#### Scenario: User sees own balance
- **WHEN** a `user`-role session requests their balance
- **THEN** the response MUST include granted total, spent total, and remaining balance for that user only.

#### Scenario: Admin views ledger
- **WHEN** an admin requests a user's credit history
- **THEN** the response MUST include the grant entries with amounts, notes, granting admin, and timestamps.
