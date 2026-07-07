# admin-usage Specification

## Purpose
TBD - created by syncing change add-admin-ui. Update Purpose after archive.

## Requirements
### Requirement: Usage log is queryable with pagination and filters
The system SHALL serve `GET /admin/api/usage` returning recorded usage rows ordered by `created_at` descending, with `page`/`page_size` pagination (`page_size` capped at 200) and a total row count, filterable by time range (`from`/`to`), `provider`, `model`, and status (exact code or `4xx`/`5xx` class).

#### Scenario: Paginated log
- **WHEN** an authenticated admin requests `GET /admin/api/usage?page=2&page_size=50`
- **THEN** the response MUST contain at most 50 rows starting from the 51st most recent record, together with the total matching row count.

#### Scenario: Filtered log
- **WHEN** an authenticated admin requests the usage log filtered by a time range and a provider id
- **THEN** every returned row MUST fall within the range and belong to that provider.

### Requirement: Usage summary aggregates the dashboard data
The system SHALL serve `GET /admin/api/usage/summary` for a time range, returning in one response: totals (request count, token sums, cost sums grouped by currency, average latency, error rate), a time series bucketed by `hour` or `day`, and a per-provider, per-model breakdown.

#### Scenario: Summary totals
- **WHEN** an authenticated admin requests the summary for a time range
- **THEN** the response MUST include request count, token totals, cost totals grouped by currency, average latency, and the share of requests with status ≥ 400 within that range.

#### Scenario: Time-series buckets
- **WHEN** an authenticated admin requests the summary with `bucket=day`
- **THEN** the response MUST include per-day data points covering the requested range for request count, tokens, and cost.

#### Scenario: Provider and model breakdown
- **WHEN** an authenticated admin requests the summary
- **THEN** the response MUST include aggregates grouped by provider and model sufficient to rank spend per provider/model.

### Requirement: Disabled usage recording degrades gracefully
The system SHALL respond `200` with an explicit `usage_enabled: false` indicator and empty data on usage endpoints when no usage database is configured or usage recording is disabled.

#### Scenario: Usage endpoints without a usage database
- **WHEN** the gateway runs without a configured usage database and an authenticated admin requests a usage endpoint
- **THEN** the response MUST be `200` with `usage_enabled: false` and empty data, not an error status.
