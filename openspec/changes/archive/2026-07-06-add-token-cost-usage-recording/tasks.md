## 1. Configuration and Data Model

- [x] 1.1 Add optional usage database configuration fields for enablement, PostgreSQL URL, and connection settings.
- [x] 1.2 Add pricing configuration fields keyed by provider and upstream model, including regular input, output, cached input, cache read, cache write, currency, and optional pricing source/version.
- [x] 1.3 Validate pricing entries for non-empty provider/model names, non-negative prices, and valid currency values.
- [x] 1.4 Add internal usage record and token usage structs that exclude prompt, message, and completion content.

## 2. PostgreSQL Persistence

- [x] 2.1 Add SeaORM and PostgreSQL dependencies.
- [x] 2.2 Define the `usage_records` entity with request metadata, model fields, token count fields, estimated cost, currency, usage source, and timestamps.
- [x] 2.3 Add schema setup support through migration code or documented SQL for creating `usage_records`.
- [x] 2.4 Implement a usage recorder abstraction with a disabled/no-op recorder and a SeaORM PostgreSQL recorder.
- [x] 2.5 Initialize the optional usage recorder during gateway startup without preventing startup when configuration is absent.

## 3. Usage Parsing

- [x] 3.1 Implement OpenAI-compatible non-streaming usage parsing, including cached prompt token extraction.
- [x] 3.2 Implement Anthropic-compatible non-streaming usage parsing, including cache creation and cache read token extraction.
- [x] 3.3 Implement provider-native SSE usage observers for OpenAI-compatible streaming responses.
- [x] 3.4 Implement provider-native SSE usage observers for Anthropic-compatible streaming responses.
- [x] 3.5 Ensure missing or malformed usage metadata produces `usage_source = unavailable` without failing response forwarding.

## 4. Cost Calculation

- [x] 4.1 Resolve pricing by selected provider id and upstream model name.
- [x] 4.2 Calculate regular input cost after subtracting cached input, cache read, and cache write tokens where applicable.
- [x] 4.3 Calculate output, cached input, cache read, and cache write cost components only when matching pricing fields are configured.
- [x] 4.4 Leave estimated cost unavailable when required pricing is missing instead of falling back to regular input pricing for cache tokens.
- [x] 4.5 Use decimal-safe arithmetic for persisted estimated cost values.
- [x] 4.6 Support explicit `*` wildcard pricing fallback for provider and upstream model matching.

## 5. Proxy Integration

- [x] 5.1 Capture requested model and resolved upstream model during route/provider selection.
- [x] 5.2 Record usage for successful non-streaming responses by buffering, parsing, and rebuilding the response body unchanged.
- [x] 5.3 Wrap streaming response bodies with passive observers that forward bytes unchanged and record usage after stream completion.
- [x] 5.4 Persist request metadata with unavailable usage when usage parsing or pricing is unavailable.
- [x] 5.5 Log persistence failures without changing client response status, headers, or body.

## 6. Documentation and Examples

- [x] 6.1 Add commented example TOML for usage database configuration.
- [x] 6.2 Add commented example TOML for provider/model pricing with cache-aware price fields.
- [x] 6.3 Document that usage persistence is disabled when database configuration is absent.
- [x] 6.4 Document that prices are deployment-managed and not fetched or hard-coded by the gateway.
- [x] 6.5 Document explicit `*` wildcard fallback pricing examples and lookup order.

## 7. Tests and Verification

- [x] 7.1 Add tests for configuration loading and validation of usage database and pricing sections.
- [x] 7.2 Add tests for OpenAI-compatible usage parsing with cached prompt tokens.
- [x] 7.3 Add tests for Anthropic-compatible usage parsing with cache creation and cache read tokens.
- [x] 7.4 Add tests for cache-aware cost calculation and missing cache pricing behavior.
- [x] 7.5 Add tests that prompt and completion bodies are not stored in usage records.
- [x] 7.6 Add tests that usage parsing and persistence failures do not alter proxied responses.
- [x] 7.7 Run `cargo test` and verify OpenSpec status before implementation is considered complete.
- [x] 7.8 Add tests for wildcard pricing fallback priority.
