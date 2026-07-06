use anyhow::Context;
use axum::http::{
    HeaderMap, HeaderName, HeaderValue,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HOST},
};

use super::model::{Protocol, Provider};

pub(super) fn outbound_headers(
    inbound: &HeaderMap,
    provider: &Provider,
) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();

    for (name, value) in inbound.iter() {
        if is_request_header_forwardable(name) {
            headers.insert(name.clone(), value.clone());
        }
    }

    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", provider.api_key))
            .context("invalid provider API key header")?,
    );

    if provider.protocol == Protocol::Anthropic {
        headers.insert(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_str(&provider.api_key).context("invalid Anthropic API key header")?,
        );

        if let Some(version) = &provider.anthropic_version {
            headers.insert(
                HeaderName::from_static("anthropic-version"),
                HeaderValue::from_str(version).context("invalid Anthropic version header")?,
            );
        }
    }

    Ok(headers)
}

pub(super) fn is_response_header_forwardable(name: &HeaderName) -> bool {
    !matches!(*name, HOST | AUTHORIZATION)
        && !name.as_str().eq_ignore_ascii_case("connection")
        && !name.as_str().eq_ignore_ascii_case("transfer-encoding")
        && !name.as_str().eq_ignore_ascii_case("keep-alive")
        && !name.as_str().eq_ignore_ascii_case("proxy-authenticate")
        && !name.as_str().eq_ignore_ascii_case("proxy-authorization")
        && !name.as_str().eq_ignore_ascii_case("te")
        && !name.as_str().eq_ignore_ascii_case("trailer")
        && !name.as_str().eq_ignore_ascii_case("upgrade")
}

fn is_request_header_forwardable(name: &HeaderName) -> bool {
    matches!(*name, ACCEPT | CONTENT_TYPE)
        || name.as_str().eq_ignore_ascii_case("anthropic-beta")
        || name
            .as_str()
            .eq_ignore_ascii_case("anthropic-dangerous-direct-browser-access")
        || name.as_str().eq_ignore_ascii_case("openai-organization")
        || name.as_str().eq_ignore_ascii_case("openai-project")
        || name.as_str().eq_ignore_ascii_case("user-agent")
        || name.as_str().eq_ignore_ascii_case("x-app")
        || name
            .as_str()
            .eq_ignore_ascii_case("x-claude-code-session-id")
        || name.as_str().eq_ignore_ascii_case("x-request-id")
        || name.as_str().starts_with("x-stainless-")
}
