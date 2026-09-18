use axum::Router;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dedupe2=debug,tower_http=debug".into()),
        )
        .init();

    let state = dedupe2::web::AppState::persistent();

    // Autosave: the scan cache is dirty after every scan that learned
    // something new; write it every 30s so a restart keeps the knowledge.
    let autosave = state.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            let state = autosave.clone();
            let _ = tokio::task::spawn_blocking(move || state.persist()).await;
        }
    });

    let app: Router = dedupe2::web::app_with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("failed to bind to 127.0.0.1:3000");

    tracing::info!("listening on http://{}", listener.local_addr().unwrap());

    tokio::select! {
        result = axum::serve(listener, app) => {
            result.unwrap();
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutting down — persisting scan cache");
            state.persist();
        }
    }
}
