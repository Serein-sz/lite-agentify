## Why

Estimated cost can be persisted as a negative value for Anthropic requests with heavy cache reads. `calculate_cost` subtracts `cache_read` and `cache_write` tokens from `input_tokens`, but Anthropic reports those as independent additive token classes, not as a subset of `input_tokens`. When cache reads exceed input tokens, regular input goes negative and drags total cost below zero. The defect was latent until streaming usage began recording Anthropic `input_tokens` and `cache_read_input_tokens` correctly, which is when the negative costs started appearing.

## What Changes

- Fix `calculate_cost` so only `cached_input` (the OpenAI `cached_tokens` subset of `prompt_tokens`) is subtracted from `input_tokens`; stop subtracting `cache_read` and `cache_write`, which are independent token classes for Anthropic.
- Clamp `regular_input` at zero as a defensive guard so cost can never go negative.
- Keep persisted token counts provider-native; no change to what is stored per token class.
- Correct the existing `calculates_cache_aware_cost` test, which currently encodes the buggy subtraction, and add a regression test reproducing the negative-cost record.

## Capabilities

### New Capabilities

### Modified Capabilities
- `llm-gateway`: Correct the cache-aware cost estimation requirement so only cached input tokens that are a subset of regular input are subtracted, and estimated cost is never negative.

## Impact

- Affected code: `src/gateway/usage.rs` (`calculate_cost`), `src/gateway/usage.rs` unit tests.
- Behavior: Anthropic requests with cache reads now estimate positive, correct cost; OpenAI cost estimation is unchanged.
- Data: no schema change; previously persisted negative-cost records are not retroactively corrected.
- Non-goals: no change to token parsing, persistence schema, streaming observation, or pricing configuration.
