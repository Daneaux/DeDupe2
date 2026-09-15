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

#[tokio::test]
async fn purgatory_moves_duplicate_files_into_side_tree() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("candidate");
    let purg = root.path().join("purgatory");
    std::fs::create_dir_all(src.join("2013/07-30")).unwrap();
    std::fs::create_dir_all(src.join("2013/07-31")).unwrap();
    std::fs::create_dir_all(&purg).unwrap();

    let d1 = src.join("2013/07-30/IMG_1.jpg");
    let d2 = src.join("2013/07-31/IMG_2.jpg");
    let keep = src.join("2013/kept.jpg");
    std::fs::write(&d1, b"dup-one").unwrap();
    std::fs::write(&d2, b"dup-two").unwrap();
    std::fs::write(&keep, b"keep").unwrap();

    let files = format!("{}\n{}", d1.display(), d2.display());
    let res = app()
        .oneshot(form_request(
            "/compare/purgatory",
            format!(
                "files={}&b_root={}&purgatory={}",
                urlencode(&files),
                urlencode(&src.display().to_string()),
                urlencode(&purg.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;

    assert!(html.contains("Moved to Purgatory"), "{html}");
    assert!(html.contains("2"), "moved count missing: {html}");
    assert_eq!(std::fs::read(purg.join("2013/07-30/IMG_1.jpg")).unwrap(), b"dup-one");
    assert_eq!(std::fs::read(purg.join("2013/07-31/IMG_2.jpg")).unwrap(), b"dup-two");
    assert!(!src.join("2013/07-30").exists(), "empty dirs pruned");
    assert!(!src.join("2013/07-31").exists());
    assert!(keep.exists());
    assert!(src.exists(), "candidate root kept");
}

#[tokio::test]
async fn purgatory_rejects_empty_inputs() {
    let res = app()
        .oneshot(form_request(
            "/compare/purgatory",
            "files=&b_root=/tmp&purgatory=/tmp".into(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = app()
        .oneshot(form_request(
            "/compare/purgatory",
            "files=/a.js&b_root=/tmp&purgatory=".into(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unreadable_files_in_date_folders_join_originals_and_get_dated() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();

    // Unreadable (corrupt jpeg bytes), but inside a date-named folder.
    let dated = b.join("2013-12-27/garbage.jpg");
    std::fs::create_dir_all(dated.parent().unwrap()).unwrap();
    std::fs::write(&dated, b"\xFF\xD8\xFF\xE0 not really a jpeg").unwrap();

    // Unreadable and no folder date: must stay out of the originals.
    let undated = b.join("misc/garbage2.jpg");
    std::fs::create_dir_all(undated.parent().unwrap()).unwrap();
    std::fs::write(&undated, b"\xFF\xD8\xFF\xE0 also not a jpeg").unwrap();

    let res = app()
        .oneshot(form_request(
            "/compare/run",
            format!(
                "a={}&b={}",
                urlencode(&a.display().to_string()),
                urlencode(&b.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;

    assert!(
        html.contains("unreadable file(s) dated from their folder"),
        "hint missing: {html}"
    );

    // The exif form's originals input must contain the dated file only.
    let marker = "name=\"originals\" value=\"";
    let start = html.find(marker).expect("originals input") + marker.len();
    let end = start + html[start..].find('"').unwrap();
    let originals = &html[start..end];
    assert!(
        originals.contains(&dated.display().to_string()),
        "dated unreadable file must be an original: {originals}"
    );
    assert!(
        !originals.contains(&undated.display().to_string()),
        "undated unreadable file must not be an original: {originals}"
    );

    // And the EXIF pass dates it from the folder.
    let res = app()
        .oneshot(form_request(
            "/compare/exif",
            format!(
                "originals={}&a={}",
                urlencode(originals),
                urlencode(&a.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(
        html.contains("2013-12-27"),
        "folder date must be applied in the exif scan: {html}"
    );
}

#[tokio::test]
async fn copy_uses_carried_dates_and_reextracts_only_missing_ones() {
    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("candidate");
    let library = root.path().join("library");
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::create_dir_all(&library).unwrap();

    // Real EXIF date of this fixture is 2022-08-17.
    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    let a = candidate.join("a.jpg");
    let b = candidate.join("b.jpg");
    std::fs::write(&a, &img).unwrap();
    std::fs::write(&b, &img).unwrap();

    // Only a.jpg has a carried date (deliberately different from its EXIF).
    let dates = format!("{}\t2010-05-30 06:59:32", a.display());

    let body = format!(
        "originals={}&destination={}&format={}&op=copy&dates={}",
        urlencode(&format!("{}\n{}", a.display(), b.display())),
        urlencode(&library.display().to_string()),
        urlencode("YYYY/MM-DD <folder description>"),
        urlencode(&dates)
    );
    let res = app().oneshot(form_request("/compare/copy", body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;
    assert!(html.contains("event: done"), "{html}");

    // Collect what landed where.
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.push(p.strip_prefix(base).unwrap().display().to_string());
            }
        }
    }
    let mut landed = Vec::new();
    walk(&library, &library, &mut landed);

    // a.jpg: carried date wins (2010-05-30), proving no re-extraction.
    assert!(
        landed.iter().any(|p| p.contains("2010/05-30") && p.ends_with("a.jpg")),
        "carried date must be used: {landed:?}"
    );
    // b.jpg: no carried date, so its real EXIF date is extracted.
    assert!(
        landed.iter().any(|p| p.contains("2022/08-17") && p.ends_with("b.jpg")),
        "missing carried date must fall back to extraction: {landed:?}"
    );
}

#[tokio::test]
async fn unknown_carried_dates_skip_extraction_entirely() {
    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("candidate");
    let library = root.path().join("library");
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::create_dir_all(&library).unwrap();

    // This raw fixture has a readable EXIF date. If the copy fell back to
    // extraction it would be placed; marked "unknown" by the scan, it must
    // be skipped without being re-read (that re-read is the silent stall).
    let raw = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/raw-exif-mod/raw_cr2.CR2"),
    )
    .unwrap();
    let raw_path = candidate.join("shot.CR2");
    std::fs::write(&raw_path, &raw).unwrap();

    let dates = format!("{}\tunknown", raw_path.display());
    let body = format!(
        "originals={}&destination={}&format={}&op=copy&dates={}",
        urlencode(&raw_path.display().to_string()),
        urlencode(&library.display().to_string()),
        urlencode("YYYY/MM-DD <folder description>"),
        urlencode(&dates)
    );
    let res = app().oneshot(form_request("/compare/copy", body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;
    assert!(html.contains("event: done"), "{html}");

    // Nothing copied, and the raw was not re-extracted (no destination folder
    // exists at all).
    let landed: Vec<_> = std::fs::read_dir(&library).unwrap().collect();
    assert!(landed.is_empty(), "unknown-marked file must not be placed: {landed:?}");
}
