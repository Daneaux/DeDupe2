//! Only the app's own pages may talk to the server.
//!
//! The server binds to loopback, but any web page open in the same browser
//! can still send requests to `http://127.0.0.1:<port>`: a cross-site form
//! POST could start file moves, and DNS rebinding could make a foreign
//! hostname resolve to loopback. Every request is therefore checked:
//!
//! - `Host` must be a loopback name (`localhost`, `127.0.0.1`, `[::1]`);
//! - `Sec-Fetch-Site`, when the browser sends it, must be `same-origin` or
//!   `none` (typed URL / bookmark) — `same-site` is rejected too, since a
//!   different local port is a different app;
//! - `Origin`, when present, must be exactly this server's origin.
//!
//! Requests with none of these headers (curl, in-process tests) pass:
//! browsers always send `Host`, and send `Origin` on every cross-origin
//! POST, so the attacks above cannot omit them.

use axum::extract::Request;
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub(super) async fn same_origin_only(request: Request, next: Next) -> Response {
    match check(request.headers()) {
        Ok(()) => next.run(request).await,
        Err(reason) => {
            tracing::warn!("rejected request to {}: {reason}", request.uri());
            (StatusCode::FORBIDDEN, reason).into_response()
        }
    }
}

fn check(headers: &HeaderMap) -> Result<(), &'static str> {
    let text = |name: &str| headers.get(name).map(|v| v.to_str().unwrap_or("\u{0}"));

    let host = text(HOST.as_str());
    if let Some(host) = host {
        if !is_loopback_authority(host) {
            return Err("forbidden: unexpected Host header");
        }
    }

    if let Some(site) = text("sec-fetch-site") {
        if !matches!(site, "same-origin" | "none") {
            return Err("forbidden: cross-site request");
        }
    }

    if let Some(origin) = text(ORIGIN.as_str()) {
        let Some(authority) = origin.strip_prefix("http://") else {
            return Err("forbidden: foreign origin");
        };
        let same = match host {
            Some(host) => authority.eq_ignore_ascii_case(host),
            None => is_loopback_authority(authority),
        };
        if !same {
            return Err("forbidden: foreign origin");
        }
    }

    Ok(())
}

/// `host[:port]` naming this machine's loopback interface.
fn is_loopback_authority(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        match rest.split_once(']') {
            Some((host, tail)) if tail.is_empty() || is_port(tail) => host,
            _ => return false,
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if is_port(&format!(":{port}")) => host,
            Some(_) => return false,
            None => authority,
        }
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

/// `:<digits>`.
fn is_port(tail: &str) -> bool {
    tail.strip_prefix(':')
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn loopback_authorities() {
        for ok in [
            "localhost",
            "localhost:3000",
            "LOCALHOST:1",
            "127.0.0.1:3000",
            "[::1]:3000",
            "[::1]",
        ] {
            assert!(is_loopback_authority(ok), "{ok}");
        }
        for bad in [
            "evil.com",
            "evil.com:3000",
            "127.0.0.1.evil.com:3000",
            "localhost.evil.com",
            "10.0.0.1:3000",
            "localhost:",
            "localhost:abc",
            "[::1]x",
            "",
        ] {
            assert!(!is_loopback_authority(bad), "{bad}");
        }
    }

    #[test]
    fn the_apps_own_requests_pass() {
        assert!(check(&headers(&[])).is_ok(), "no headers: curl / tests");
        assert!(check(&headers(&[("host", "127.0.0.1:3000")])).is_ok());
        assert!(check(&headers(&[
            ("host", "localhost:3000"),
            ("sec-fetch-site", "same-origin"),
            ("origin", "http://localhost:3000"),
        ]))
        .is_ok());
        assert!(
            check(&headers(&[
                ("host", "127.0.0.1:3000"),
                ("sec-fetch-site", "none")
            ]))
            .is_ok(),
            "typed URL"
        );
    }

    #[test]
    fn cross_site_requests_are_rejected() {
        let cases: &[&[(&'static str, &str)]] = &[
            &[("host", "127.0.0.1:3000"), ("sec-fetch-site", "cross-site")],
            &[("host", "127.0.0.1:3000"), ("sec-fetch-site", "same-site")],
            &[("host", "127.0.0.1:3000"), ("origin", "http://evil.com")],
            &[
                ("host", "127.0.0.1:3000"),
                ("origin", "http://127.0.0.1:8080"),
            ],
            &[("host", "127.0.0.1:3000"), ("origin", "null")],
            &[
                ("host", "127.0.0.1:3000"),
                ("origin", "https://127.0.0.1:3000"),
            ],
            &[("origin", "http://evil.com")],
        ];
        for case in cases {
            assert!(check(&headers(case)).is_err(), "{case:?}");
        }
    }

    #[test]
    fn dns_rebinding_hosts_are_rejected() {
        assert!(check(&headers(&[("host", "attacker.example:3000")])).is_err());
        assert!(check(&headers(&[
            ("host", "attacker.example:3000"),
            ("origin", "http://attacker.example:3000"),
            ("sec-fetch-site", "same-origin"),
        ]))
        .is_err());
    }
}
