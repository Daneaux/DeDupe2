use axum::Router;

/// Port used when neither `--port` nor `$DEDUPE2_PORT` is given.
const DEFAULT_PORT: u16 = 3000;

/// How often the scan cache is written to disk while the server runs.
const AUTOSAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dedupe2=debug,tower_http=debug".into()),
        )
        .init();

    let port = match port_from(std::env::args().skip(1), std::env::var("DEDUPE2_PORT").ok()) {
        Ok(port) => port,
        Err(e) => {
            eprintln!("{e}\nusage: webService [--port N]   (or set DEDUPE2_PORT)");
            std::process::exit(2);
        }
    };

    let state = dedupe2::web::AppState::persistent();

    // Autosave: the scan cache is dirty after every scan that learned
    // something new; write it periodically so a restart keeps the knowledge.
    let autosave = state.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(AUTOSAVE_EVERY);
        loop {
            ticker.tick().await;
            let state = autosave.clone();
            let _ = tokio::task::spawn_blocking(move || state.persist()).await;
        }
    });

    let app: Router = dedupe2::web::app_with_state(state.clone());

    // Loopback only: the UI moves files on disk and must never be reachable
    // from other machines.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind to {addr}: {e}"));

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

/// `--port N` (or `--port=N`) beats `$DEDUPE2_PORT`, which beats the default.
fn port_from(mut args: impl Iterator<Item = String>, env: Option<String>) -> Result<u16, String> {
    let parse = |value: &str| {
        value
            .trim()
            .parse::<u16>()
            .map_err(|_| format!("invalid port: {value:?}"))
    };
    let Some(arg) = args.next() else {
        return env.map_or(Ok(DEFAULT_PORT), |value| parse(&value));
    };
    let port = if let Some(value) = arg.strip_prefix("--port=") {
        parse(value)?
    } else if arg == "--port" {
        parse(&args.next().ok_or("--port needs a value")?)?
    } else {
        return Err(format!("unknown argument: {arg}"));
    };
    match args.next() {
        Some(extra) => Err(format!("unknown argument: {extra}")),
        None => Ok(port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn port_resolution_order() {
        assert_eq!(port_from(args(&[]), None), Ok(DEFAULT_PORT));
        assert_eq!(port_from(args(&[]), Some("8080".into())), Ok(8080));
        assert_eq!(
            port_from(args(&["--port", "9000"]), Some("8080".into())),
            Ok(9000)
        );
        assert_eq!(port_from(args(&["--port=9001"]), None), Ok(9001));
    }

    #[test]
    fn bad_ports_are_rejected() {
        assert!(port_from(args(&["--port"]), None).is_err());
        assert!(port_from(args(&["--port", "70000"]), None).is_err());
        assert!(port_from(args(&[]), Some("abc".into())).is_err());
        assert!(port_from(args(&["--verbose"]), None).is_err());
        assert!(port_from(args(&["--port", "1", "x"]), None).is_err());
    }
}
