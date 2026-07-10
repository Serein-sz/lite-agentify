# admin-usage Specification (delta)

## ADDED Requirements

### Requirement: Usage rows are attributed to a user and API key
The system SHALL persist the authenticated user id and API key id on every usage record created after this change, and SHALL treat rows with NULL attribution as pre-accounts history.

#### Scenario: New usage row carries attribution
- **WHEN** an authenticated proxied request completes and a usage record is written
- **THEN** the record MUST contain the user id and API key id that made the request.

#### Scenario: Historical rows remain readable
- **WHEN** usage queries cover rows written before this change
- **THEN** those rows MUST be returned with empty attribution rather than being excluded or causing errors.

## MODIFIED Requirements

### Requirement: Usage log is queryable with pagination and filters
The system SHALL serve `GET /admin/api/usage` returning recorded usage rows ordered by `created_at` descending, with `page`/`page_size` pagination (`page_size` capped at 200) and a total row count, filterable by time range (`from`/`to`), `provider`, `model`, and status (exact code or `4xx`/`5xx` class). For `admin`-role sessions the endpoint SHALL additionally filter by `user` and `api_key`; for `user`-role sessions the endpoint SHALL return only rows attributed to the session's user.

#### Scenario: Paginated log
- **WHEN** an authenticated admin requests `GET /admin/api/usage?page=2&page_size=50`
- **THEN** the response MUST contain at most 50 rows starting from the 51st most recent record, together with the total matching row count.

#### Scenario: Filtered log
- **WHEN** an authenticated admin requests the usage log filtered by a time range and a provider id
- **THEN** every returned row MUST fall within the range and belong to that provider.

#### Scenario: User sees only own usage
- **WHEN** a `user`-role session requests the usage log
- **THEN** every returned row MUST be attributed to that user, regardless of requested filters.

### Requirement: Usage summary aggregates the dashboard data
The system SHALL serve `GET /admin/api/usage/summary` for a time range, returning in one response: totals (request count, token sums, cost sums grouped by currency, average latency, error rate), a time series bucketed by `hour` or `day`, and a per-provider, per-model breakdown. For `user`-role sessions all aggregates SHALL cover only rows attributed to the session's user; for `admin`-role sessions the summary SHALL cover all rows and support filtering by `user`.

#### Scenario: Summary totals
- **WHEN** an authenticated admin requests the summary for a time range
- **THEN** the response MUST include request count, token totals, cost totals grouped by currency, average latency, and the share of requests with status ≥ 400 within that range.

#### Scenario: Time-series buckets
- **WHEN** an authenticated admin requests the summary with `bucket=day`
- **THEN** the response MUST include per-day data points covering the requested range for request count, tokens, and cost.

#### Scenario: Provider and model breakdown
- **WHEN** an authenticated admin requests the summary
- **THEN** the response MUST include aggregates grouped by provider and model sufficient to rank spend per provider/model.

#### Scenario: User summary is scoped to own usage
- **WHEN** a `user`-role session requests the summary
- **THEN** all totals, series, and breakdowns MUST cover only rows attributed to that user.

## REMOVED Requirements

### Requirement: Disabled usage recording degrades gracefully
**Reason**: PostgreSQL is now a mandatory dependency and usage recording is always enabled; the "no usage database" state no longer exists.
**Migration**: Deployments that ran without `usage_database` must provision PostgreSQL before upgrading (see `user-accounts`). The `usage_enabled: false` response shape is retired.
