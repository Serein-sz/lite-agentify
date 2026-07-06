#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UsageSource {
    ProviderResponse,
    StreamSummary,
    Unavailable,
}

impl std::fmt::Display for UsageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProviderResponse => f.write_str("provider_response"),
            Self::StreamSummary => f.write_str("stream_summary"),
            Self::Unavailable => f.write_str("unavailable"),
        }
    }
}
