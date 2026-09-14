use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio;
use tower::ServiceExt;

fn app() -> axum::Router {
    dedupe2::web::app()
}

async fn body_string(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn form_request(uri: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

fn write_junk(path: &Path, name: &str) -> String {
    let full = path.join(name);
    std::fs::write(&full, b"\xFF\xD8\xFF\xE0 garbage jpeg-ish bytes").unwrap();
    full.display().to_string()
}

#[tokio::test]
async fn compare_form_renders() {
    let res = app()
        .oneshot(
            Request::builder()
                .uri("/compare")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = body_string(res.into_body()).await;
    assert!(text.contains("compare-form"), "form body should include the compare form");
}

#[tokio::test]
async fn compare_run_streams_progress_then_done() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let a_file = write_junk(&a, "photo1.jpg");
    let _b_file = write_junk(&b, "photo2.jpg");

    let body = format!("a={}&b={}", urlencode(&a_file), urlencode(&b.display().to_string()));
    let res = app().oneshot(form_request("/compare/run", body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let content_type = res.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(
        content_type.contains("text/event-stream"),
        "expected SSE stream, got {content_type}"
    );

    let text = body_string(res.into_body()).await;
    assert!(text.contains("event: done"), "stream must end with a done event: {text}");
    assert!(text.contains("compare-deep-form"), "done event must carry rendered results");
}

#[tokio::test]
async fn compare_run_rejects_empty_trees() {
    let res = app()
        .oneshot(form_request("/compare/run", "a=&b=".into()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deep_scan_without_pairs_is_rejected() {
    let res = app()
        .oneshot(form_request("/compare/deep", "pairs=&originals=".into()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn verify_run_rejects_empty_root() {
    let res = app()
        .oneshot(form_request("/verify/run", "root=".into()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn large_payloads_reach_the_handler_not_the_body_limit() {
    // The 64mb DefaultBodyLimit layer must actually wrap the routes (it is
    // registered after them); a multi-mb form should hit handler logic, not
    // a 413 from a misapplied default 2mb limit.
    let big = "x".repeat(5 * 1024 * 1024);
    let body = format!("a={big}&b={big}");
    let res = app().oneshot(form_request("/compare/run", body)).await.unwrap();
    assert_ne!(
        res.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "5mb payload should pass the 64mb limit"
    );
}

#[tokio::test]
async fn verify_streams_a_result_for_an_empty_but_existent_tree() {
    let root = tempfile::tempdir().unwrap();
    let res = app()
        .oneshot(form_request(
            "/verify/run",
            format!("root={}", urlencode(&root.path().display().to_string())),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = body_string(res.into_body()).await;
    assert!(text.contains("event: done"), "verify stream should complete: {text}");
}

fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            c if c.is_ascii_alphanumeric() || "/.:-_".contains(c) => c.to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}
