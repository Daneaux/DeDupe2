//! Compare tab follow-ups that touch files: EXIF preview, dated copy/move,
//! flat table transfers, and moving confirmed duplicates to purgatory.

use std::convert::Infallible;
use std::path::{Path, PathBuf};

use askama::Template;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::post;
use axum::Router;
use tokio::sync::mpsc;
use tokio_stream::Stream;

use crate::exif::CreationDate;
use crate::filemover::{
    destination_for, move_to_purgatory_with_progress, transfer_dated_with_progress,
    transfer_files_with_progress, CopyOutcome, Operation,
};
use crate::web::common::{
    dated_from_form, parse_carried_dates, parse_paths, require_dir, sse_stream, to_string,
    ExifRowView, Msg, DEFAULT_FORMAT,
};
use crate::web::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/compare/exif", post(compare_exif))
        .route("/compare/copy", post(compare_copy))
        .route("/compare/transfer", post(compare_transfer))
        .route("/compare/purgatory", post(compare_purgatory))
}

#[derive(Template)]
#[template(path = "compare_exif.html")]
struct CompareExifTemplate {
    destination: String,
    format: String,
    originals_input: String,
    dates_input: String,
    rows: Vec<ExifRowView>,
    valid_count: usize,
    invalid_count: usize,
}

#[derive(Template)]
#[template(path = "compare_copied.html")]
struct CompareCopiedTemplate {
    copied: usize,
    skipped: usize,
    destination: String,
    format: String,
}

#[derive(serde::Deserialize)]
struct TableTransferForm {
    files: String,
    destination: String,
    op: Option<String>,
}

#[derive(Template)]
#[template(path = "transfer_result.html")]
struct TransferResultTemplate {
    destination: String,
    operation: &'static str,
    verb: &'static str,
    planned: usize,
    transferred: usize,
    renamed_on_collision: usize,
    failure_count: usize,
    failures: Vec<(String, String)>,
}

#[derive(serde::Deserialize)]
struct PurgatoryForm {
    files: String,
    b_root: String,
    purgatory: String,
}

#[derive(Template)]
#[template(path = "purgatory_result.html")]
struct PurgatoryResultTemplate {
    purgatory: String,
    b_root: String,
    planned: usize,
    moved: usize,
    renamed_on_collision: usize,
    failure_count: usize,
    failures: Vec<(String, String)>,
}

#[derive(serde::Deserialize)]
struct ExifForm {
    originals: String,
    a: Option<String>,
    destination: Option<String>,
    format: Option<String>,
}

#[derive(serde::Deserialize)]
struct CopyForm {
    originals: String,
    destination: String,
    format: Option<String>,
    op: Option<String>,
    /// `path<TAB>date` lines carried from the EXIF scan so the transfer does
    /// not re-extract metadata for every file.
    dates: Option<String>,
}

async fn compare_exif(
    State(state): State<AppState>,
    Form(form): Form<ExifForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let originals = parse_paths(&form.originals);
    if originals.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no originals to scan".into()));
    }
    let a_default = form.a.unwrap_or_default();
    let destination_root = form
        .destination
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(a_default.as_str())
        .to_string();
    let format = form
        .format
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();
    let originals_input = form.originals.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        progress(0, originals.len());
        let dates =
            crate::Scanner::cache::creation_dates_cached(&state.cache, &originals, &progress);
        let dated: Vec<crate::Scanner::compare::DatedOriginal> = originals
            .iter()
            .cloned()
            .zip(dates)
            .map(
                |(path, creation_date)| crate::Scanner::compare::DatedOriginal {
                    path,
                    creation_date,
                },
            )
            .collect();
        let dates_input = dated
            .iter()
            .map(|d| format!("{}\t{}", to_string(&d.path), d.creation_date))
            .collect::<Vec<_>>()
            .join("\n");
        let html = render_compare_exif(
            &dated,
            &destination_root,
            &format,
            &originals_input,
            &dates_input,
        );
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

