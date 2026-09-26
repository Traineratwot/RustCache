//! Embedded SPA assets (feature `embed-ui`).
//!
//! Serves the Vite production build from `web/dist` as static files with
//! an `index.html` fallback so client-side routes work on refresh.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../web/dist/"]
struct Assets;

pub async fn spa_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess(path);
            let body = file.data.into_owned();
            ([(header::CONTENT_TYPE, mime)], body).into_response()
        }
        None => {
            // SPA fallback: any unknown path serves index.html
            match Assets::get("index.html") {
                Some(file) => {
                    let body = file.data.into_owned();
                    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
                }
                None => (StatusCode::NOT_FOUND, "index.html not embedded").into_response(),
            }
        }
    }
}

fn mime_guess(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else if path.ends_with(".eot") {
        "application/vnd.ms-fontobject"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}
