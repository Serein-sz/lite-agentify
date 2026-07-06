## Context

Streaming responses are forwarded to clients unchanged while a passive `UsageObserver` inspects provider-native SSE events for usage metadata. The current observer accumulates every chunk into a `Vec<u8>` and parses the whole buffer once in `finish()`. Two problems live in that path:

1. Retained memory is O(response length) per concurrent stream, even though usage metadata is small and appears late in the stream.
2. Anthropic streaming parsing reads a non-existent `/delta/usage` path and never reads the `message_start` usage, then overwrites usage per event instead of merging, so `input_tokens` is lost for Anthropic streams.

This change keeps the observer passive and the client stream byte-identical. It only changes how bytes are inspected and how usage fields are combined.

## Goals / Non-Goals

**Goals:**
- Make observer retained memory independent of response length.
- Correctly capture Anthropic streaming `input_tokens` and `output_tokens` across separate SSE events.
- Merge usage token classes across events instead of replacing whole records.
- Add streaming tests for Anthropic usage, cross-event merge, and split-line chunk boundaries.

**Non-Goals:**
- Change non-streaming usage parsing, the cost formula, pricing lookup, or persistence schema.
- Change the proxy, failover, or streaming-forwarding behavior; client bytes stay identical.
- Add per-event usage records; still one final usage record per request.

## Decisions

### Parse SSE incrementally, retain only a partial line

`UsageObserver` will keep a small `line_buffer: Vec<u8>` holding only bytes not yet terminated by `\n`. On each `feed`, completed lines are split out and consumed immediately; the trailing unterminated fragment stays buffered for the next chunk. `finish()` processes any final unterminated line.

```
feed(chunk):
  line_buffer.extend(chunk)
  while line_buffer contains b'\n':
    (line, rest) = split_at_first_newline(line_buffer)
    consume_line(line)          # parse SSE "data:" payloads
    line_buffer = rest
finish():
  consume_line(line_buffer)     # trailing partial line, if any
  return merged_usage
```

Retained state becomes: one partial line (bounded by provider line length) plus the merged `TokenUsage`. This is independent of total response length.

Alternatives considered:
- Keep full buffering, only fix the Anthropic path: leaves the memory problem unsolved.
- Byte-level streaming JSON parser: heavier and unnecessary; SSE is line-delimited and per-line JSON is small.

### Merge usage field-by-field instead of replacing

`finish()` currently does `latest = parsed`, which discards earlier fields. Provider event shapes differ:

```
OpenAI (stream_options.include_usage): final data event carries the complete usage object.
Anthropic:
  message_start  -> /message/usage : input_tokens (+ cache_* creation/read)
  message_delta  -> /usage         : output_tokens
```

The observer will merge per token class: a later event that reports a field updates that field; fields it omits are preserved from earlier events. Merge is applied as events are consumed so the final merged `TokenUsage` reflects all events.

```
merge(acc, next):
  for each field in (input, output, cached_input, cache_read, cache_write, total):
    if next.field.is_some(): acc.field = next.field
```

This is correct for OpenAI (single complete event overwrites empty accumulator) and fixes Anthropic (input from `message_start`, output from `message_delta`).

Alternatives considered:
- Keep-last-nonempty whole record: still loses input_tokens for Anthropic because input and output arrive in different events.
- Sum fields across events: wrong; Anthropic `message_delta` reports cumulative output, not increments, and OpenAI repeats totals.

### Fix Anthropic streaming JSON paths

Replace the `/usage` + `/delta/usage` fallback with explicit reads:
- `message_start`: usage at `/message/usage`.
- `message_delta`: usage at `/usage` (sibling of `delta`, not a child).

Parsing stays tolerant: any event whose usage object is absent or unparseable is skipped, and a stream that never yields parseable usage still records `usage_source = unavailable`.

## Risks / Trade-offs

- Provider event shapes may vary by API version -> read documented paths for both event types and merge defensively; unknown shapes yield `unavailable`, never a client-facing error.
- Cumulative vs incremental output tokens: Anthropic `message_delta.usage.output_tokens` is cumulative, so field-level replace (not sum) is intentional and matches keeping the latest reported value.
- UTF-8 across chunk boundaries: splitting on `\n` (a single byte, never a UTF-8 continuation byte) is safe; per-line UTF-8 validation replaces the previous whole-buffer validation.

## Migration Plan

1. Refactor `UsageObserver` to incremental line parsing with a partial-line buffer.
2. Add field-level merge to `TokenUsage` and apply it as events are consumed.
3. Fix Anthropic streaming paths (`/message/usage`, `/usage`).
4. Add tests: Anthropic streaming input+output, cross-event merge, split-line chunk boundary, OpenAI streaming regression.
5. Run `cargo test` and verify OpenSpec status.

No configuration, schema, or deployment change. Behavior change is limited to more accurate Anthropic streaming usage records.

## Open Questions

- Should the merge also populate `total_tokens` by summing input and output when a provider omits it, or leave `total_tokens` provider-reported only? Current plan: leave provider-reported only, consistent with non-streaming parsing.
