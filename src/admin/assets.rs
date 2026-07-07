use axum::{
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

/// The built admin SPA. Release builds embed `ui/dist` into the binary;
/// debug builds read it from disk at runtime (only `.gitkeep` is committed,
/// so a debug gateway without a frontend build serves a helpful 404).
#[derive(RustEmbed)]
#[folder = "ui/dist"]
struct UiAssets;

/// Serves the SPA for every /admin path that is not an API route: real asset
/// files by path, `index.html` for everything else so client-side routes
/// deep-link correctly.
pub(super) async fn spa_fallback(uri: Uri) -> Response {
    serve::<UiAssets>(uri.path())
}

fn serve<E: RustEmbed>(path: &str) -> Response {
    let trimmed = path.trim_start_matches('/');

    // Unknown /admin/api/* paths are API 404s, never the SPA shell.
    if trimmed == "api" || trimmed.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "unknown admin API endpoint").into_response();
    }

    if !trimmed.is_empty()
        && let Some(file) = E::get(trimmed)
    {
        return file_response(trimmed, file);
    }

    match E::get("index.html") {
        Some(file) => file_response("index.html", file),
        None => (
            StatusCode::NOT_FOUND,
            "admin UI assets are not built; run `pnpm build` in ui/ and rebuild",
        )
            .into_response(),
    }
}

fn file_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    // Vite emits content-hashed files under assets/, safe to cache forever;
    // index.html (and anything else) must revalidate so deploys take effect.
    let cache_control = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, cache_control),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Committed fixture tree standing in for a built SPA in unit tests.
    #[derive(RustEmbed)]
    #[folder = "src/admin/test_dist"]
    struct TestAssets;

    fn header<'r>(response: &'r Response, name: &header::HeaderName) -> &'r str {
        response.headers().get(name).unwrap().to_str().unwrap()
    }

    #[test]
    fn root_serves_index_html_with_no_cache() {
        let response = serve::<TestAssets>("/");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(header(&response, &header::CONTENT_TYPE).starts_with("text/html"));
        assert_eq!(header(&response, &header::CACHE_CONTROL), "no-cache");
    }

    #[test]
    fn client_route_deep_link_serves_index_html() {
        let response = serve::<TestAssets>("/config");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(header(&response, &header::CONTENT_TYPE).starts_with("text/html"));
    }

    #[test]
    fn hashed_asset_serves_javascript_mime_and_immutable_cache() {
        let response = serve::<TestAssets>("/assets/app.js");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(header(&response, &header::CONTENT_TYPE).contains("javascript"));
        assert!(header(&response, &header::CACHE_CONTROL).contains("immutable"));
    }

    #[test]
    fn unknown_api_path_is_not_the_spa_shell() {
        let response = serve::<TestAssets>("/api/does-not-exist");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
