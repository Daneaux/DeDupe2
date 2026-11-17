//! HTTP layer: form parsing, SSE progress streaming, and template
//! rendering. Pure translation between axum and the library APIs —
//! exposed as `app()` so handler-level tests can drive the full router.

mod common;
mod guard;
mod pages;
pub mod settings;
mod state;

use axum::extract::DefaultBodyLimit;
use axum::Router;
use tower_http::services::ServeDir;

pub use state::AppState;

/// The full app over throwaway in-memory state.
pub fn app() -> Router {
    app_with_state(AppState::default())
}

pub fn app_with_state(state: AppState) -> Router {
    pages::routes()
        .nest_service("/static", ServeDir::new("static"))
        // Compare forms post the full originals list back to the server (one
        // path per line) — several MB for large candidate trees. NOTE: this
        // layer must come after the routes or it applies to nothing.
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        // Outermost: reject other sites' requests before any handler runs.
        .layer(axum::middleware::from_fn(guard::same_origin_only))
        .with_state(state)
}
