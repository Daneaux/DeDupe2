use std::convert::Infallible;
use std::path::{Path, PathBuf};

use askama::Template;
use axum::extract::{DefaultBodyLimit, Form, Query};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{routing::get, routing::post, Router};
use dedupe2::filemover::{
    consolidate_events_with_progress, default_event_purgatory,
    preview_consolidate_events_with_progress, ConsolidationOutcome, ConsolidationPlan,
};
use dedupe2::Scanner::compare::{
    compare_folders_with_progress, compare_sets_with_progress, deep_scan_pairs_with_progress,
    scan_exif_with_progress, verify_tree_dates_with_progress, Comparison, DupPair,
    SetComparison, VerifyOutcome,
};
use dedupe2::filemover::{
    destination_for, plan_rehome, rehome_verified, transfer_originals_with_progress,
    CopyOutcome, RehomeOutcome,
};
use std::process::Command;

use dedupe2::exif::CreationDate;
use dedupe2::filemover::Operation;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tower_http::services::ServeDir;

#[derive(Template)]
#[template(path = "consolidate.html")]
struct ConsolidateFormTemplate {
    title: &'static str,
    page: &'static str,
}

#[derive(Template)]
#[template(path = "compare.html")]
struct CompareFormTemplate {
    title: &'static str,
    page: &'static str,
}

#[derive(Template)]
#[template(path = "consolidate_preview.html")]
struct ConsolidatePreviewTemplate {
    dir: String,
    purgatory: String,
    foldings: usize,
    purged_total: usize,
    entries: Vec<ConsolidateEntryView>,
}

#[derive(Template)]
#[template(path = "consolidate_done.html")]
struct ConsolidateDoneTemplate {
    folded: usize,
    moved: usize,
    purged: usize,
    purgatory: String,
}

