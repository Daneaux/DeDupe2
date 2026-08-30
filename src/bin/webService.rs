use askama::Template;
use axum::{routing::get, Router};
use tower_http::services::ServeDir;
use dedupe2::volumes::{FileType, Volume, VolumeManager};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    title: &'static str,
    volumes: Vec<Volume>,
    file_types: Vec<FileType>,
}

async fn index() -> IndexTemplate {
    let manager = VolumeManager::demo();
    IndexTemplate {
        title: "DeDupe2",
        volumes: manager.volumes().to_vec(),
        file_types: manager.file_types().to_vec(),
    }
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
        .route("/", get(index))
        .nest_service("/static", ServeDir::new("static"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("failed to bind to 127.0.0.1:3000");

    tracing::info!("listening on http://{}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.unwrap();
}
