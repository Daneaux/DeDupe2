//! HTTP layer: form parsing, SSE progress streaming, and template
//! rendering. Pure translation between axum and the library APIs —
//! exposed as `app()` so handler-level tests can drive the full router.

use std::collections::{HashMap, HashSet};

pub mod settings;
use std::convert::Infallible;
use std::path::{Path, PathBuf};

use askama::Template;
use axum::extract::{DefaultBodyLimit, Form, Query, State};
use axum::response::Html;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use axum::{routing::get, routing::post, Router};
use crate::filemover::{
    consolidate_events_with_progress, default_event_purgatory,
    preview_consolidate_events_with_progress, ConsolidationOutcome, ConsolidationPlan,
};
use crate::Scanner::compare::{
    compare_folders_cached, compare_sets_cached, deep_scan_pairs_cached, verify_tree_dates_cached,
    Comparison, DupPair, SetComparison, VerifyOutcome,
};
use crate::Scanner::similar::{
    find_similar_between_with_progress, find_similar_with_progress, SimilarCompareOutcome,
    SimilarOutcome,
};
use crate::filemover::{
    destination_for, move_to_purgatory_with_progress, organize_into_destination_with_progress,
    plan_rehome, rehome_verified, swap_better_copies_with_progress,
    transfer_dated_with_progress, transfer_files_with_progress, CopyOutcome, RehomeOutcome,
};
use std::process::Command;

use crate::exif::{creation_date, CreationDate};
use crate::filemover::Operation;
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
    cache_reused: usize,
    cache_computed: usize,
    /// Full result sets for the client-side filter (JSON, rendered |safe).
    a_json: String,
    b_json: String,
    dup_json: String,
    unreadable_json: String,
    a_only_count: usize,
    b_only_count: usize,
    dup_total: usize,
    dup_pairs_all: String,
    dup_b_files_all: String,
    unreadable_count: usize,
    /// Unreadable files sitting in a date-named folder: still placeable, so
    /// they are folded into the originals list (dated by folder proxy).
    unreadable_dated_count: usize,
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

#[derive(Template)]
#[template(path = "similar.html")]
struct SimilarFormTemplate {
    title: &'static str,
    page: &'static str,
}

#[derive(serde::Deserialize)]
struct SimilarForm {
    root: String,
    /// Optional second tree: when set, similar images are matched BETWEEN
    /// the two trees instead of within one.
    compare: Option<String>,
}

#[derive(Template)]
#[template(path = "similar_compare.html")]
struct SimilarCompareTemplate {
    a_root: String,
    b_root: String,
    cache_reused: usize,
    cache_computed: usize,
    scanned_a: usize,
    scanned_b: usize,
    lossy_a: usize,
    lossy_b: usize,
    match_count: usize,
    removable_count: usize,
    swap_count: usize,
    duplicate_count: usize,
    /// Full match rows for the client-side filter (JSON, rendered |safe).
    rows_json: String,
    /// Full exact-duplicate rows for the client-side filter.
    duplicates_json: String,
    duplicates_input: String,
    candidates_input: String,
    /// `a_path<TAB>b_path` lines for the swap form (B is the bigger file).
    swaps_input: String,
}



#[derive(Template)]
#[template(path = "similar_result.html")]
struct SimilarResultTemplate {
    root: String,
    cache_reused: usize,
    cache_computed: usize,
    scanned: usize,
    lossy: usize,
    group_count: usize,
    candidate_count: usize,
    removable_count: usize,
    duplicate_count: usize,
    /// Full group rows for the client-side filter (JSON, rendered |safe).
    rows_json: String,
    /// Full exact-duplicate rows for the client-side filter.
    duplicates_json: String,
    /// Removable candidates, one per line — the purgatory move form.
    candidates_input: String,
    /// Exact-duplicate extras, one per line — their purgatory move form.
    duplicates_input: String,
}

#[derive(serde::Deserialize)]
struct SwapForm {
    /// One `a_path<TAB>b_path` pair per line (A's worse copy, B's better copy).
    swaps: String,
    a_root: String,
    b_root: String,
    purgatory: String,
}

#[derive(Template)]
#[template(path = "similar_swap_done.html")]
struct SimilarSwapTemplate {
    a_root: String,
    b_root: String,
    purgatory: String,
    planned: usize,
    swapped: usize,
    purged: usize,
    failure_count: usize,
    failures: Vec<(String, String)>,
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
    cache_reused: usize,
    cache_computed: usize,
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
    b: Option<String>,
}

