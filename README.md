# lite-agentify

## Usage Recording

Token usage and cost persistence is optional. If `usage_database` is absent or disabled, the gateway continues proxying requests without writing usage records. Pricing is deployment-managed configuration; the gateway does not fetch provider prices or hard-code model rates.

Example TOML:

```toml
# [usage_database]
# enabled = true
# url = "postgres://lite_agentify:password@localhost:5432/lite_agentify"
# max_connections = 5

# [[pricing]]
# provider = "openai"
# model = "gpt-4.1"
# input_per_1m = "2.00"
# output_per_1m = "8.00"
# cached_input_per_1m = "0.50"
# currency = "USD"
# pricing_source = "manual-2026-07"

# [[pricing]]
# provider = "anthropic"
# model = "claude-sonnet-4"
# input_per_1m = "3.00"
# output_per_1m = "15.00"
# cache_read_per_1m = "0.30"
# cache_write_per_1m = "3.75"
# currency = "USD"
# pricing_source = "manual-2026-07"

# Fallback pricing is supported with explicit "*" wildcards.
# Lookup order is:
# 1. provider + model
# 2. provider + "*"
# 3. "*" + model
# 4. "*" + "*"
#
# [[pricing]]
# provider = "*"
# model = "gpt-4.1"
# input_per_1m = "2.00"
# output_per_1m = "8.00"
# cached_input_per_1m = "0.50"
# currency = "USD"
# pricing_source = "global-gpt-4.1"
#
# [[pricing]]
# provider = "*"
# model = "*"
# input_per_1m = "1.00"
# output_per_1m = "3.00"
# currency = "USD"
# pricing_source = "global-default"
```

Create the PostgreSQL table before enabling persistence:

```sql
CREATE TABLE IF NOT EXISTS usage_records (
    id uuid PRIMARY KEY,
    request_id text NOT NULL,
    created_at timestamptz NOT NULL,
    provider_id text NOT NULL,
    protocol text NOT NULL,
    path text NOT NULL,
    requested_model text NULL,
    upstream_model text NULL,
    status integer NOT NULL,
    latency_ms bigint NOT NULL,
    input_tokens bigint NULL,
    output_tokens bigint NULL,
    cached_input_tokens bigint NULL,
    cache_read_tokens bigint NULL,
    cache_write_tokens bigint NULL,
    total_tokens bigint NULL,
    estimated_cost numeric(20, 10) NULL,
    currency text NULL,
    usage_source text NOT NULL,
    pricing_source text NULL
);
```
