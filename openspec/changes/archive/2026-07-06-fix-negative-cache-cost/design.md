## Context

`calculate_cost` derives regular (non-cached) input tokens before applying per-token-class pricing. It currently subtracts three token classes from `input_tokens`:

```rust
let regular_input = input_tokens
    .saturating_sub(cached_input)
    .saturating_sub(cache_read)
    .saturating_sub(cache_write);
```

This assumes every cache class is a subset of `input_tokens`. That is true only for OpenAI, where `prompt_tokens` includes `cached_tokens`. It is false for Anthropic, where `input_tokens`, `cache_read_input_tokens`, and `cache_creation_input_tokens` are independent, additive counts. For an Anthropic request with `input_tokens = 27586` and `cache_read = 106262`, `regular_input` becomes `-78676`, and the negative regular-input cost drags the total below zero (observed: `-0.3344440000 USD`).

## Goals / Non-Goals

**Goals:**
- Estimated cost is never negative.
- Only token classes that are definitionally a subset of `input_tokens` are subtracted from it.
- OpenAI cost estimation is unchanged.
- Anthropic cost with cache reads is positive and correct.

**Non-Goals:**
- Change token parsing or what is persisted per token class.
- Change pricing configuration, lookup, or streaming observation.
- Retroactively correct already-persisted negative-cost records.

## Decisions

### Subtract only `cached_input` from `input_tokens`

Only OpenAI's `cached_tokens` is a subset of `prompt_tokens`, so it is the only class that must be removed to avoid double-counting. Anthropic's `cache_read` and `cache_write` are separate classes already excluded from `input_tokens`, so subtracting them is wrong.

```rust
let regular_input = input_tokens.saturating_sub(cached_input).max(0);
```

This needs no protocol branching:
- OpenAI: `cached_input = cached_tokens` (subset), `cache_read = cache_write = 0` -> `regular = prompt - cached`.
- Anthropic: `cached_input = 0`, cache classes independent -> `regular = input_tokens`.

Cost components for `cache_read` and `cache_write` continue to be added separately at their configured prices, so those tokens are still billed, just not subtracted from regular input.

Alternatives considered:
- Normalize `input_tokens` at the parse layer to a single "non-cached" meaning: makes `calculate_cost` trivial but changes persisted OpenAI token counts so they no longer match provider-reported values, breaking reconciliation against provider billing. Rejected.
- Protocol-branched subtraction in `calculate_cost`: more code for no benefit; the subset insight removes the need to branch. Rejected.

### Clamp `regular_input` at zero

`.max(0)` is defense-in-depth: even with unexpected provider reporting where a subtracted subset exceeds `input_tokens`, cost cannot go negative.

## Risks / Trade-offs

- The `calculates_cache_aware_cost` unit test currently asserts `0.005265`, a value produced by the buggy subtraction. After the fix the correct value is `0.006465` (cache_read and cache_write tokens no longer reduce regular input). The test must be updated to the corrected expectation; this is expected, not a regression.
- A brief comment is warranted on the subtraction line to record why only `cached_input` is removed, since the reason is a non-obvious cross-provider invariant.

## Migration Plan

1. Change the `regular_input` computation in `calculate_cost` to subtract only `cached_input` and clamp at zero.
2. Update `calculates_cache_aware_cost` to the corrected expected cost.
3. Add a regression test reproducing the negative-cost record (large `cache_read`, small `input_tokens`) asserting non-negative cost.
4. Run `cargo test` and verify OpenSpec status.

No configuration, schema, or deployment change.

## Open Questions

- Should previously persisted negative-cost records be corrected by a one-off backfill? Current plan: out of scope; document that only new records are affected.