#[derive(Template)]
#[template(path = "compare_result.html")]
struct CompareResultTemplate {
    a: String,
    b: String,
    a_rows: Vec<PathRowView>,
    a_more: usize,
    dup_rows: Vec<DupRowView>,
    dup_more: usize,
    b_rows: Vec<PathRowView>,
    b_more: usize,
    a_only_count: usize,
    b_only_count: usize,
    dup_total: usize,
    dup_pairs_all: String,
    unreadable_rows: Vec<PathRowView>,
    unreadable_more: usize,
    unreadable_count: usize,
    originals_all: String,
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

#[derive(Template)]
#[template(path = "sets.html")]
struct SetsFormTemplate {
    title: &'static str,
    page: &'static str,
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

#[derive(Template)]
#[template(path = "sets_result.html")]
struct SetsResultTemplate {
    a: String,
    a_total: usize,
    a_unique: usize,
    a_unreadable: usize,
    duplicates: usize,
    candidates_only: usize,
    candidates: Vec<SetTreeRowView>,
}

struct SetTreeRowView {
    pub root: String,
    pub total: usize,
    pub duplicates: usize,
    pub unique_to_tree: usize,
    pub shared_with_candidates: usize,
    pub unreadable: usize,
}

#[derive(serde::Deserialize)]
struct SetForm {
    a: String,
    candidates: String,
}

#[derive(serde::Deserialize)]
struct DeepForm {
    pairs: String,
    originals: String,
    destination: Option<String>,
    format: Option<String>,
}

#[derive(Template)]
#[template(path = "deep_scan.html")]
struct DeepScanTemplate {
    checked: usize,
    kept: usize,
    removed_count: usize,
    removed: Vec<DeepPairView>,
    originals_input: String,
    destination: String,
    format: String,
    valid_count: usize,
}

struct DeepPairView {
    pub a: String,
    pub b: String,
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
}

struct PathRowView {
    pub path: String,
}

struct DupRowView {
    pub tree: String,
    pub badge: String,
    pub row_class: String,
    pub path: String,
}

struct ExifRowView {
    pub path: String,
    pub date: String,
    pub destination: String,
    pub valid: bool,
}

#[derive(serde::Deserialize)]
struct CompareForm {
    a: String,
    b: String,
}

struct ConsolidateEntryView {
    pub header: String,
    pub moved_count: usize,
    pub purged: Vec<String>,
}

#[derive(serde::Deserialize)]
struct ConsolidateForm {
    dir: String,
    purgatory: Option<String>,
}

const MAX_TABLE_ROWS: usize = 20;
const DEFAULT_FORMAT: &str = "YYYY/MM-DD <folder description>";

enum Msg {
    Progress(usize, usize),
    Done(String),
    Error(String),
}

fn parse_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn to_string(p: impl AsRef<Path>) -> String {
    p.as_ref().display().to_string()
}

fn sse_stream(rx: mpsc::UnboundedReceiver<Msg>) -> impl Stream<Item = Result<Event, Infallible>> {
    UnboundedReceiverStream::new(rx).map(|msg| {
        let (event, data) = match msg {
            Msg::Progress(done, total) => ("progress", format!("{done} {total}")),
            Msg::Done(html) => ("done", html),
            Msg::Error(e) => ("error", e),
        };
        Ok(Event::default().event(event).data(data))
    })
}

async fn consolidate_form() -> ConsolidateFormTemplate {
    ConsolidateFormTemplate {
        title: "DeDupe2",
        page: "consolidate",
    }
}

async fn consolidate_preview(
    Form(form): Form<ConsolidateForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let dir_str = form.dir.trim().to_string();
    if dir_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide a year folder".into()));
    }
    let dir = PathBuf::from(&dir_str);
    let purgatory = form
        .purgatory
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        match preview_consolidate_events_with_progress(&dir, purgatory.as_deref(), &progress) {
            Ok(plan) => {
                let html = render_consolidate_preview(&plan, &dir_str);
                let _ = done_tx.send(Msg::Done(html));
            }
            Err(e) => {
                let _ = done_tx.send(Msg::Error(e.to_string()));
            }
        }
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

async fn consolidate_run(
    Form(form): Form<ConsolidateForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let dir_str = form.dir.trim().to_string();
    if dir_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide a year folder".into()));
    }
    let dir = PathBuf::from(&dir_str);
    let purgatory = form
        .purgatory
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    let purgatory_resolved = match &purgatory {
        Some(p) => p.clone(),
        None => default_event_purgatory(&dir),
    };

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();
    let purgatory_render = purgatory_resolved.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        match consolidate_events_with_progress(&dir, purgatory.as_deref(), &progress) {
            Ok(outcome) => {
                let html = render_consolidate_done(&outcome, &purgatory_render);
                let _ = done_tx.send(Msg::Done(html));
            }
            Err(e) => {
                let _ = done_tx.send(Msg::Error(e.to_string()));
            }
        }
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

async fn compare_form() -> CompareFormTemplate {
    CompareFormTemplate {
        title: "DeDupe2",
        page: "compare",
    }
}

async fn compare_run(
    Form(form): Form<CompareForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let a_str = form.a.trim().to_string();
    let b_str = form.b.trim().to_string();
    if a_str.is_empty() || b_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide both trees".into()));
    }
    let a = PathBuf::from(&a_str);
    let b = PathBuf::from(&b_str);

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        match compare_folders_with_progress(&a, &b, &progress) {
            Ok(cmp) => {
                let html = render_compare(&cmp, &a_str, &b_str);
                let _ = done_tx.send(Msg::Done(html));
            }
            Err(e) => {
                let _ = done_tx.send(Msg::Error(e.to_string()));
            }
        }
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

async fn compare_exif(
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
        let dated = scan_exif_with_progress(&originals, &progress);
        let dates_input = dated
            .iter()
            .map(|d| format!("{}\t{}", to_string(&d.path), d.creation_date))
            .collect::<Vec<_>>()
            .join("\n");
        let html =
            render_compare_exif(&dated, &destination_root, &format, &originals_input, &dates_input);
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
        return Err((StatusCode::BAD_REQUEST, "provide a destination folder".into()));
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

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        match transfer_originals_with_progress(&originals, &destination, &format, op, &progress) {
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
    dated: &[dedupe2::Scanner::compare::DatedOriginal],
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
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

fn render_compare_copied(outcome: &CopyOutcome, destination: &Path, format: &str) -> String {
    let tpl = CompareCopiedTemplate {
        copied: outcome.copied,
        skipped: outcome.skipped_no_exif,
        destination: to_string(destination),
        format: format.to_string(),
    };
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
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
    let result = Command::new("explorer").arg(format!("/select,{}", path)).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open")
        .arg(std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("/")))
        .spawn();
    #[cfg(target_os = "windows")]
    let _ = &result;
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = &result;

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, format!("reveal failed: {e}"))),
    }
}

