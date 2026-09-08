use std::convert::Infallible;
use std::path::{Path, PathBuf};

use askama::Template;
use axum::extract::Form;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{routing::get, routing::post, Router};
use dedupe2::filemover::{
    consolidate_events_with_progress, default_event_purgatory,
    preview_consolidate_events_with_progress, ConsolidationOutcome, ConsolidationPlan,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tower_http::services::ServeDir;

#[derive(Template)]
#[template(path = "consolidate.html")]
struct ConsolidateFormTemplate {
    title: &'static str,
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

enum Msg {
    Progress(usize, usize),
    Done(String),
    Error(String),
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
    ConsolidateFormTemplate { title: "DeDupe2" }
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
        .nest_service("/static", ServeDir::new("static"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("failed to bind to 127.0.0.1:3000");

    tracing::info!("listening on http://{}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.unwrap();
}
