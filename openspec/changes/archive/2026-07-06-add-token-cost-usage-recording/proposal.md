## Why

The gateway can proxy and log operational request metadata, but it cannot persist token usage or estimate provider cost for billing, budgeting, or usage reporting. Adding durable usage recording now turns the gateway from a pass-through proxy into a measurable control point for LLM spend.

## What Changes

- Add token usage extraction for OpenAI-compatible and Anthropic-compatible responses, including non-streaming bodies and stream usage metadata when providers expose it.
- Estimate request cost from configured provider/model pricing, including separate cached input, cache read, and cache write prices where provider usage reports those token classes.
- Persist usage records to PostgreSQL through SeaORM when usage database configuration is enabled.
- Add gateway configuration for usage database connectivity and per-provider model pricing.
- Keep prompt and completion content out of persisted usage records.
- Preserve current proxy behavior when usage parsing, cost calculation, or persistence is unavailable; client responses must not fail because usage recording failed.
- Add commented example configuration for PostgreSQL usage storage and pricing so deployments can opt in without guessing field names.

## Capabilities

### New Capabilities

### Modified Capabilities
- `llm-gateway`: Extend gateway request metadata recording into durable token usage and cost recording backed by configured PostgreSQL storage.

## Impact

- Affected code: gateway configuration loading, router response handling, upstream streaming body handling, usage parsing, pricing calculation, persistence layer, and tests.
- Dependencies: add SeaORM and PostgreSQL driver support, plus migration or schema setup support for usage records.
- Systems: optional PostgreSQL database for usage persistence; deployments without usage database configuration continue to run without persistence.
- Configuration: new optional usage database and pricing sections in the TOML gateway config, documented with commented examples.