async fn compare_deep(
    Form(form): Form<DeepForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let mut pairs = Vec::new();
    for line in form.pairs.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let a = parts.next().unwrap_or("").trim();
        let b = parts.next().unwrap_or("").trim();
        if !a.is_empty() && !b.is_empty() {
            pairs.push(DupPair {
                a: PathBuf::from(a),
                b: PathBuf::from(b),
            });
        }
    }
    if pairs.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no duplicate pairs to scan".into()));
    }

    let destination = form
        .destination
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
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
        let outcome = deep_scan_pairs_with_progress(&pairs, &progress);

        // False positives (b side) join the originals list.
        let mut originals: Vec<String> = parse_paths(&originals_input)
            .into_iter()
            .map(|p| to_string(&p))
            .collect();
        for pair in &outcome.removed {
            let b = to_string(&pair.b);
            if !originals.contains(&b) {
                originals.push(b);
            }
        }
        originals.sort();
        let originals_input = originals.join("\n");
        let valid_count = originals.len();

        let html = render_deep_scan(&outcome, &originals_input, &destination, &format, valid_count);
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

fn render_deep_scan(
    outcome: &dedupe2::Scanner::compare::DeepScanOutcome,
    originals_input: &str,
    destination: &str,
    format: &str,
    valid_count: usize,
) -> String {
    let removed: Vec<DeepPairView> = outcome
        .removed
        .iter()
        .map(|p| DeepPairView {
            a: to_string(&p.a),
            b: to_string(&p.b),
        })
        .collect();

    let tpl = DeepScanTemplate {
        checked: outcome.checked,
        kept: outcome.kept,
        removed_count: outcome.removed.len(),
        removed,
        originals_input: originals_input.to_string(),
        destination: destination.to_string(),
        format: format.to_string(),
        valid_count,
    };
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

async fn verify_form() -> VerifyFormTemplate {
    VerifyFormTemplate {
        title: "DeDupe2",
        page: "verify",
    }
}

async fn verify_run(
    Form(form): Form<VerifyForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let root_str = form.root.trim().to_string();
    if root_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide the destination tree".into()));
    }
    let root = PathBuf::from(&root_str);

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        match verify_tree_dates_with_progress(&root, &progress) {
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
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

async fn verify_rehome(
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
        return Err((StatusCode::BAD_REQUEST, "missing destination tree root".into()));
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
        let plans = plan_rehome(&mismatches, &root, &format);
        let outcome = rehome_verified(&plans, &progress);

        // Prove it: re-verify the tree after the re-home.
        let reverified = match verify_tree_dates_with_progress(&root, &progress) {
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

fn render_rehome(
    outcome: &RehomeOutcome,
    reverified: &VerifyOutcome,
    root: &Path,
) -> String {
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
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

async fn sets_form() -> SetsFormTemplate {
    SetsFormTemplate {
        title: "DeDupe2",
        page: "sets",
    }
}

async fn sets_run(
    Form(form): Form<SetForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let a_str = form.a.trim().to_string();
    if a_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide the destination tree (A)".into()));
    }
    let candidates = parse_paths(&form.candidates);
    if candidates.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide at least one candidate tree".into()));
    }
    let a = PathBuf::from(&a_str);

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        match compare_sets_with_progress(&a, &candidates, &progress) {
            Ok(cmp) => {
                let html = render_sets(&cmp, &a_str);
                let _ = done_tx.send(Msg::Done(html));
            }
            Err(e) => {
                let _ = done_tx.send(Msg::Error(e.to_string()));
            }
        }
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

fn render_sets(cmp: &SetComparison, a: &str) -> String {
    let candidates: Vec<SetTreeRowView> = cmp
        .candidates
        .iter()
        .map(|t| SetTreeRowView {
            root: to_string(&t.root),
            total: t.total,
            duplicates: t.duplicates,
            unique_to_tree: t.unique_to_tree,
            shared_with_candidates: t.shared_with_candidates,
            unreadable: t.unreadable,
        })
        .collect();

    let tpl = SetsResultTemplate {
        a: a.to_string(),
        a_total: cmp.a_total,
        a_unique: cmp.a_unique,
        a_unreadable: cmp.a_unreadable,
        duplicates: cmp.duplicates,
        candidates_only: cmp.candidates_only,
        candidates,
    };
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

fn render_compare(cmp: &Comparison, a: &str, b: &str) -> String {
    let mut dup_rows: Vec<DupRowView> = Vec::new();
    for g in &cmp.duplicates {
        for p in &g.a {
            dup_rows.push(DupRowView {
                tree: "A".into(),
                badge: "ok".into(),
                row_class: "row-a".into(),
                path: to_string(p),
            });
        }
        for p in &g.b {
            dup_rows.push(DupRowView {
                tree: "B".into(),
                badge: "dirty".into(),
                row_class: "row-b".into(),
                path: to_string(p),
            });
        }
    }
    let dup_total = dup_rows.len();

    let dup_pairs_all: Vec<String> = cmp
        .duplicates
        .iter()
        .flat_map(|g| {
            g.a.iter()
                .flat_map(|a| g.b.iter().map(move |b| (to_string(a), to_string(b))))
                .collect::<Vec<_>>()
        })
        .map(|(a, b)| format!("{a}\t{b}"))
        .collect();
    let dup_pairs_all = dup_pairs_all.join("\n");

    let a_rows: Vec<PathRowView> = cmp
        .a_only
        .iter()
        .map(|p| PathRowView { path: to_string(p) })
        .collect();

    let b_rows: Vec<PathRowView> = cmp
        .b_only
        .iter()
        .map(|p| PathRowView { path: to_string(p) })
        .collect();

    let (a_rows, a_more) = truncate_rows(a_rows);
    let (dup_rows, dup_more) = truncate_rows(dup_rows);
    let (b_rows, b_more) = truncate_rows(b_rows);

    let unreadable_rows: Vec<PathRowView> = cmp
        .unreadable
        .iter()
        .map(|p| PathRowView { path: to_string(p) })
        .collect();
    let (unreadable_rows, unreadable_more) = truncate_rows(unreadable_rows);

    let originals_all = cmp
        .b_only
        .iter()
        .map(|p| to_string(p))
        .collect::<Vec<_>>()
        .join("\n");

    let tpl = CompareResultTemplate {
        a: a.to_string(),
        b: b.to_string(),
        a_rows,
        a_more,
        dup_rows,
        dup_more,
        b_rows,
        b_more,
        a_only_count: cmp.a_only.len(),
        b_only_count: cmp.b_only.len(),
        dup_total,
        dup_pairs_all,
        unreadable_rows,
        unreadable_more,
        unreadable_count: cmp.unreadable.len(),
        originals_all,
    };
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

fn truncate_rows<T>(items: Vec<T>) -> (Vec<T>, usize) {
    if items.len() > MAX_TABLE_ROWS {
        let more = items.len() - MAX_TABLE_ROWS;
        (items.into_iter().take(MAX_TABLE_ROWS).collect(), more)
    } else {
        (items, 0)
    }
}

fn render_consolidate_preview(plan: &ConsolidationPlan, dir: &str) -> String {
    let mut entries = Vec::new();
    let mut purged_total = 0usize;
    for e in &plan.entries {
        let header = if e.folded.is_empty() {
            e.keeper.clone()
        } else {
            format!("{} \u{2192} {}", e.folded.join(" + "), e.keeper)
        };
        let purged: Vec<String> = e.purged.iter().map(|m| to_string(&m.source)).collect();
        purged_total += purged.len();
        entries.push(ConsolidateEntryView {
            header,
            moved_count: e.moved.len(),
            purged,
        });
    }

    let tpl = ConsolidatePreviewTemplate {
        dir: dir.to_string(),
        purgatory: to_string(&plan.purgatory),
        foldings: entries.len(),
        purged_total,
        entries,
    };
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

fn render_consolidate_done(outcome: &ConsolidationOutcome, purgatory: &Path) -> String {
    let tpl = ConsolidateDoneTemplate {
        folded: outcome.folded_groups,
        moved: outcome.moved_files,
        purged: outcome.purged_files,
        purgatory: to_string(purgatory),
    };
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dedupe2=debug,tower_http=debug".into()),
        )
        .init();

    let app = Router::new()
        .route("/", get(consolidate_form))
        .route("/consolidate", get(consolidate_form))
        .route("/consolidate/preview", post(consolidate_preview))
        .route("/consolidate/run", post(consolidate_run))
        .route("/compare", get(compare_form))
        .route("/compare/run", post(compare_run))
        .route("/compare/exif", post(compare_exif))
        .route("/compare/deep", post(compare_deep))
        .route("/reveal", get(reveal))
        .route("/compare/copy", post(compare_copy))
        .route("/sets", get(sets_form))
        .route("/sets/run", post(sets_run))
        .route("/verify", get(verify_form))
        .route("/verify/run", post(verify_run))
        .route("/verify/rehome", post(verify_rehome))
        .nest_service("/static", ServeDir::new("static"))
        // Compare forms post the full originals list back to the server (one
        // path per line) — several MB for large candidate trees. NOTE: this
        // layer must come after the routes or it applies to nothing.
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("failed to bind to 127.0.0.1:3000");

    tracing::info!("listening on http://{}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.unwrap();
}
