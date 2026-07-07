use axum::body::Bytes;
use serde_json::Value;

use super::parse::{parse_anthropic_usage, parse_openai_usage};
use crate::domain::TokenUsage;
use crate::model::Protocol;

pub(crate) struct UsageObserver {
    protocol: Protocol,
    line_buffer: Vec<u8>,
    usage: TokenUsage,
    seen_usage: bool,
}

impl UsageObserver {
    pub(crate) fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            line_buffer: Vec::new(),
            usage: TokenUsage::default(),
            seen_usage: false,
        }
    }

    pub(crate) fn feed(&mut self, chunk: &Bytes) {
        self.line_buffer.extend_from_slice(chunk);
        while let Some(newline) = self.line_buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.line_buffer.drain(..=newline).collect::<Vec<u8>>();
            self.consume_line(&line);
        }
    }

    pub(crate) fn finish(&mut self) -> Option<TokenUsage> {
        if !self.line_buffer.is_empty() {
            let line = std::mem::take(&mut self.line_buffer);
            self.consume_line(&line);
        }
        self.seen_usage.then(|| self.usage.clone())
    }

    fn consume_line(&mut self, line: &[u8]) {
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        let Some(data) = line.trim_end().strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let parsed = match self.protocol {
            Protocol::OpenAi => value.get("usage").and_then(parse_openai_usage),
            Protocol::Anthropic => value
                .pointer("/message/usage")
                .and_then(parse_anthropic_usage)
                .or_else(|| value.get("usage").and_then(parse_anthropic_usage)),
        };
        if let Some(parsed) = parsed {
            self.usage.merge_from(&parsed);
            self.seen_usage = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_merges_usage_fields_across_events() {
        let mut observer = UsageObserver::new(Protocol::Anthropic);
        observer.feed(&Bytes::from_static(
            b"data: {\"message\":{\"usage\":{\"input_tokens\":25}}}\n\n",
        ));
        observer.feed(&Bytes::from_static(
            b"data: {\"usage\":{\"output_tokens\":270}}\n\n",
        ));

        let usage = observer.finish().unwrap();
        assert_eq!(usage.input_tokens, Some(25));
        assert_eq!(usage.output_tokens, Some(270));
    }

    #[test]
    fn observer_reassembles_usage_line_split_across_chunks() {
        let mut observer = UsageObserver::new(Protocol::OpenAi);
        observer.feed(&Bytes::from_static(b"data: {\"usage\":{\"prompt_to"));
        observer.feed(&Bytes::from_static(
            b"kens\":100,\"completion_tokens\":25}}\n\n",
        ));

        let usage = observer.finish().unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(25));
    }

    #[test]
    fn observer_parses_final_line_without_trailing_newline() {
        let mut observer = UsageObserver::new(Protocol::OpenAi);
        observer.feed(&Bytes::from_static(
            b"data: {\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}",
        ));

        let usage = observer.finish().unwrap();
        assert_eq!(usage.input_tokens, Some(7));
        assert_eq!(usage.output_tokens, Some(3));
    }

    #[test]
    fn observer_without_usage_returns_none() {
        let mut observer = UsageObserver::new(Protocol::OpenAi);
        observer.feed(&Bytes::from_static(
            b"data: {\"choices\":[]}\n\ndata: [DONE]\n\n",
        ));

        assert!(observer.finish().is_none());
    }
}