#[derive(Template)]
#[template(path = "deep_scan.html")]
struct DeepScanTemplate {
    cache_reused: usize,
    cache_computed: usize,
    checked: usize,
    kept: usize,
    covered: usize,
    removed_count: usize,
    removed: Vec<DeepPairView>,
    originals_input: String,
    destination: String,
    format: String,
    valid_count: usize,
    confirmed_b_input: String,
    b_root: String,
}

struct DeepPairView {
    pub a: String,
    pub b: String,
    pub reason: String,
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

/// Resolve a path for comparison: canonicalize the deepest existing
/// ancestor (resolving symlinks, e.g. a symlinked `4tbext`) and keep the
/// non-existent tail attached, so a not-yet-created destination can still be
/// compared against an existing source.
fn resolved_path(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !current.exists() {
        match current.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => break,
        }
        if !current.pop() {
            break;
        }
    }
    let mut resolved = std::fs::canonicalize(&current).unwrap_or(current);
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    resolved
}

/// Whether one tree contains the other (equal counts as contained).
fn trees_nested(a: &Path, b: &Path) -> Option<(&'static str, PathBuf, PathBuf)> {
    let (ra, rb) = (resolved_path(a), resolved_path(b));
    if ra == rb {
        return Some(("", ra, rb));
    }
    if ra.starts_with(&rb) {
        return Some(("A is inside B", ra, rb));
    }
    if rb.starts_with(&ra) {
        return Some(("B is inside A", ra, rb));
    }
    None
}

/// Two-tree operations compare trees against each other, so nesting them
/// makes the intersection compare against itself (self-pairs, empty
/// originals). Reject before any scanning.
fn require_disjoint_trees(a: &Path, b: &Path) -> Result<(), (StatusCode, String)> {
    match trees_nested(a, b) {
        None => Ok(()),
        Some(("", _, _)) => Err((
            StatusCode::BAD_REQUEST,
            format!("the two trees are the same: {}", a.display()),
        )),
        Some((which, ra, rb)) => Err((
            StatusCode::BAD_REQUEST,
            format!(
                "the trees overlap ({which}) — pick disjoint trees: {} vs {}",
                ra.display(),
                rb.display()
            ),
        )),
    }
}

/// Input trees must exist before a scan starts — otherwise the failure
/// surfaces only after the (long) scan of the other tree (or worse, silently
/// scans nothing thanks to the resilient collector).
fn require_dir(path: &Path) -> Result<(), (StatusCode, String)> {
    if path.is_dir() {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            format!("not a directory: {}", path.display()),
        ))
    }
}

/// Resolve the transfer dates for a set of paths from the form's carried
/// dates: `unknown`-marked paths stay unknown (no re-read), paths not seen by
/// the scan fall back to extraction.
fn dated_from_form(
    files: &[PathBuf],
    carried: &HashMap<String, CreationDate>,
    carried_unknown: &HashSet<String>,
) -> Vec<(PathBuf, CreationDate)> {
    files
        .iter()
        .map(|path| {
            let key = to_string(path);
            let date = if let Some(date) = carried.get(&key) {
                date.clone()
            } else if carried_unknown.contains(&key) {
                CreationDate::Unknown
            } else {
                creation_date(path)
            };
            (path.clone(), date)
        })
        .collect()
}

/// Parse the hidden `dates` field. Returns `path -> date` for dated files and
/// the set of paths the scan already resolved as `unknown` — those are known,
/// just undatable, and must NOT trigger a fallback re-extraction (rawler on
/// each of them is a silent, minutes-long stall before the first copy).
fn parse_carried_dates(input: &str) -> (HashMap<String, CreationDate>, HashSet<String>) {
    let mut dated = HashMap::new();
    let mut unknown = HashSet::new();
    for line in input.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '\t');
        let path = parts.next().unwrap_or("").trim();
        let date = parts.next().unwrap_or("").trim();
        if path.is_empty() || date.is_empty() {
            continue;
        }
        if date.eq_ignore_ascii_case("unknown") {
            unknown.insert(path.to_string());
        } else {
            dated.insert(path.to_string(), CreationDate::DateCreated(date.to_string()));
        }
    }
    (dated, unknown)
}

struct PathRowView {
    pub path: String,
}

