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

#[tokio::test]
async fn deep_scan_web_flow_never_removes_byte_identical_pairs() {
    use std::path::Path;

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    // Byte-identical copy in each tree (same image1.JPG bytes).
    let img = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    let a_path = a.join("photo.jpg");
    let b_path = b.join("photo.jpg");
    std::fs::write(&a_path, &img).unwrap();
    std::fs::write(&b_path, &img).unwrap();

    // 1. Compare finds them as a duplicate pair.
    let body = format!(
        "a={}&b={}",
        urlencode(&a.display().to_string()),
        urlencode(&b.display().to_string())
    );
    let res = app().oneshot(form_request("/compare/run", body)).await.unwrap();
    let html = body_string(res.into_body()).await;
    assert!(html.contains("photo.jpg"), "compare should list the pair");

    // 2. Deep scan must CONFIRM it, not remove it.
    let body = format!(
        "pairs={}&originals=&destination={}",
        urlencode(&format!("{}\t{}", a_path.display(), b_path.display())),
        urlencode(&b.display().to_string())
    );
    let res = app()
        .oneshot(form_request("/compare/deep", body))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(
        html.contains("0 removed as false positives")
            && html.contains("1 confirmed duplicates"),
        "byte-identical pair must be confirmed in the deep scan result: {html}"
    );
    assert!(
        !html.contains("Removed false positives"),
        "byte-identical pair must not be removed: {html}"
    );
}

#[tokio::test]
async fn deep_scan_suppresses_cross_rows_and_originals_when_every_b_matches() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let mk = |tail: &[u8]| {
        let mut payload = vec![0xABu8; 70 * 1024];
        payload.extend_from_slice(tail);
        let boxed = |tag: &[u8; 4], payload: &[u8]| {
            let mut v = ((payload.len() as u32) + 8).to_be_bytes().to_vec();
            v.extend_from_slice(tag);
            v.extend_from_slice(payload);
            v
        };
        let ftyp = boxed(b"ftyp", b"isomisom");
        let moov = boxed(b"moov", &[0u8; 128]);
        let mdat = boxed(b"mdat", &payload);
        [ftyp, moov, mdat].concat()
    };
    let v1 = mk(b"AAAA-actual-content");
    let v2 = mk(b"BBBB-different-content");

    for (dir, n1, n2) in [(&a, "a1.mp4", "a2.mp4"), (&b, "b1.mp4", "b2.mp4")] {
        std::fs::write(dir.join(n1), &v1).unwrap();
        std::fs::write(dir.join(n2), &v2).unwrap();
    }

    // Exactly what compare_result.html posts: the full A x B cross product.
    let pairs = [
        format!("{}\t{}", a.join("a1.mp4").display(), b.join("b1.mp4").display()),
        format!("{}\t{}", a.join("a1.mp4").display(), b.join("b2.mp4").display()),
        format!("{}\t{}", a.join("a2.mp4").display(), b.join("b1.mp4").display()),
        format!("{}\t{}", a.join("a2.mp4").display(), b.join("b2.mp4").display()),
    ]
    .join("\n");

    let res = app()
        .oneshot(form_request(
            "/compare/deep",
            format!(
                "pairs={}&originals=&destination={}",
                urlencode(&pairs),
                urlencode(&b.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;

    assert!(html.contains("4 pairs checked"));
    assert!(html.contains("2 confirmed duplicates"));
    assert!(html.contains("2 shallow-only matches already covered"));
    assert!(
        !html.contains("Removed false positives"),
        "cross rows must not surface: {html}"
    );
    // originals list is empty, so the EXIF scan button must be disabled.
    assert!(html.contains("Scan for EXIF") && html.contains("disabled"), "originals must stay empty: {html}");
}
