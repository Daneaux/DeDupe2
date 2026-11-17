//! Similar tab: perceptual-hash matching within one tree or between two,
//! and the per-row decisions / swaps that act on the matches.

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

use crate::exif::CreationDate;
use crate::filemover::{
    apply_similar_decisions_with_progress, swap_better_copies_with_progress, SimilarDecision,
};
use crate::web::common::{
    require_dir, require_disjoint_trees, sse_stream, to_string, Msg, DEFAULT_FORMAT,
};
use crate::web::AppState;
use crate::Scanner::similar::{
    find_similar_between_with_progress, find_similar_with_progress, SimilarCompareOutcome,
    SimilarOutcome,
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/similar", get(similar_form))
        .route("/similar/run", post(similar_run))
        .route("/similar/decide", post(similar_decide))
        .route("/similar/swap", post(similar_swap))
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
struct DecideForm {
    /// One `a_path<TAB>b_path<TAB>action` line per selected row.
    decisions: String,
    a_root: String,
    b_root: String,
    purgatory: String,
    format: Option<String>,
}

#[derive(Template)]
#[template(path = "similar_decide_done.html")]
struct SimilarDecideTemplate {
    a_root: String,
    b_root: String,
    purgatory: String,
    planned: usize,
    kept_both: usize,
    purged_both: usize,
    bigger_taken: usize,
    failure_count: usize,
    failures: Vec<(String, String)>,
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
                // EXIF dates for the B copies power the per-row destination
                // preview and the keep-both action (cache-backed).
                let b_paths: Vec<PathBuf> = outcome.matches.iter().map(|m| m.b.clone()).collect();
                let dates =
                    crate::Scanner::cache::creation_dates_cached(&state.cache, &b_paths, &progress);
                render_similar_compare(
                    &outcome,
                    &root_str,
                    &other,
                    after.reused.saturating_sub(before.reused),
                    after.computed.saturating_sub(before.computed),
                    &dates,
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
    dates: &[CreationDate],
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
        .enumerate()
        .map(|(i, m)| {
            serde_json::json!({
                "a": to_string(&m.a),
                "b": to_string(&m.b),
                "date": dates.get(i).map(|d| d.to_string()).unwrap_or_else(|| "unknown".into()),
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
            .map(|d| format!("{}\t{}", to_string(&d.keeper), to_string(&d.path)))
            .collect::<Vec<_>>()
            .join("\n"),
        candidates_input: outcome
            .removable_b
            .iter()
            .map(to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        swaps_input,
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
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
            .map(|d| format!("{}\t{}", to_string(&d.keeper), to_string(&d.path)))
            .collect::<Vec<_>>()
            .join("\n"),
        candidates_input: outcome
            .removable
            .iter()
            .map(to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
}

/// Execute the per-row decisions from the similar table.
async fn similar_decide(
    Form(form): Form<DecideForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    let mut decisions: Vec<(PathBuf, PathBuf, SimilarDecision)> = Vec::new();
    for line in form.decisions.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let a = parts.next().unwrap_or("").trim();
        let b = parts.next().unwrap_or("").trim();
        let action = parts.next().unwrap_or("").trim();
        let decision = match action {
            "keep_both" => SimilarDecision::KeepBoth,
            "purgatory_both" => SimilarDecision::PurgatoryBoth,
            "take_bigger" => SimilarDecision::TakeBigger,
            _ => continue,
        };
        if !a.is_empty() && !b.is_empty() {
            decisions.push((PathBuf::from(a), PathBuf::from(b), decision));
        }
    }
    if decisions.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no decisions to apply".into()));
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
        progress(0, decisions.len());
        let outcome = apply_similar_decisions_with_progress(
            &decisions,
            &a_root,
            &b_root,
            Path::new(&purgatory),
            &format,
            &progress,
        );
        let tpl = SimilarDecideTemplate {
            a_root: to_string(&a_root),
            b_root: to_string(&b_root),
            purgatory,
            planned: outcome.planned,
            kept_both: outcome.kept_both,
            purged_both: outcome.purged_both,
            bigger_taken: outcome.bigger_taken,
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
        let html = tpl
            .render()
            .unwrap_or_else(|e| format!("render error: {e}"));
        let _ = done_tx.send(Msg::Done(html));
    });

    Ok(Sse::new(sse_stream(rx)).keep_alive(KeepAlive::default()))
}
