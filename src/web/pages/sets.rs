//! Sets tab: one tree against several candidate trees.

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
    parse_paths, require_dir, require_disjoint_trees, sse_stream, to_string, Msg,
};
use crate::web::AppState;
use crate::Scanner::compare::{compare_sets_cached, SetComparison};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/sets", get(sets_form))
        .route("/sets/run", post(sets_run))
}

#[derive(Template)]
#[template(path = "sets.html")]
struct SetsFormTemplate {
    title: &'static str,
    page: &'static str,
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
        return Err((
            StatusCode::BAD_REQUEST,
            "provide the destination tree (A)".into(),
        ));
    }
    let candidates = parse_paths(&form.candidates);
    if candidates.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "provide at least one candidate tree".into(),
        ));
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
    tpl.render()
        .unwrap_or_else(|e| format!("render error: {e}"))
}
