//! Web UI and API (axum): REST under `/api`, live frames on `/ws`, the embedded UI everywhere else.

pub mod api;
pub mod ws;

use std::sync::Arc;

use axum::Router;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};

use crate::serve::App;

#[derive(rust_embed::Embed)]
#[folder = "$HUEJACK_WEB_DIR"]
struct Assets;

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/status", get(api::status))
        .route("/api/bridges/discover", get(api::discover))
        .route(
            "/api/bridge/pair",
            post(api::start_pair).get(api::pair_status),
        )
        .route("/api/areas", get(api::areas))
        .route(
            "/api/settings",
            put(api::put_settings).get(api::get_settings),
        )
        .route("/api/calibration", post(api::calibration))
        .route("/api/test-pattern", post(api::test_pattern))
        .route("/api/bluetooth/pairing", post(api::bluetooth_pairing))
        .route("/api/bluetooth/devices", get(api::bluetooth_devices))
        .route(
            "/api/bluetooth/devices/{address}",
            delete(api::bluetooth_remove),
        )
        .route(
            "/api/{*rest}",
            get(api::not_found)
                .post(api::not_found)
                .put(api::not_found)
                .delete(api::not_found),
        )
        .route("/ws", get(ws::upgrade))
        .fallback(static_file)
        .with_state(app)
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let cache = if path.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            (
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    (header::CACHE_CONTROL, cache.to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        // Single-page app: unknown paths without an extension get the UI.
        None if !path.contains('.') => match Assets::get("index.html") {
            Some(file) => (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                file.data,
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
