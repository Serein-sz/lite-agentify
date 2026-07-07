#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TokenUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
}

impl TokenUsage {
    pub(crate) fn has_tokens(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cached_input_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_write_tokens.is_some()
            || self.total_tokens.is_some()
    }

    /// Fills `total_tokens` from the component counts when the upstream did
    /// not report a total. Anthropic responses never include a total, and some
    /// OpenAI-compatible providers omit it. `cached_input_tokens` is a subset
    /// of `input_tokens` (OpenAI), so it is not added; Anthropic's cache
    /// read/write tokens are billed separately and are included.
    pub(crate) fn ensure_total(&mut self) {
        if self.total_tokens.is_some() {
            return;
        }
        let components = [
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        ];
        if components.iter().any(Option::is_some) {
            self.total_tokens = Some(components.into_iter().flatten().sum());
        }
    }

    pub(crate) fn merge_from(&mut self, other: &TokenUsage) {
        if other.input_tokens.is_some() {
            self.input_tokens = other.input_tokens;
        }
        if other.output_tokens.is_some() {
            self.output_tokens = other.output_tokens;
        }
        if other.cached_input_tokens.is_some() {
            self.cached_input_tokens = other.cached_input_tokens;
        }
        if other.cache_read_tokens.is_some() {
            self.cache_read_tokens = other.cache_read_tokens;
        }
        if other.cache_write_tokens.is_some() {
            self.cache_write_tokens = other.cache_write_tokens;
        }
        if other.total_tokens.is_some() {
            self.total_tokens = other.total_tokens;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_reported_total() {
        let mut usage = TokenUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            total_tokens: Some(999),
            ..TokenUsage::default()
        };
        usage.ensure_total();
        assert_eq!(usage.total_tokens, Some(999));
    }

    #[test]
    fn derives_total_from_input_and_output() {
        let mut usage = TokenUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            ..TokenUsage::default()
        };
        usage.ensure_total();
        assert_eq!(usage.total_tokens, Some(120));
    }

    #[test]
    fn derives_total_including_anthropic_cache_tokens() {
        let mut usage = TokenUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cache_read_tokens: Some(300),
            cache_write_tokens: Some(40),
            ..TokenUsage::default()
        };
        usage.ensure_total();
        assert_eq!(usage.total_tokens, Some(460));
    }

    #[test]
    fn excludes_openai_cached_subset_from_total() {
        // cached_input_tokens is a subset of input_tokens and must not be added.
        let mut usage = TokenUsage {
            input_tokens: Some(1000),
            output_tokens: Some(200),
            cached_input_tokens: Some(400),
            ..TokenUsage::default()
        };
        usage.ensure_total();
        assert_eq!(usage.total_tokens, Some(1200));
    }

    #[test]
    fn leaves_total_none_without_components() {
        let mut usage = TokenUsage::default();
        usage.ensure_total();
        assert_eq!(usage.total_tokens, None);
    }
}
