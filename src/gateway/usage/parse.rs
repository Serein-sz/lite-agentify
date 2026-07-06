use serde_json::Value;

use crate::gateway::domain::TokenUsage;
use crate::gateway::model::Protocol;

pub(crate) fn parse_non_streaming_usage(protocol: Protocol, body: &[u8]) -> Option<TokenUsage> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    match protocol {
        Protocol::OpenAi => parse_openai_usage(value.get("usage")?),
        Protocol::Anthropic => parse_anthropic_usage(value.get("usage")?),
    }
}

pub(crate) fn parse_openai_usage(usage: &Value) -> Option<TokenUsage> {
    let input_tokens = number(usage, "prompt_tokens");
    let output_tokens = number(usage, "completion_tokens");
    let total_tokens = number(usage, "total_tokens");
    let cached_input_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|details| number(details, "cached_tokens"));

    let parsed = TokenUsage {
        input_tokens,
        output_tokens,
        cached_input_tokens,
        total_tokens,
        ..TokenUsage::default()
    };
    parsed.has_tokens().then_some(parsed)
}

pub(crate) fn parse_anthropic_usage(usage: &Value) -> Option<TokenUsage> {
    let parsed = TokenUsage {
        input_tokens: number(usage, "input_tokens"),
        output_tokens: number(usage, "output_tokens"),
        cache_write_tokens: number(usage, "cache_creation_input_tokens"),
        cache_read_tokens: number(usage, "cache_read_input_tokens"),
        ..TokenUsage::default()
    };
    parsed.has_tokens().then_some(parsed)
}

fn number(value: &Value, key: &str) -> Option<i64> {
    value.get(key)?.as_i64().filter(|value| *value >= 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_cached_usage() {
        let usage = parse_non_streaming_usage(
            Protocol::OpenAi,
            br#"{"usage":{"prompt_tokens":1000,"completion_tokens":200,"total_tokens":1200,"prompt_tokens_details":{"cached_tokens":400}}}"#,
        )
        .unwrap();

        assert_eq!(usage.input_tokens, Some(1000));
        assert_eq!(usage.output_tokens, Some(200));
        assert_eq!(usage.cached_input_tokens, Some(400));
        assert_eq!(usage.total_tokens, Some(1200));
    }

    #[test]
    fn parses_anthropic_cache_usage() {
        let usage = parse_non_streaming_usage(
            Protocol::Anthropic,
            br#"{"usage":{"input_tokens":1000,"output_tokens":200,"cache_creation_input_tokens":100,"cache_read_input_tokens":300}}"#,
        )
        .unwrap();

        assert_eq!(usage.input_tokens, Some(1000));
        assert_eq!(usage.output_tokens, Some(200));
        assert_eq!(usage.cache_write_tokens, Some(100));
        assert_eq!(usage.cache_read_tokens, Some(300));
    }
}
