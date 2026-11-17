//! Settings tab (cache location, flush, clear) and the per-tree cache status
//! endpoint behind the "cached" hints.

use std::path::{Path, PathBuf};

use askama::Template;
use axum::extract::{Form, Query, State};
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::web::common::to_string;
use crate::web::{settings, AppState};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings", get(settings_form))
        .route("/settings/save", post(settings_save))
        .route("/settings/flush", post(settings_flush))
        .route("/settings/clear", post(settings_clear))
        .route("/cache-status", get(cache_status))
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    title: &'static str,
    page: &'static str,
    cache_path: String,
    default_cache_path: String,
    settings_path: String,
    env_override: bool,
    cached_files: usize,
    cache_disk_bytes: Option<u64>,
    cache_dirty: bool,
    message: Option<String>,
}

#[derive(serde::Deserialize)]
struct CachePathForm {
    cache_path: String,
}

#[derive(serde::Deserialize)]
struct CacheStatusQuery {
    path: String,
}

#[derive(serde::Serialize)]
struct CacheStatus {
    cached_files: usize,
    shallow: usize,
    deep: usize,
    phash: usize,
    exif: usize,
}

/// What the queried tree already has cached, layer by layer — powers the
/// "cached" hints next to the path fields.
async fn cache_status(
    State(state): State<AppState>,
    Query(query): Query<CacheStatusQuery>,
) -> Json<CacheStatus> {
    let coverage =
        crate::scanner::cache::lock(&state.cache).branch_coverage(Path::new(query.path.trim()));
    Json(CacheStatus {
        cached_files: coverage.files,
        shallow: coverage.shallow,
        deep: coverage.deep,
        phash: coverage.phash,
        exif: coverage.exif,
    })
}

fn render_settings(state: &AppState, message: Option<String>) -> String {
    let cache_path = state.cache_path();
    let cached_files = crate::scanner::cache::lock(&state.cache).stats().files;
    let cache_dirty = crate::scanner::cache::lock(&state.cache).dirty();
    let cache_disk_bytes = cache_path
        .as_deref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());
    let default_path = crate::scanner::cache::default_cache_path();
    let tpl = SettingsTemplate {
        title: "DeDupe2",
        page: "settings",
        cache_path: cache_path
            .as_deref()
            .map(to_string)
            .unwrap_or_else(|| "(in-memory only)".to_string()),
        default_cache_path: to_string(&default_path),
        settings_path: state
            .settings_path
            .as_deref()
            .map(to_string)
            .unwrap_or_else(|| "(not written)".to_string()),
        env_override: std::env::var_os("DEDUPE2_CACHE").is_some(),
        cached_files,
        cache_disk_bytes,
        cache_dirty,
        message,
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
}

async fn settings_form(State(state): State<AppState>) -> Html<String> {
    Html(render_settings(&state, None))
}

/// Point the cache at a new file: current knowledge is written there and
/// future saves follow; the settings file remembers the choice.
async fn settings_save(
    State(state): State<AppState>,
    Form(form): Form<CachePathForm>,
) -> Html<String> {
    let raw = form.cache_path.trim().to_string();
    let new_path = if raw.is_empty() {
        crate::scanner::cache::default_cache_path()
    } else {
        PathBuf::from(&raw)
    };

    let message = match state.switch_cache_path(new_path.clone()) {
        Ok(()) => {
            let mut saved_settings = settings::Settings::load();
            saved_settings.cache_path = Some(new_path.clone());
            let settings_note = match &state.settings_path {
                Some(path) => match saved_settings.save_to(path) {
                    Ok(()) => String::new(),
                    Err(e) => format!(" (warning: settings file not written: {e})"),
                },
                None => String::new(),
            };
            Some(format!(
                "Cache moved to {}{settings_note}",
                new_path.display()
            ))
        }
        Err(e) => Some(format!("Could not move the cache: {e}")),
    };
    Html(render_settings(&state, message))
}

async fn settings_flush(State(state): State<AppState>) -> Html<String> {
    let message = match state.cache_path() {
        Some(path) => {
            let mut cache = crate::scanner::cache::lock(&state.cache);
            match cache.save(&path) {
                Ok(()) => Some(format!("Cache written to {}", path.display())),
                Err(e) => Some(format!("Could not write the cache: {e}")),
            }
        }
        None => Some("No cache location configured — nothing to write.".to_string()),
    };
    Html(render_settings(&state, message))
}

async fn settings_clear(State(state): State<AppState>) -> Html<String> {
    let removed = state.cache_path().map(|path| {
        let mut cache = crate::scanner::cache::lock(&state.cache);
        *cache = crate::scanner::cache::ScanCache::new();
        std::fs::remove_file(&path).is_ok()
    });
    let message = match (state.cache_path(), removed) {
        (Some(_), Some(true)) => "Cache cleared and its file deleted.".to_string(),
        (Some(path), Some(false)) => {
            format!("Cache cleared in memory (no file at {}).", path.display())
        }
        _ => "Cache cleared.".to_string(),
    };
    Html(render_settings(&state, Some(message)))
}
