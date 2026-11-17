//! Organize tab: preview and run an EXIF-dated move into a destination tree.

use std::convert::Infallible;
use std::path::Path;

use askama::Template;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::mpsc;
use tokio_stream::Stream;

use crate::filemover::{destination_for, organize_into_destination_with_progress, Operation};
use crate::web::common::{
    dated_from_form, parse_carried_dates, parse_paths, require_dir, resolved_path, sse_stream,
    to_string, truncate_rows, Msg, DEFAULT_FORMAT,
};
use crate::web::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/organize", get(organize_form))
        .route("/organize/scan", post(organize_scan))
        .route("/organize/run", post(organize_run))
}

#[derive(Template)]
#[template(path = "organize.html")]
struct OrganizeFormTemplate {
    title: &'static str,
    page: &'static str,
}

#[derive(serde::Deserialize)]
struct OrganizeForm {
    source: String,
    destination: String,
    format: Option<String>,
}

#[derive(Template)]
#[template(path = "organize_result.html")]
struct OrganizeResultTemplate {
    source: String,
    destination: String,
    format: String,
    cache_reused: usize,
    cache_computed: usize,
    total: usize,
    dated_count: usize,
    unknown_count: usize,
    rows: Vec<OrganizeRowView>,
    rows_more: usize,
    files_input: String,
    dates_input: String,
}

struct OrganizeRowView {
    pub source: String,
    pub date: String,
    pub destination: String,
}

#[derive(serde::Deserialize)]
struct OrganizeRunForm {
    files: String,
    dates: Option<String>,
    destination: String,
    format: Option<String>,
    op: Option<String>,
}

#[derive(Template)]
#[template(path = "organize_done.html")]
struct OrganizeDoneTemplate {
    destination: String,
    planned: usize,
    moved: usize,
    skipped_no_date: usize,
    rejected_count: usize,
    rejected: Vec<(String, String)>,
    failure_count: usize,
    failures: Vec<(String, String)>,
}

async fn organize_form() -> OrganizeFormTemplate {
    OrganizeFormTemplate {
        title: "DeDupe2",
        page: "organize",
    }
}

async fn organize_scan(
    State(state): State<AppState>,
    Form(form): Form<OrganizeForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let source_str = form.source.trim().to_string();
    if source_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide a source tree".into()));
    }
    let destination = form.destination.trim().to_string();
    if destination.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide a destination root".into()));
    }
    require_dir(Path::new(&source_str))?;
    // Organizing into a folder inside the source would re-process the moved
    // files on the next run (and can loop); the source may live inside the
    // destination, but not the other way around.
    let source_resolved = resolved_path(Path::new(&source_str));
    let destination_resolved = resolved_path(Path::new(&destination));
    if destination_resolved.starts_with(&source_resolved) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "the destination is inside the source tree: {} vs {}",
                destination_resolved.display(),
                source_resolved.display()
            ),
        ));
    }
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
        let files = match crate::image_reader::collect_image_files(Path::new(&source_str)) {
            Ok(files) => files,
            Err(e) => {
                let _ = done_tx.send(Msg::Error(e.to_string()));
                return;
            }
        };
        progress(0, files.len());
        let before = crate::scanner::cache::lock(&state.cache).stats();
        let dates = crate::scanner::cache::creation_dates_cached(&state.cache, &files, &progress);
        let after = crate::scanner::cache::lock(&state.cache).stats();
        let cache_reused = after.reused.saturating_sub(before.reused);
        let cache_computed = after.computed.saturating_sub(before.computed);
        let dated: Vec<crate::scanner::compare::DatedOriginal> = files
            .iter()
            .cloned()
            .zip(dates)
            .map(
                |(path, creation_date)| crate::scanner::compare::DatedOriginal {
                    path,
                    creation_date,
                },
            )
            .collect();

        let mut rows: Vec<OrganizeRowView> = Vec::new();
        let mut files_input: Vec<String> = Vec::new();
        let mut dates_input: Vec<String> = Vec::new();
        let mut dated_count = 0usize;
        for d in &dated {
            let source = to_string(&d.path);
            let destination_path =
                destination_for(&d.path, &d.creation_date, Path::new(&destination), &format)
                    .map(to_string)
                    .unwrap_or_default();
            if !destination_path.is_empty() {
                dated_count += 1;
            }
            rows.push(OrganizeRowView {
                source: source.clone(),
                date: d.creation_date.to_string(),
                destination: destination_path,
            });
            files_input.push(source);
            dates_input.push(format!("{}\t{}", to_string(&d.path), d.creation_date));
        }
        let total = rows.len();
        let unknown_count = total - dated_count;
        let (rows, rows_more) = truncate_rows(rows);

        let tpl = OrganizeResultTemplate {
            source: source_str,
            destination,
            format,
            total,
            dated_count,
            unknown_count,
            rows,
            rows_more,
            files_input: files_input.join("\n"),
            dates_input: dates_input.join("\n"),
            cache_reused,
            cache_computed,
        };
        let html = tpl
            .render()
            .unwrap_or_else(|e| format!("render error: {e}"));
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

async fn organize_run(
    Form(form): Form<OrganizeRunForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let files = parse_paths(&form.files);
    if files.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "nothing to organize".into()));
    }
    let destination = form.destination.trim().to_string();
    if destination.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide a destination root".into()));
    }
    let format = form
        .format
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let op = match form.op.as_deref() {
        Some("copy") => Operation::Copy,
        _ => Operation::Move,
    };
    let (carried, carried_unknown) = parse_carried_dates(form.dates.as_deref().unwrap_or(""));

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        progress(0, files.len());

        let dated = dated_from_form(&files, &carried, &carried_unknown);
        let outcome = organize_into_destination_with_progress(
            &dated,
            Path::new(&destination),
            &format,
            op,
            &progress,
        );

        let tpl = OrganizeDoneTemplate {
            destination,
            planned: outcome.planned,
            moved: outcome.moved,
            skipped_no_date: outcome.skipped_no_date,
            rejected_count: outcome.rejected_duplicates.len(),
            rejected: outcome
                .rejected_duplicates
                .iter()
                .map(|(source, existing)| (to_string(source), to_string(existing)))
                .collect(),
            failure_count: outcome.failures.len(),
            failures: outcome
                .failures
                .iter()
                .map(|(path, reason)| (to_string(path), reason.clone()))
                .collect(),
        };
        let html = tpl
            .render()
            .unwrap_or_else(|e| format!("render error: {e}"));
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}
