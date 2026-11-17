//! One module per tab of the web UI. Each owns its templates, form types,
//! handlers and renderers, and exposes its routes through `routes()`.

use axum::Router;

use super::AppState;

mod compare;
mod consolidate;
mod files;
mod organize;
mod sets;
mod settings;
mod similar;
mod transfer;
mod verify;

/// Every page's routes, merged.
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .merge(compare::routes())
        .merge(consolidate::routes())
        .merge(files::routes())
        .merge(organize::routes())
        .merge(sets::routes())
        .merge(settings::routes())
        .merge(similar::routes())
        .merge(transfer::routes())
        .merge(verify::routes())
}
