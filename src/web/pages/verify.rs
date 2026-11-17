//! Verify tab: check files sit in the folder their EXIF date implies, and
//! re-home the ones that do not.

use std::convert::Infallible;
use std::path::{Path, PathBuf};

use askama::Template;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::mpsc;
use tokio_stream::Stream;

use crate::filemover::{plan_rehome, rehome_verified, RehomeOutcome};
use crate::scanner::compare::{verify_tree_dates_cached, VerifyOutcome};
use crate::web::common::{require_dir, sse_stream, to_string, Msg, DEFAULT_FORMAT};
use crate::web::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/verify", get(verify_form))
        .route("/verify/run", post(verify_run))
        .route("/verify/rehome", post(verify_rehome))
}

#[derive(Template)]
#[template(path = "verify.html")]
struct VerifyFormTemplate {
    title: &'static str,
    page: &'static str,
}

#[derive(Template)]
#[template(path = "verify_result.html")]
struct VerifyResultTemplate {
    root: String,
    total: usize,
    ok: usize,
    mismatch_count: usize,
    outside_structure: usize,
    undated: usize,
    mismatches_json: String,
    files_json: String,
}

#[derive(serde::Deserialize)]
struct VerifyForm {
    root: String,
}

#[derive(serde::Deserialize)]
struct RehomeForm {
    mismatches: String,
    root: String,
    format: Option<String>,
}

#[derive(Template)]
#[template(path = "rehome_result.html")]
struct RehomeResultTemplate {
    moved: usize,
    renamed_on_collision: usize,
    failure_count: usize,
    failures: Vec<(String, String)>,
    remaining_mismatches: usize,
    reverified_total: usize,
    root: String,
}

async fn verify_form() -> VerifyFormTemplate {
    VerifyFormTemplate {
        title: "DeDupe2",
        page: "verify",
    }
}

async fn verify_run(
    State(state): State<AppState>,
    Form(form): Form<VerifyForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let root_str = form.root.trim().to_string();
    if root_str.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "provide the destination tree".into(),
        ));
    }
    let root = PathBuf::from(&root_str);
    require_dir(&root)?;

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        progress(0, 0);
        match verify_tree_dates_cached(&root, &state.cache, &progress) {
            Ok(outcome) => {
                let html = render_verify(&outcome, &root_str);
                let _ = done_tx.send(Msg::Done(html));
            }
            Err(e) => {
                let _ = done_tx.send(Msg::Error(e.to_string()));
            }
        }
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

fn render_verify(outcome: &VerifyOutcome, root: &str) -> String {
    let mismatches_json = serde_json::to_string(
        &outcome
            .mismatches
            .iter()
            .map(|m| {
                serde_json::json!({
                    "path": to_string(&m.path),
                    "exif": m.exif_date,
                    "folder": m.folder_date,
                    "days": m.days_off,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());

    let files_json = serde_json::to_string(
        &outcome
            .files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "path": to_string(&f.path),
                    "exif": f.exif_date,
                    "folder": f.folder_date,
                    "status": f.status.as_str(),
                    "days": f.days_off,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());

    let tpl = VerifyResultTemplate {
        root: root.to_string(),
        total: outcome.total,
        ok: outcome.ok,
        mismatch_count: outcome.mismatches.len(),
        outside_structure: outcome.outside_structure,
        undated: outcome.undated,
        mismatches_json,
        files_json,
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
}

async fn verify_rehome(
    State(state): State<AppState>,
    Form(form): Form<RehomeForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let mut mismatches: Vec<(PathBuf, String)> = Vec::new();
    for line in form.mismatches.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let path = parts.next().unwrap_or("").trim();
        let date = parts.next().unwrap_or("").trim();
        if !path.is_empty() && !date.is_empty() {
            mismatches.push((PathBuf::from(path), date.to_string()));
        }
    }
    if mismatches.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no mismatches to re-home".into()));
    }
    let root = PathBuf::from(form.root.trim());
    if root.as_os_str().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "missing destination tree root".into(),
        ));
    }
    require_dir(&root)?;
    let format = form
        .format
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        progress(0, mismatches.len());
        let plans = plan_rehome(&mismatches, &root, &format);
        let outcome = rehome_verified(&plans, &progress);

        // Prove it: re-verify the tree after the re-home.
        let reverified = match verify_tree_dates_cached(&root, &state.cache, &progress) {
            Ok(v) => v,
            Err(e) => {
                let _ = done_tx.send(Msg::Error(e.to_string()));
                return;
            }
        };

        let html = render_rehome(&outcome, &reverified, &root);
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

fn render_rehome(outcome: &RehomeOutcome, reverified: &VerifyOutcome, root: &Path) -> String {
    let failures: Vec<(String, String)> = outcome
        .failures
        .iter()
        .map(|(p, r)| (to_string(p), r.clone()))
        .collect();

    let tpl = RehomeResultTemplate {
        moved: outcome.moved,
        renamed_on_collision: outcome.renamed_on_collision,
        failure_count: failures.len(),
        failures,
        remaining_mismatches: reverified.mismatches.len(),
        reverified_total: reverified.total,
        root: to_string(root),
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
}
