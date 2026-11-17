//! Compare tab: two-tree comparison and the deep (full-content) confirmation pass.

use std::convert::Infallible;
use std::path::PathBuf;

use askama::Template;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::mpsc;
use tokio_stream::Stream;

use crate::web::common::{
    parse_paths, require_dir, require_disjoint_trees, sse_stream, to_string, DupRowView, Msg,
    PathRowView, DEFAULT_FORMAT,
};
use crate::web::AppState;
use crate::Scanner::compare::{
    compare_folders_cached, deep_scan_pairs_cached, Comparison, DupPair,
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/compare", get(compare_form))
        .route("/compare/run", post(compare_run))
        .route("/compare/deep", post(compare_deep))
}

#[derive(Template)]
#[template(path = "compare.html")]
struct CompareFormTemplate {
    title: &'static str,
    page: &'static str,
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
    unreadable_count: usize,
    /// Unreadable files sitting in a date-named folder: still placeable, so
    /// they are folded into the originals list (dated by folder proxy).
    unreadable_dated_count: usize,
    originals_all: String,
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
    /// `keeper<TAB>duplicate` lines for the purgatory form.
    confirmed_pairs_input: String,
    b_root: String,
}

struct DeepPairView {
    pub a: String,
    pub b: String,
    pub reason: String,
}

#[derive(serde::Deserialize)]
struct CompareForm {
    a: String,
    b: String,
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

    let confirmed_pairs_input = outcome
        .confirmed
        .iter()
        .map(|pair| format!("{}\t{}", to_string(&pair.a), to_string(&pair.b)))
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
        confirmed_pairs_input,
        b_root: b_root.to_string(),
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
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

    let unreadable_all: Vec<String> = cmp.unreadable.iter().map(to_string).collect();
    let unreadable_json = serde_json::to_string(&unreadable_all).unwrap_or_else(|_| "[]".into());

    // Unreadable files (decoder failed on both extension and content sniff)
    // that live in a date-named folder are still placeable: the folder proxy
    // gives them a date, so they join the originals and flow through the
    // EXIF scan like everything else. Without a folder date they stay out.
    let mut originals: Vec<String> = cmp.b_only.iter().map(to_string).collect();
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

    let tpl = CompareResultTemplate {
        a: a.to_string(),
        b: b.to_string(),
        cache_reused,
        cache_computed,
        a_only_count: cmp.a_only.len(),
        b_only_count: cmp.b_only.len(),
        dup_total,
        dup_pairs_all,
        unreadable_count: cmp.unreadable.len(),
        unreadable_dated_count,
        originals_all,
        a_json,
        b_json,
        dup_json,
        unreadable_json,
    };
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
}