struct DupRowView {
    pub tree: String,
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
    State(state): State<AppState>,
    Form(form): Form<CompareForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let a_str = form.a.trim().to_string();
    let b_str = form.b.trim().to_string();
    if a_str.is_empty() || b_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide both trees".into()));
    }
    let a = PathBuf::from(&a_str);
    let b = PathBuf::from(&b_str);
    require_dir(&a)?;
    require_dir(&b)?;
    require_disjoint_trees(&a, &b)?;

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        tracing::info!("compare scan: {} vs {}", a.display(), b.display());
        let before = crate::Scanner::cache::lock(&state.cache).stats();
        match compare_folders_cached(&a, &b, &state.cache, &progress) {
            Ok(cmp) => {
                tracing::info!(
                    "compare scan finished: {} duplicates, {} originals",
                    cmp.duplicates.len(),
                    cmp.b_only.len()
                );
                let after = crate::Scanner::cache::lock(&state.cache).stats();
                let html = render_compare(
                    &cmp,
                    &a_str,
                    &b_str,
                    after.reused.saturating_sub(before.reused),
                    after.computed.saturating_sub(before.computed),
                );
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
        let dates = crate::Scanner::cache::creation_dates_cached(&state.cache, &originals, &progress);
        let dated: Vec<crate::Scanner::compare::DatedOriginal> = originals
            .iter()
            .cloned()
            .zip(dates)
            .map(|(path, creation_date)| crate::Scanner::compare::DatedOriginal {
                path,
                creation_date,
            })
            .collect();
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
    State(state): State<AppState>,
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
    let b_root = form
        .b
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string();

    tracing::info!("deep scan request: {} pairs", pairs.len());
    tracing::debug!("deep pairs:\n{}", form.pairs);

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();
    let originals_input = form.originals.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        progress(0, pairs.len());
        let before = crate::Scanner::cache::lock(&state.cache).stats();
        let outcome = deep_scan_pairs_cached(&pairs, &state.cache, &progress);
        let after = crate::Scanner::cache::lock(&state.cache).stats();
        let cache_reused = after.reused.saturating_sub(before.reused);
        let cache_computed = after.computed.saturating_sub(before.computed);

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

        let html = render_deep_scan(
            &outcome,
            &originals_input,
            &destination,
            &format,
            valid_count,
            &b_root,
            cache_reused,
            cache_computed,
        );
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

fn render_deep_scan(
    outcome: &crate::Scanner::compare::DeepScanOutcome,
    originals_input: &str,
    destination: &str,
    format: &str,
    valid_count: usize,
    b_root: &str,
    cache_reused: usize,
    cache_computed: usize,
) -> String {
    let removed: Vec<DeepPairView> = outcome
        .removed
        .iter()
        .map(|p| DeepPairView {
            a: to_string(&p.a),
            b: to_string(&p.b),
            reason: p.reason.clone(),
        })
        .collect();

    let confirmed_b_input = outcome
        .confirmed_b
        .iter()
        .map(|p| to_string(p))
        .collect::<Vec<_>>()
        .join("\n");

    let tpl = DeepScanTemplate {
        cache_reused,
        cache_computed,
        checked: outcome.checked,
        kept: outcome.kept,
        covered: outcome.covered,
        removed_count: outcome.removed.len(),
        removed,
        originals_input: originals_input.to_string(),
        destination: destination.to_string(),
        format: format.to_string(),
        valid_count,
        confirmed_b_input,
        b_root: b_root.to_string(),
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
    State(state): State<AppState>,
    Form(form): Form<VerifyForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let root_str = form.root.trim().to_string();
    if root_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide the destination tree".into()));
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
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
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
        return Err((StatusCode::BAD_REQUEST, "candidate tree root is missing".into()));
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
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
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
    let coverage = crate::Scanner::cache::lock(&state.cache)
        .branch_coverage(Path::new(query.path.trim()));
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
    let cached_files = crate::Scanner::cache::lock(&state.cache).stats().files;
    let cache_dirty = crate::Scanner::cache::lock(&state.cache).dirty();
    let cache_disk_bytes = cache_path
        .as_deref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());
    let default_path = crate::Scanner::cache::default_cache_path();
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
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
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
        crate::Scanner::cache::default_cache_path()
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
            let mut cache = crate::Scanner::cache::lock(&state.cache);
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
        let mut cache = crate::Scanner::cache::lock(&state.cache);
        *cache = crate::Scanner::cache::ScanCache::new();
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

async fn similar_form() -> SimilarFormTemplate {
    SimilarFormTemplate {
        title: "DeDupe2",
        page: "similar",
    }
}

async fn similar_run(
    State(state): State<AppState>,
    Form(form): Form<SimilarForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let root_str = form.root.trim().to_string();
    if root_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide a tree to scan".into()));
    }
    let root = PathBuf::from(&root_str);

    let compare_root = form
        .compare
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    require_dir(Path::new(&root_str))?;
    if let Some(other) = compare_root.as_deref() {
        require_dir(Path::new(other))?;
        require_disjoint_trees(Path::new(&root_str), Path::new(other))?;
    }

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        let before = crate::Scanner::cache::lock(&state.cache).stats();

        let html = match compare_root {
            Some(other) => {
                let other_root = PathBuf::from(&other);
                let outcome = match find_similar_between_with_progress(
                    &root,
                    &other_root,
                    &state.cache,
                    &progress,
                ) {
                    Ok(outcome) => outcome,
                    Err(e) => {
                        let _ = done_tx.send(Msg::Error(e.to_string()));
                        return;
                    }
                };
                let after = crate::Scanner::cache::lock(&state.cache).stats();
                render_similar_compare(
                    &outcome,
                    &root_str,
                    &other,
                    after.reused.saturating_sub(before.reused),
                    after.computed.saturating_sub(before.computed),
                )
            }
            None => {
                let outcome = match find_similar_with_progress(&root, &state.cache, &progress) {
                    Ok(outcome) => outcome,
                    Err(e) => {
                        let _ = done_tx.send(Msg::Error(e.to_string()));
                        return;
                    }
                };
                let after = crate::Scanner::cache::lock(&state.cache).stats();
                render_similar(
                    &outcome,
                    &root_str,
                    after.reused.saturating_sub(before.reused),
                    after.computed.saturating_sub(before.computed),
                )
            }
        };
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

fn render_similar_compare(
    outcome: &SimilarCompareOutcome,
    a_root: &str,
    b_root: &str,
    cache_reused: usize,
    cache_computed: usize,
) -> String {
    let swap_count = outcome
        .matches
        .iter()
        .filter(|m| !m.removable && m.b_is_better)
        .count();
    let swaps_input = outcome
        .matches
        .iter()
        .filter(|m| !m.removable && m.b_is_better)
        .map(|m| format!("{}\t{}", to_string(&m.a), to_string(&m.b)))
        .collect::<Vec<_>>()
        .join("\n");

    let rows: Vec<serde_json::Value> = outcome
        .matches
        .iter()
        .map(|m| {
            serde_json::json!({
                "a": to_string(&m.a),
                "b": to_string(&m.b),
                "distance": m.distance,
                "verdict": if m.removable {
                    "B is the smaller file — removable"
                } else {
                    "B is the bigger file — swap available"
                },
                "removable": m.removable,
                "a_pixels": m.a_pixels,
                "b_pixels": m.b_pixels,
                "a_bytes": m.a_bytes,
                "b_bytes": m.b_bytes,
            })
        })
        .collect();
    let rows_json = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into());

    let tpl = SimilarCompareTemplate {
        a_root: a_root.to_string(),
        b_root: b_root.to_string(),
        cache_reused,
        cache_computed,
        scanned_a: outcome.scanned_a,
        scanned_b: outcome.scanned_b,
        lossy_a: outcome.lossy_a,
        lossy_b: outcome.lossy_b,
        match_count: outcome.matches.len(),
        removable_count: outcome.removable_b.len(),
        swap_count,
        duplicate_count: outcome.exact_duplicates.len(),
        rows_json,
        duplicates_json: serde_json::to_string(
            &outcome
                .exact_duplicates
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "keeper": to_string(&d.keeper),
                        "path": to_string(&d.path),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into()),
        duplicates_input: outcome
            .exact_duplicates
            .iter()
            .map(|d| to_string(&d.path))
            .collect::<Vec<_>>()
            .join("\n"),
        candidates_input: outcome
            .removable_b
            .iter()
            .map(|p| to_string(p))
            .collect::<Vec<_>>()
            .join("\n"),
        swaps_input,
    };
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

fn render_similar(
    outcome: &SimilarOutcome,
    root: &str,
    cache_reused: usize,
    cache_computed: usize,
) -> String {
    let rows: Vec<serde_json::Value> = outcome
        .groups
        .iter()
        .flat_map(|group| {
            group.candidates.iter().map(move |candidate| {
                serde_json::json!({
                    "keeper": to_string(&group.keeper),
                    "candidate": to_string(&candidate.path),
                    "distance": candidate.distance,
                    "keeper_pixels": group.keeper_pixels,
                    "candidate_pixels": candidate.pixels,
                    "keeper_bytes": group.keeper_bytes,
                    "candidate_bytes": candidate.bytes,
                })
            })
        })
        .collect();
    let rows_json = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into());

    let tpl = SimilarResultTemplate {
        root: root.to_string(),
        cache_reused,
        cache_computed,
        scanned: outcome.scanned,
        lossy: outcome.lossy,
        group_count: outcome.groups.len(),
        candidate_count: outcome.candidates,
        removable_count: outcome.removable.len(),
        duplicate_count: outcome.exact_duplicates.len(),
        rows_json,
        duplicates_json: serde_json::to_string(
            &outcome
                .exact_duplicates
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "keeper": to_string(&d.keeper),
                        "path": to_string(&d.path),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into()),
        duplicates_input: outcome
            .exact_duplicates
            .iter()
            .map(|d| to_string(&d.path))
            .collect::<Vec<_>>()
            .join("\n"),
        candidates_input: outcome
            .removable
            .iter()
            .map(|p| to_string(p))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

/// Copy or move a filtered table selection into one destination folder.
async fn compare_transfer(
    Form(form): Form<TableTransferForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let files = parse_paths(&form.files);
    if files.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no filtered files to transfer".into()));
    }
    let destination = form.destination.trim().to_string();
    if destination.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide a destination folder".into()));
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
        let outcome = transfer_files_with_progress(
            &files,
            Path::new(&destination),
            op,
            &progress,
        );
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
        let html = tpl.render().unwrap_or_else(|e| format!("render error: {e}"));
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

async fn similar_swap(
    Form(form): Form<SwapForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let mut swaps: Vec<(PathBuf, PathBuf)> = Vec::new();
    for line in form.swaps.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let a = parts.next().unwrap_or("").trim();
        let b = parts.next().unwrap_or("").trim();
        if !a.is_empty() && !b.is_empty() {
            swaps.push((PathBuf::from(a), PathBuf::from(b)));
        }
    }
    if swaps.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no swap pairs to run".into()));
    }
    let a_root = PathBuf::from(form.a_root.trim());
    let b_root = PathBuf::from(form.b_root.trim());
    require_dir(&a_root)?;
    require_dir(&b_root)?;
    require_disjoint_trees(&a_root, &b_root)?;
    let purgatory = form.purgatory.trim().to_string();
    if purgatory.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide a purgatory folder".into()));
    }

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        progress(0, swaps.len() * 2);
        let outcome = swap_better_copies_with_progress(
            &swaps,
            &a_root,
            &b_root,
            Path::new(&purgatory),
            &progress,
        );
        let tpl = SimilarSwapTemplate {
            a_root: to_string(&a_root),
            b_root: to_string(&b_root),
            purgatory,
            planned: outcome.planned,
            swapped: outcome.swapped,
            purged: outcome.purged,
            failure_count: outcome.failures.len(),
            failures: outcome
                .failures
                .iter()
                .map(|(path, reason)| (to_string(path), reason.clone()))
                .collect(),
        };
        let html = tpl.render().unwrap_or_else(|e| format!("render error: {e}"));
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
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
        let before = crate::Scanner::cache::lock(&state.cache).stats();
        let dates = crate::Scanner::cache::creation_dates_cached(&state.cache, &files, &progress);
        let after = crate::Scanner::cache::lock(&state.cache).stats();
        let cache_reused = after.reused.saturating_sub(before.reused);
        let cache_computed = after.computed.saturating_sub(before.computed);
        let dated: Vec<crate::Scanner::compare::DatedOriginal> = files
            .iter()
            .cloned()
            .zip(dates)
            .map(|(path, creation_date)| crate::Scanner::compare::DatedOriginal {
                path,
                creation_date,
            })
            .collect();

        let mut rows: Vec<OrganizeRowView> = Vec::new();
        let mut files_input: Vec<String> = Vec::new();
        let mut dates_input: Vec<String> = Vec::new();
        let mut dated_count = 0usize;
        for d in &dated {
            let source = to_string(&d.path);
            let destination_path = destination_for(&d.path, &d.creation_date, Path::new(&destination), &format)
                .map(|p| to_string(p))
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
        let html = tpl.render().unwrap_or_else(|e| format!("render error: {e}"));
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
        let html = tpl.render().unwrap_or_else(|e| format!("render error: {e}"));
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
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
        return Err((StatusCode::BAD_REQUEST, "missing destination tree root".into()));
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
    State(state): State<AppState>,
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
    require_dir(&a)?;
    for candidate in &candidates {
        require_dir(candidate)?;
    }
    // Every tree compared against every other must be disjoint.
    let mut trees: Vec<&PathBuf> = vec![&a];
    trees.extend(candidates.iter());
    for (i, first) in trees.iter().enumerate() {
        for second in trees.iter().skip(i + 1) {
            require_disjoint_trees(first, second)?;
        }
    }

    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let progress_tx = tx.clone();
    let done_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let progress = move |done: usize, total: usize| {
            let _ = progress_tx.send(Msg::Progress(done, total));
        };
        let before = crate::Scanner::cache::lock(&state.cache).stats();
        match compare_sets_cached(&a, &candidates, &state.cache, &progress) {
            Ok(cmp) => {
                let after = crate::Scanner::cache::lock(&state.cache).stats();
                let cache_reused = after.reused.saturating_sub(before.reused);
                let cache_computed = after.computed.saturating_sub(before.computed);
                let html = render_sets(&cmp, &a_str, cache_reused, cache_computed);
                let _ = done_tx.send(Msg::Done(html));
            }
            Err(e) => {
                let _ = done_tx.send(Msg::Error(e.to_string()));
            }
        }
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}

fn render_sets(cmp: &SetComparison, a: &str, cache_reused: usize, cache_computed: usize) -> String {
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
        cache_reused,
        cache_computed,
        a_total: cmp.a_total,
        a_unique: cmp.a_unique,
        a_unreadable: cmp.a_unreadable,
        duplicates: cmp.duplicates,
        candidates_only: cmp.candidates_only,
        candidates,
    };
    tpl.render().unwrap_or_else(|e| format!("render error: {e}"))
}

fn render_compare(
    cmp: &Comparison,
    a: &str,
    b: &str,
    cache_reused: usize,
    cache_computed: usize,
) -> String {
    let mut dup_rows: Vec<DupRowView> = Vec::new();
    for g in &cmp.duplicates {
        for p in &g.a {
            dup_rows.push(DupRowView {
                tree: "A".into(),
                path: to_string(p),
            });
        }
        for p in &g.b {
            dup_rows.push(DupRowView {
                tree: "B".into(),
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

    // Full result sets are embedded as JSON so the filter box can search
    // everything, not just the rows that fit on screen.
    let a_json = serde_json::to_string(&a_rows.iter().map(|r| &r.path).collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".into());
    let b_json = serde_json::to_string(&b_rows.iter().map(|r| &r.path).collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".into());
    let dup_json = serde_json::to_string(
        &dup_rows
            .iter()
            .map(|r| serde_json::json!({ "tree": r.tree, "path": r.path }))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());

    let unreadable_all: Vec<String> = cmp.unreadable.iter().map(|p| to_string(p)).collect();
    let unreadable_json =
        serde_json::to_string(&unreadable_all).unwrap_or_else(|_| "[]".into());

    // Unreadable files (decoder failed on both extension and content sniff)
    // that live in a date-named folder are still placeable: the folder proxy
    // gives them a date, so they join the originals and flow through the
    // EXIF scan like everything else. Without a folder date they stay out.
    let mut originals: Vec<String> = cmp.b_only.iter().map(|p| to_string(p)).collect();
    let mut unreadable_dated_count = 0usize;
    for p in &cmp.unreadable {
        if crate::exif::folder_proxy_date(p).is_some() {
            originals.push(to_string(p));
            unreadable_dated_count += 1;
        }
    }
    originals.sort();
    originals.dedup();
    let originals_all = originals.join("\n");

    // Every B-side file in a shallow duplicate group — the purge candidates
    // (deep scan narrows this to confirmed duplicates).
    let dup_b_files_all = cmp
        .duplicates
        .iter()
        .flat_map(|g| g.b.iter().map(|p| to_string(p)))
        .collect::<Vec<_>>()
        .join("\n");

    let tpl = CompareResultTemplate {
        a: a.to_string(),
        b: b.to_string(),
        cache_reused,
        cache_computed,
        a_only_count: cmp.a_only.len(),
        b_only_count: cmp.b_only.len(),
        dup_total,
        dup_pairs_all,
        dup_b_files_all,
        unreadable_count: cmp.unreadable.len(),
        unreadable_dated_count,
        originals_all,
        a_json,
        b_json,
        dup_json,
        unreadable_json,
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

/// Server state: the hierarchical scan cache is shared by every request for
/// the lifetime of the process, so repeated scans reuse what earlier passes
/// already computed.
#[derive(Clone, Default)]
pub struct AppState {
    pub cache: std::sync::Arc<std::sync::Mutex<crate::Scanner::cache::ScanCache>>,
    /// Where the cache is persisted; `None` keeps everything in memory
    /// (tests and throwaway runs). Changeable from the Settings tab.
    pub cache_path: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
    /// Where the settings file lives; `None` skips writing it (tests).
    pub settings_path: Option<PathBuf>,
    /// Guards against overlapping autosaves (a save can take a while on a
    /// large cache; stacking them would starve the scans behind the lock).
    persisting: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    /// In-memory state for tests.
    pub fn in_memory() -> AppState {
        AppState::default()
    }

    /// Production state: settings (then environment, then platform default)
    /// pick the cache location, which is loaded before serving.
    pub fn persistent() -> AppState {
        let settings = settings::Settings::load();
        let path = resolve_cache_path(&settings);
        let cache = crate::Scanner::cache::ScanCache::load(&path);
        AppState {
            cache: std::sync::Arc::new(std::sync::Mutex::new(cache)),
            cache_path: std::sync::Arc::new(std::sync::Mutex::new(Some(path))),
            settings_path: Some(settings::Settings::settings_path()),
            ..AppState::default()
        }
    }

    pub fn cache_path(&self) -> Option<PathBuf> {
        self.cache_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Save when there is something new to save. Cheap no-op otherwise, and
    /// never runs two saves at once.
    pub fn persist(&self) {
        use std::sync::atomic::Ordering;

        let Some(path) = self.cache_path() else {
            return;
        };
        if self.persisting.swap(true, Ordering::SeqCst) {
            return; // a save is already in flight
        }
        let result = {
            let mut cache = crate::Scanner::cache::lock(&self.cache);
            if cache.dirty() {
                cache.save(&path)
            } else {
                Ok(())
            }
        };
        self.persisting.store(false, Ordering::SeqCst);
        if let Err(e) = result {
            tracing::warn!("failed to persist scan cache to {}: {e}", path.display());
        }
    }

    /// Move the cache to a new location: the current (live) cache is written
    /// there and all future saves go to the new path.
    pub fn switch_cache_path(&self, new_path: PathBuf) -> std::io::Result<()> {
        let mut cache = crate::Scanner::cache::lock(&self.cache);
        cache.save(&new_path)?;
        *self
            .cache_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(new_path);
        Ok(())
    }
}

/// Resolution order: `$DEDUPE2_CACHE` (for scripts/tests) beats the settings
/// file, which beats the platform default.
fn resolve_cache_path(settings: &settings::Settings) -> PathBuf {
    if let Some(p) = std::env::var_os("DEDUPE2_CACHE") {
        return PathBuf::from(p);
    }
    settings
        .cache_path
        .clone()
        .unwrap_or_else(crate::Scanner::cache::default_cache_path)
}

pub fn app() -> Router {
    app_with_state(AppState::default())
}

pub fn app_with_state(state: AppState) -> Router {
    Router::new()
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
        .route("/settings", get(settings_form))
        .route("/settings/save", post(settings_save))
        .route("/settings/flush", post(settings_flush))
        .route("/settings/clear", post(settings_clear))
        .route("/similar", get(similar_form))
        .route("/similar/swap", post(similar_swap))
        .route("/compare/transfer", post(compare_transfer))
        .route("/similar/run", post(similar_run))
        .route("/organize", get(organize_form))
        .route("/organize/scan", post(organize_scan))
        .route("/organize/run", post(organize_run))
        .route("/compare/purgatory", post(compare_purgatory))
        .route("/cache-status", get(cache_status))
        .nest_service("/static", ServeDir::new("static"))
        // Compare forms post the full originals list back to the server (one
        // path per line) — several MB for large candidate trees. NOTE: this
        // layer must come after the routes or it applies to nothing.
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}