async fn compare_copy(
    Form(form): Form<CopyForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let originals = parse_paths(&form.originals);
    if originals.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no originals to copy".into()));
    }
    let dest_str = form.destination.trim().to_string();
    if dest_str.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "provide a destination folder".into(),
        ));
    }
    let destination = PathBuf::from(&dest_str);
    let format = form
        .format
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let op = match form.op.as_deref() {
        Some("move") => Operation::Move,
        _ => Operation::Copy,
    };

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();
    let dest_render = destination.clone();
    let format_render = format.clone();

    let (carried, carried_unknown) = parse_carried_dates(form.dates.as_deref().unwrap_or(""));

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        // Show liveness immediately: the phase label switches from
        // "Working…" the moment this arrives.
        progress(0, originals.len());

        // Dates come from the EXIF scan. Files the scan marked `unknown` are
        // skipped without extraction (they cannot be placed); only paths not
        // seen by the scan at all (stale page, file added since) are re-read.
        let dated = dated_from_form(&originals, &carried, &carried_unknown);

        match transfer_dated_with_progress(&dated, &destination, &format, op, &progress) {
            Ok(outcome) => {
                let html = render_compare_copied(&outcome, &dest_render, &format_render);
                let _ = done_tx.send(Msg::Done(html));
            }
            Err(e) => {
                let _ = done_tx.send(Msg::Error(e.to_string()));
            }
        }
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

fn render_compare_exif(
    dated: &[crate::Scanner::compare::DatedOriginal],
    destination_root: &str,
    format: &str,
    originals_input: &str,
    dates_input: &str,
) -> String {
    let dest_path = PathBuf::from(destination_root);
    let rows: Vec<ExifRowView> = dated
        .iter()
        .map(|d| {
            let valid = matches!(d.creation_date, CreationDate::DateCreated(_));
            let destination = if valid {
                destination_for(&d.path, &d.creation_date, &dest_path, format)
                    .map(|p| to_string(&p))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            ExifRowView {
                path: to_string(&d.path),
                date: d.creation_date.to_string(),
                destination,
                valid,
            }
        })
        .collect();
    let valid_count = rows.iter().filter(|r| r.valid).count();
    let invalid_count = rows.len() - valid_count;

    let tpl = CompareExifTemplate {
        destination: destination_root.to_string(),
        format: format.to_string(),
        originals_input: originals_input.to_string(),
        dates_input: dates_input.to_string(),
        rows,
        valid_count,
        invalid_count,
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
}

fn render_compare_copied(outcome: &CopyOutcome, destination: &Path, format: &str) -> String {
    let tpl = CompareCopiedTemplate {
        copied: outcome.copied,
        skipped: outcome.skipped_no_exif,
        destination: to_string(destination),
        format: format.to_string(),
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
}

async fn compare_purgatory(
    Form(form): Form<PurgatoryForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let files = parse_paths(&form.files);
    if files.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no duplicate files to move".into()));
    }
    let purgatory = form.purgatory.trim().to_string();
    if purgatory.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide a purgatory folder".into()));
    }
    let b_root = form.b_root.trim().to_string();
    if b_root.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "candidate tree root is missing".into(),
        ));
    }
    require_dir(Path::new(&b_root))?;

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        progress(0, files.len());
        let outcome = move_to_purgatory_with_progress(
            &files,
            Path::new(&b_root),
            Path::new(&purgatory),
            &progress,
        );
        let html = render_purgatory(&outcome, &purgatory, &b_root);
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

fn render_purgatory(
    outcome: &crate::filemover::PurgatoryOutcome,
    purgatory: &str,
    b_root: &str,
) -> String {
    let tpl = PurgatoryResultTemplate {
        purgatory: purgatory.to_string(),
        b_root: b_root.to_string(),
        planned: outcome.planned,
        moved: outcome.moved,
        renamed_on_collision: outcome.renamed_on_collision,
        failure_count: outcome.failures.len(),
        failures: outcome
            .failures
            .iter()
            .map(|(p, r)| (to_string(p), r.clone()))
            .collect(),
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
}

/// Copy or move a filtered table selection into one destination folder.
async fn compare_transfer(
    Form(form): Form<TableTransferForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let files = parse_paths(&form.files);
    if files.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "no filtered files to transfer".into(),
        ));
    }
    let destination = form.destination.trim().to_string();
    if destination.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "provide a destination folder".into(),
        ));
    }
    let op = match form.op.as_deref() {
        Some("move") => Operation::Move,
        _ => Operation::Copy,
    };

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        progress(0, files.len());
        let outcome = transfer_files_with_progress(&files, Path::new(&destination), op, &progress);
        let tpl = TransferResultTemplate {
            destination,
            operation: match op {
                Operation::Move => "Move",
                Operation::Copy => "Copy",
            },
            verb: match op {
                Operation::Move => "moved",
                Operation::Copy => "copied",
            },
            planned: outcome.planned,
            transferred: outcome.transferred,
            renamed_on_collision: outcome.renamed_on_collision,
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
