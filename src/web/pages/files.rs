//! File helpers for result tables: reveal in the OS file browser, thumbnails.

use std::path::PathBuf;
use std::process::Command;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;

use crate::web::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/reveal", get(reveal))
        .route("/thumb", get(thumb))
}

#[derive(serde::Deserialize)]
struct RevealQuery {
    path: String,
}

async fn reveal(Query(q): Query<RevealQuery>) -> Result<(), (StatusCode, String)> {
    let path = q.path.trim();
    if path.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty path".into()));
    }

    // Open the containing folder with the file selected (no app launch).
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let result = Command::new("explorer")
        .arg(format!("/select,{}", path))
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open")
        .arg(
            std::path::Path::new(path)
                .parent()
                .unwrap_or(std::path::Path::new("/")),
        )
        .spawn();
    #[cfg(target_os = "windows")]
    let _ = &result;
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = &result;

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("reveal failed: {e}"),
        )),
    }
}

#[derive(serde::Deserialize)]
struct ThumbQuery {
    path: String,
}

/// Small JPEG preview of a file for the result tables (lazy-loaded by the
/// browser). Local tool: paths are read directly, nothing is exposed.
async fn thumb(Query(query): Query<ThumbQuery>) -> axum::response::Response {
    use axum::http::header;
    use axum::response::IntoResponse;

    let path = PathBuf::from(query.path.trim());
    let bytes = tokio::task::spawn_blocking(move || crate::image_reader::thumbnail_jpeg(&path))
        .await
        .ok()
        .flatten();
    match bytes {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "private, max-age=3600"),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no preview").into_response(),
    }
}
