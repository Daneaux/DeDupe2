use std::convert::Infallible;
use std::path::{Path, PathBuf};

use askama::Template;
use axum::extract::{DefaultBodyLimit, Form};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{routing::get, routing::post, Router};
use dedupe2::filemover::{
    consolidate_events_with_progress, default_event_purgatory,
    preview_consolidate_events_with_progress, ConsolidationOutcome, ConsolidationPlan,
};
use dedupe2::Scanner::compare::{
    compare_folders_with_progress, scan_exif_with_progress, Comparison,
};
use dedupe2::filemover::{destination_for, transfer_originals_with_progress, CopyOutcome};
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
    unreadable_rows: Vec<PathRowView>,
    unreadable_more: usize,
    unreadable_count: usize,
    originals_all: String,
}

#[derive(Template)]
#[template(path = "compare_exif.html")]
struct CompareExifTemplate {
    a: String,
    originals_input: String,
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
struct ExifForm {
    originals: String,
    a: Option<String>,
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

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();
    let originals_input = form.originals.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        let dated = scan_exif_with_progress(&originals, &progress);
        let html = render_compare_exif(&dated, &a_default, &originals_input);
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
    a: &str,
    originals_input: &str,
) -> String {
    let destination_root = PathBuf::from(a);
    let rows: Vec<ExifRowView> = dated
        .iter()
        .map(|d| {
            let valid = matches!(d.creation_date, CreationDate::DateCreated(_));
            let destination = if valid {
                destination_for(&d.path, &d.creation_date, &destination_root, DEFAULT_FORMAT)
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
        a: a.to_string(),
        originals_input: originals_input.to_string(),
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
        .route("/compare/copy", post(compare_copy))
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
