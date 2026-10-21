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
    let _a_file = write_junk(&a, "photo1.jpg");
    let _b_file = write_junk(&b, "photo2.jpg");

    let body = format!(
        "a={}&b={}",
        urlencode(&a.display().to_string()),
        urlencode(&b.display().to_string())
    );
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

#[tokio::test]
async fn organize_scan_previews_destinations_from_exif() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source/2005");
    let dest = root.path().join("dest");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&dest).unwrap();

    // Fixture EXIF date is 2022-08-17.
    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    let shot = source.join("shot.jpg");
    std::fs::write(&shot, &img).unwrap();

    let res = app()
        .oneshot(form_request(
            "/organize/scan",
            format!(
                "source={}&destination={}&format={}",
                urlencode(&source.display().to_string()),
                urlencode(&dest.display().to_string()),
                urlencode("YYYY/MM-DD <folder description>")
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;

    assert!(html.contains("Organize Preview"), "{html}");
    assert!(html.contains("2022/08-17"), "destination column: {html}");
    assert!(html.contains("Organize 1 files"), "move button: {html}");
    // The hidden files/dates inputs must carry the batch for the run phase.
    assert!(html.contains("shot.jpg"));
    assert!(html.contains("2022-08-17"), "carried date: {html}");
}

#[tokio::test]
async fn organize_run_rejects_duplicates_already_in_destination() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source/2005");
    let dest = root.path().join("dest");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(dest.join("2005/01-10")).unwrap();

    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    let dup = source.join("dup.jpg");
    let fresh = source.join("fresh.jpg");
    std::fs::write(&dup, &img).unwrap();
    std::fs::write(&fresh, b"unique photo bytes").unwrap();
    let existing = dest.join("2005/01-10/already-here.jpg");
    std::fs::write(&existing, &img).unwrap();

    let files = format!("{}\n{}", dup.display(), fresh.display());
    let dates = format!(
        "{}\t2005-01-10 10:00:00\n{}\t2005-01-10 11:00:00",
        dup.display(),
        fresh.display()
    );
    let res = app()
        .oneshot(form_request(
            "/organize/run",
            format!(
                "files={}&dates={}&destination={}&format={}&op=move",
                urlencode(&files),
                urlencode(&dates),
                urlencode(&dest.display().to_string()),
                urlencode("YYYY/MM-DD")
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;

    assert!(html.contains("Organize Complete"), "{html}");
    assert!(html.contains("Rejected duplicates"), "{html}");
    assert!(html.contains("already-here.jpg"), "matched existing file: {html}");
    assert!(dup.exists(), "rejected source stays in the source tree");
    assert!(!fresh.exists(), "fresh file moved");
    assert!(dest.join("2005/01-10/fresh.jpg").exists());
}

/// Static consistency: every page that includes compare.js must define the
/// progress/result element ids the script binds to, and the script's lookup
/// chain must include that page's prefix. Missing this made the organize
/// tab's submit button do nothing (null elements → exception).
#[test]
fn js_progress_bindings_match_every_page_that_uses_them() {
    let js = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static/compare.js"),
    )
    .unwrap();

    let pages = [
        ("templates/compare.html", "compare"),
        ("templates/sets.html", "sets"),
        ("templates/verify.html", "verify"),
        ("templates/organize.html", "organize"),
        ("templates/similar.html", "similar"),
    ];
    for (template, prefix) in pages {
        let html = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(template),
        )
        .unwrap();
        assert!(
            js.contains(&format!("\"{prefix}-\" + suffix")),
            "compare.js pick() must look up {prefix}- ids"
        );
        for suffix in ["progress", "progress-fill", "progress-text", "result"] {
            assert!(
                html.contains(&format!("id=\"{prefix}-{suffix}\"")),
                "{template} is missing id=\"{prefix}-{suffix}\""
            );
        }
    }
}

#[tokio::test]
async fn repeated_compare_indicates_cache_reuse() {
    use dedupe2::web::{app_with_state, AppState};

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    std::fs::write(a.join("one.jpg"), &img).unwrap();
    std::fs::write(b.join("one.jpg"), &img).unwrap();
    std::fs::write(b.join("two.jpg"), b"another photo").unwrap();

    let state = AppState::default();
    let body = format!(
        "a={}&b={}",
        urlencode(&a.display().to_string()),
        urlencode(&b.display().to_string())
    );

    // First run computes; second run must report the reused cache.
    let res = app_with_state(state.clone())
        .oneshot(form_request("/compare/run", body.clone()))
        .await
        .unwrap();
    let first = body_string(res.into_body()).await;
    assert!(!first.contains("reused / 0 computed"), "first run computes: {first}");

    let res = app_with_state(state)
        .oneshot(form_request("/compare/run", body))
        .await
        .unwrap();
    let second = body_string(res.into_body()).await;
    assert!(
        second.contains("cache:") && second.contains("reused / 0 computed"),
        "second run must show a fully reused cache: {second}"
    );
}

#[tokio::test]
async fn sets_reuses_the_compare_folders_cache() {
    use dedupe2::web::{app_with_state, AppState};

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    std::fs::write(a.join("one.jpg"), &img).unwrap();
    std::fs::write(b.join("one.jpg"), &img).unwrap();
    std::fs::write(b.join("two.jpg"), sample_bytes()).unwrap();

    let state = AppState::default();

    // Compare folders first: this hashes A and B.
    let res = app_with_state(state.clone())
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
    let _ = body_string(res.into_body()).await;

    // Compare sets over the same trees must reuse every hash.
    let res = app_with_state(state)
        .oneshot(form_request(
            "/sets/run",
            format!(
                "a={}&candidates={}",
                urlencode(&a.display().to_string()),
                urlencode(&b.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(
        html.contains("cache:") && html.contains("reused / 0 computed"),
        "sets must reuse the compare-folders cache: {html}"
    );
}

fn sample_bytes() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-dupe-diff-size/IMG_2571.jpg"),
    )
    .unwrap()
}

#[tokio::test]
async fn cache_status_reports_scanned_trees() {
    use dedupe2::web::{app_with_state, AppState};

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    std::fs::write(a.join("one.jpg"), &img).unwrap();
    std::fs::write(b.join("one.jpg"), &img).unwrap();

    let state = AppState::default();

    // Nothing scanned yet.
    let res = app_with_state(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/cache-status?path={}",
                    urlencode(&a.display().to_string())
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_string(res.into_body()).await;
    assert!(body.contains("\"cached_files\":0"), "{body}");

    // Scan, then the tree reports cached coverage. The SSE body must be
    // drained so the scan is actually finished before asking.
    let res = app_with_state(state.clone())
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
    let _ = body_string(res.into_body()).await;

    let res = app_with_state(state)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/cache-status?path={}",
                    urlencode(&a.display().to_string())
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_string(res.into_body()).await;
    assert!(body.contains("\"cached_files\":1"), "{body}");
}

#[tokio::test]
async fn cache_status_reports_each_layer_after_scans() {
    use dedupe2::web::{app_with_state, AppState};

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    std::fs::write(a.join("one.jpg"), &img).unwrap();
    std::fs::write(b.join("one.jpg"), &img).unwrap();

    let state = AppState::default();
    let drain = |res: axum::response::Response| async move { body_string(res.into_body()).await };

    let status_for = |state: AppState, path: std::path::PathBuf| async move {
        let res = app_with_state(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/cache-status?path={}", urlencode(&path.display().to_string())))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        body_string(res.into_body()).await
    };

    // Compare: shallow layer only.
    let res = app_with_state(state.clone())
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
    let _ = drain(res).await;
    let body = status_for(state.clone(), a.clone()).await;
    assert!(body.contains("\"shallow\":1"), "{body}");
    assert!(body.contains("\"deep\":0"), "{body}");
    assert!(body.contains("\"exif\":0"), "{body}");

    // Deep scan adds the deep layer.
    let res = app_with_state(state.clone())
        .oneshot(form_request(
            "/compare/deep",
            format!(
                "pairs={}&originals=&destination={}",
                urlencode(&format!(
                    "{}\t{}",
                    a.join("one.jpg").display(),
                    b.join("one.jpg").display()
                )),
                urlencode(&a.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let _ = drain(res).await;
    let body = status_for(state.clone(), a.clone()).await;
    assert!(body.contains("\"shallow\":1"), "{body}");
    assert!(body.contains("\"deep\":1"), "{body}");
    assert!(body.contains("\"exif\":0"), "{body}");

    // EXIF pass adds the date layer.
    let res = app_with_state(state.clone())
        .oneshot(form_request(
            "/compare/exif",
            format!(
                "originals={}&a={}",
                urlencode(&a.join("one.jpg").display().to_string()),
                urlencode(&a.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let _ = drain(res).await;
    let body = status_for(state, a.clone()).await;
    assert!(body.contains("\"shallow\":1"), "{body}");
    assert!(body.contains("\"deep\":1"), "{body}");
    assert!(body.contains("\"exif\":1"), "{body}");
}

#[tokio::test]
async fn similar_tab_finds_scaled_copies_and_offers_purgatory() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("library");
    std::fs::create_dir_all(&tree).unwrap();

    let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    let keeper = tree.join("original.jpg");
    std::fs::copy(&original, &keeper).unwrap();

    // A smaller, recompressed copy of the same picture.
    let img = image::open(&original).unwrap();
    let small = img.resize_exact(
        (img.width() as f32 * 0.5) as u32,
        (img.height() as f32 * 0.5) as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let candidate = tree.join("small.jpg");
    small.save_with_format(&candidate, image::ImageFormat::Jpeg).unwrap();

    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!("root={}", urlencode(&tree.display().to_string())),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;

    assert!(html.contains("Similar Images"), "{html}");
    assert!(html.contains("original.jpg"), "keeper listed: {html}");
    assert!(html.contains("small.jpg"), "candidate listed: {html}");
    assert!(
        html.contains("Move 1 candidates to purgatory"),
        "purgatory form offered: {html}"
    );
    // The purgatory form carries the candidate.
    let marker = "name=\"files\" value=\"";
    let start = html.find(marker).expect("files input") + marker.len();
    let end = start + html[start..].find('"').unwrap();
    assert!(html[start..end].contains("small.jpg"), "{}", &html[start..end]);
}

#[tokio::test]
async fn similar_tab_compare_mode_matches_between_two_trees() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("library");
    let b = root.path().join("candidate");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    let keeper = a.join("original.jpg");
    std::fs::copy(&original, &keeper).unwrap();

    // B holds a half-size recompressed copy of A's original.
    let img = image::open(&original).unwrap();
    let small = img.resize_exact(
        (img.width() as f32 * 0.5) as u32,
        (img.height() as f32 * 0.5) as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let candidate = b.join("small.jpg");
    small.save_with_format(&candidate, image::ImageFormat::Jpeg).unwrap();

    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!(
                "root={}&compare={}",
                urlencode(&a.display().to_string()),
                urlencode(&b.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;

    assert!(html.contains("Similar Images — A vs B"), "{html}");
    assert!(html.contains("original.jpg"), "A side listed: {html}");
    assert!(html.contains("small.jpg"), "B side listed: {html}");
    assert!(
        html.contains("Move 1 B copies to purgatory"),
        "removable B copy offered: {html}"
    );
    let marker = "name=\"files\" value=\"";
    let start = html.find(marker).expect("files input") + marker.len();
    let end = start + html[start..].find('"').unwrap();
    assert!(
        html[start..end].contains("small.jpg"),
        "B candidate in purgatory form: {}",
        &html[start..end]
    );
}

#[tokio::test]
async fn settings_tab_shows_and_moves_the_cache() {
    use dedupe2::web::{app_with_state, AppState};
    use dedupe2::Scanner::cache::ScanCache;

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    std::fs::write(a.join("one.jpg"), &img).unwrap();
    std::fs::write(b.join("one.jpg"), &img).unwrap();

    let state = AppState::default();

    // The settings page renders.
    let res = app_with_state(state.clone())
        .oneshot(
            Request::builder()
                .uri("/settings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(html.contains("Settings"), "{html}");
    assert!(html.contains("Cache file path"), "{html}");

    // Learn something so the cache has content.
    let res = app_with_state(state.clone())
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
    let _ = body_string(res.into_body()).await;

    // Move the cache to a new file via the settings form.
    let new_cache = root.path().join("store/new-cache.bin");
    let res = app_with_state(state.clone())
        .oneshot(form_request(
            "/settings/save",
            format!("cache_path={}", urlencode(&new_cache.display().to_string())),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;
    assert!(html.contains("Cache moved to"), "{html}");
    assert_eq!(state.cache_path(), Some(new_cache.clone()));
    assert!(new_cache.exists(), "cache file written at the new location");

    // The learned knowledge travelled with it.
    let loaded = ScanCache::load(&new_cache);
    assert!(loaded.stats().files >= 1, "cache content preserved");

    // Clearing wipes both memory and file.
    let res = app_with_state(state.clone())
        .oneshot(form_request("/settings/clear", String::new()))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(html.contains("Cache cleared"), "{html}");
    assert!(!new_cache.exists(), "cache file deleted");
    assert_eq!(
        dedupe2::Scanner::cache::lock(&state.cache).stats().files,
        0
    );
}

#[tokio::test]
async fn scans_reject_nonexistent_trees_up_front() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let missing = root.path().join("does/not/exist");

    // Compare: bad B fails before any scanning happens.
    let res = app()
        .oneshot(form_request(
            "/compare/run",
            format!(
                "a={}&b={}",
                urlencode(&real.display().to_string()),
                urlencode(&missing.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_string(res.into_body()).await;
    assert!(body.contains("not a directory"), "{body}");

    // Similar compare mode: the second tree is validated up front too.
    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!(
                "root={}&compare={}",
                urlencode(&real.display().to_string()),
                urlencode(&missing.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_string(res.into_body()).await;
    assert!(body.contains("not a directory"), "{body}");

    // Verify: same.
    let res = app()
        .oneshot(form_request(
            "/verify/run",
            format!("root={}", urlencode(&missing.display().to_string())),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn similar_swap_replaces_worse_a_copies_with_better_b_copies() {
    let root = tempfile::tempdir().unwrap();
    let a_root = root.path().join("library");
    let b_root = root.path().join("candidate");
    let purgatory = root.path().join("purgatory");
    std::fs::create_dir_all(&a_root).unwrap();
    std::fs::create_dir_all(&b_root).unwrap();
    std::fs::create_dir_all(&purgatory).unwrap();

    let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    let better_bytes = std::fs::read(&original).unwrap();

    // A holds the downscaled copy; B holds the original (bigger file).
    let img = image::open(&original).unwrap();
    let small = img.resize_exact(
        (img.width() as f32 * 0.5) as u32,
        (img.height() as f32 * 0.5) as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let a_worse = a_root.join("photo.jpg");
    small.save_with_format(&a_worse, image::ImageFormat::Jpeg).unwrap();
    let b_better = b_root.join("photo.jpg");
    std::fs::copy(&original, &b_better).unwrap();

    // The compare result offers a swap for this pair.
    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!(
                "root={}&compare={}",
                urlencode(&a_root.display().to_string()),
                urlencode(&b_root.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(html.contains("swap available"), "{html}");
    assert!(html.contains("Swap 1 better B copies into A"), "{html}");

    // Run the swap.
    let swaps = format!("{}\t{}", a_worse.display(), b_better.display());
    let res = app()
        .oneshot(form_request(
            "/similar/swap",
            format!(
                "swaps={}&a_root={}&b_root={}&purgatory={}",
                urlencode(&swaps),
                urlencode(&a_root.display().to_string()),
                urlencode(&b_root.display().to_string()),
                urlencode(&purgatory.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;
    assert!(html.contains("Swap Complete"), "{html}");

    // A now holds the better copy; the worse one is in purgatory.
    assert_eq!(std::fs::read(&a_worse).unwrap(), better_bytes);
    assert!(!b_better.exists());
    assert!(purgatory.join("photo.jpg").exists());
}

#[tokio::test]
async fn compare_results_embed_full_sets_for_filtering() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(a.join("2006/trip")).unwrap();
    std::fs::create_dir_all(b.join("2006/trip")).unwrap();
    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    std::fs::write(a.join("2006/trip/IMG_0100.jpg"), &img).unwrap();
    std::fs::write(b.join("2006/trip/IMG_0100.jpg"), &img).unwrap();
    // A real A-only photo (undecodable A-side files are ignored by design).
    let a_only = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-dupe-diff-size/IMG_2571.jpg"),
    )
    .unwrap();
    std::fs::write(a.join("2006/keeper-only.jpg"), &a_only).unwrap();
    std::fs::write(b.join("2006/new.jpg"), b"only in b").unwrap();

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
    let html = body_string(res.into_body()).await;

    // The per-table filters and the embedded full-result payloads exist...
    for id in [
        "compare-a-data",
        "compare-b-data",
        "compare-dup-data",
        "compare-unreadable-data",
    ] {
        assert!(html.contains(id), "missing payload {id}");
    }
    // ...and they carry the full sets (including the 2006 paths), so the
    // filter can search beyond the visible rows.
    assert!(html.contains("2006/keeper-only.jpg"), "{html}");
    assert!(html.contains("2006/new.jpg"), "{html}");
    assert!(html.contains("2006/trip/IMG_0100.jpg"), "dup path missing");
}

#[tokio::test]
async fn compare_tables_have_their_own_filters_and_more_buttons() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let img = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG"),
    )
    .unwrap();
    std::fs::write(a.join("one.jpg"), &img).unwrap();
    std::fs::write(b.join("one.jpg"), &img).unwrap();

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
    let html = body_string(res.into_body()).await;

    for suffix in ["a", "dups", "b", "unreadable"] {
        assert!(
            html.contains(&format!("id=\"compare-filter-{suffix}\"")),
            "missing per-table filter for {suffix}"
        );
        assert!(
            html.contains(&format!("id=\"compare-count-{suffix}\"")),
            "missing count for {suffix}"
        );
    }
    assert!(!html.contains("id=\"compare-filter\""), "old global filter should be gone");
    // ...and each table has a copy/move action tied to its filter.
    for suffix in ["a", "dups", "b", "unreadable"] {
        assert!(
            html.contains(&format!("id=\"compare-action-{suffix}\"")),
            "missing action form for {suffix}"
        );
        assert!(
            html.contains(&format!("data-table=\"{suffix}\"")),
            "missing data-table marker for {suffix}"
        );
    }
}

#[tokio::test]
async fn compare_table_transfer_copies_or_moves_filtered_files() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dest = root.path().join("picked");
    std::fs::create_dir_all(&src).unwrap();
    let one = src.join("a.jpg");
    let two = src.join("b.jpg");
    std::fs::write(&one, b"one").unwrap();
    std::fs::write(&two, b"two").unwrap();

    // Copy two filtered files into a typed destination folder.
    let files = format!("{}\n{}", one.display(), two.display());
    let res = app()
        .oneshot(form_request(
            "/compare/transfer",
            format!(
                "files={}&destination={}&op=copy",
                urlencode(&files),
                urlencode(&dest.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;
    assert!(html.contains("Copy Complete"), "{html}");
    assert_eq!(std::fs::read(dest.join("a.jpg")).unwrap(), b"one");
    assert_eq!(std::fs::read(dest.join("b.jpg")).unwrap(), b"two");
    assert!(one.exists() && two.exists());

    // Move one of them.
    let res = app()
        .oneshot(form_request(
            "/compare/transfer",
            format!(
                "files={}&destination={}&op=move",
                urlencode(&one.display().to_string()),
                urlencode(&dest.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(html.contains("Move Complete"), "{html}");
    assert!(!one.exists(), "move removes the source");
    assert!(dest.join("a (1).jpg").exists(), "collision renamed");

    // Validation.
    let res = app()
        .oneshot(form_request(
            "/compare/transfer",
            "files=&destination=/tmp&op=copy".into(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = app()
        .oneshot(form_request(
            "/compare/transfer",
            format!("files={}&destination=&op=copy", urlencode(&two.display().to_string())),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn similar_tables_have_filters_and_embedded_rows() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("library");
    std::fs::create_dir_all(&tree).unwrap();
    let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    std::fs::copy(&original, tree.join("original.jpg")).unwrap();
    let img = image::open(&original).unwrap();
    let rgb = img.to_rgb8();
    image::imageops::resize(&rgb, 600, 800, image::imageops::FilterType::Lanczos3)
        .save_with_format(tree.join("small.jpg"), image::ImageFormat::Jpeg)
        .unwrap();

    // Self mode.
    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!("root={}", urlencode(&tree.display().to_string())),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(html.contains("id=\"similar-rows-data\""), "{html}");
    assert!(html.contains("id=\"similar-filter-rows\""), "{html}");
        assert!(html.contains("id=\"similar-count-rows\""), "{html}");
    assert!(html.contains("original.jpg") && html.contains("small.jpg"), "{html}");

    // Compare mode: an exact copy (duplicate) plus a downscale (similar).
    let b_root = root.path().join("candidate");
    std::fs::create_dir_all(&b_root).unwrap();
    std::fs::copy(&original, b_root.join("original.jpg")).unwrap();
    // A different scale than A's own downscale, so it stays a *similar*
    // match rather than an exact duplicate.
    let rgb2 = image::open(&original).unwrap().to_rgb8();
    image::imageops::resize(&rgb2, 480, 640, image::imageops::FilterType::Lanczos3)
        .save_with_format(b_root.join("small.jpg"), image::ImageFormat::Jpeg)
        .unwrap();
    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!(
                "root={}&compare={}",
                urlencode(&tree.display().to_string()),
                urlencode(&b_root.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(html.contains("id=\"similar-matches-data\""), "{html}");
    assert!(html.contains("id=\"similar-filter-matches\""), "{html}");
        assert!(html.contains("id=\"similar-count-matches\""), "{html}");
}

#[tokio::test]
async fn similar_compare_eliminates_exact_duplicates_first() {
    let root = tempfile::tempdir().unwrap();
    let a_root = root.path().join("library");
    let b_root = root.path().join("candidate");
    std::fs::create_dir_all(&a_root).unwrap();
    std::fs::create_dir_all(&b_root).unwrap();

    let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    // A has the original; B has an exact copy plus a downscaled version.
    std::fs::copy(&original, a_root.join("photo.jpg")).unwrap();
    std::fs::copy(&original, b_root.join("exact.jpg")).unwrap();
    let img = image::open(&original).unwrap();
    let rgb = img.to_rgb8();
    image::imageops::resize(&rgb, 600, 800, image::imageops::FilterType::Lanczos3)
        .save_with_format(b_root.join("small.jpg"), image::ImageFormat::Jpeg)
        .unwrap();

    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!(
                "root={}&compare={}",
                urlencode(&a_root.display().to_string()),
                urlencode(&b_root.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;

    // The exact copy is reported as a duplicate, not as a similar match.
    assert!(html.contains("Exact duplicates"), "{html}");
    assert!(html.contains("id=\"similar-dups-data\""), "{html}");
    assert!(html.contains("id=\"purgatory-form-dups\""), "{html}");
    assert!(html.contains("Move 1 exact B copies to purgatory"), "{html}");
    let marker = "id=\"similar-dups-data\" type=\"application/json\">";
    let start = html.find(marker).unwrap() + marker.len();
    let end = start + html[start..].find("</script>").unwrap();
    let payload = &html[start..end];
    assert!(payload.contains("exact.jpg"), "duplicate payload: {payload}");
    assert!(
        !payload.contains("small.jpg"),
        "the downscale is not an exact duplicate: {payload}"
    );
}

#[tokio::test]
async fn overlapping_two_tree_operations_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let outer = root.path().join("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    let sibling = root.path().join("sibling");
    std::fs::create_dir_all(&sibling).unwrap();

    // Compare: nested in either direction and equal trees are rejected.
    for (a, b) in [
        (outer.clone(), inner.clone()),
        (inner.clone(), outer.clone()),
        (outer.clone(), outer.clone()),
    ] {
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
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{a:?} vs {b:?}");
        let body = body_string(res.into_body()).await;
        assert!(body.contains("overlap") || body.contains("same"), "{body}");
    }

    // Disjoint trees still work.
    let res = app()
        .oneshot(form_request(
            "/compare/run",
            format!(
                "a={}&b={}",
                urlencode(&outer.display().to_string()),
                urlencode(&sibling.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Sets: A and any candidate (or candidates among each other) must be
    // disjoint.
    let res = app()
        .oneshot(form_request(
            "/sets/run",
            format!(
                "a={}&candidates={}",
                urlencode(&outer.display().to_string()),
                urlencode(&inner.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Similar compare mode.
    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!(
                "root={}&compare={}",
                urlencode(&outer.display().to_string()),
                urlencode(&inner.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Swap.
    let res = app()
        .oneshot(form_request(
            "/similar/swap",
            format!(
                "swaps={}&a_root={}&b_root={}&purgatory={}",
                urlencode("/x\t/y"),
                urlencode(&outer.display().to_string()),
                urlencode(&inner.display().to_string()),
                urlencode(&sibling.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Organize: destination inside the source is rejected (loop hazard);
    // source inside the destination is legitimate.
    let res = app()
        .oneshot(form_request(
            "/organize/scan",
            format!(
                "source={}&destination={}&format=YYYY/MM-DD",
                urlencode(&outer.display().to_string()),
                urlencode(&inner.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = app()
        .oneshot(form_request(
            "/organize/scan",
            format!(
                "source={}&destination={}&format=YYYY/MM-DD",
                urlencode(&inner.display().to_string()),
                urlencode(&outer.display().to_string())
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn thumb_endpoint_serves_small_jpegs() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");

    let res = app()
        .oneshot(
            Request::builder()
                .uri(format!("/thumb?path={}", urlencode(&fixture.display().to_string())))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("content-type").unwrap(),
        "image/jpeg"
    );
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.len() > 100, "thumbnail has content");
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG magic");
    let original = std::fs::metadata(&fixture).unwrap().len() as usize;
    assert!(
        bytes.len() < original / 10,
        "thumbnail ({} bytes) should be far smaller than the original ({original})",
        bytes.len()
    );

    // Missing files yield 404, never a crash.
    let res = app()
        .oneshot(
            Request::builder()
                .uri("/thumb?path=/definitely/not/here.jpg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn similar_tables_include_preview_and_size_delta_columns() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("library");
    std::fs::create_dir_all(&tree).unwrap();
    let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    std::fs::copy(&original, tree.join("original.jpg")).unwrap();
    let rgb = image::open(&original).unwrap().to_rgb8();
    image::imageops::resize(&rgb, 600, 800, image::imageops::FilterType::Lanczos3)
        .save_with_format(tree.join("small.jpg"), image::ImageFormat::Jpeg)
        .unwrap();

    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!("root={}", urlencode(&tree.display().to_string())),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;
    assert!(html.contains("<th>Preview</th>"), "{html}");
    assert!(html.contains("Size &Delta;"), "{html}");
}

#[tokio::test]
async fn similar_decide_applies_keep_both_with_exif_destination() {
    let root = tempfile::tempdir().unwrap();
    let a_root = root.path().join("library");
    let b_root = root.path().join("candidate");
    let purgatory = root.path().join("purgatory");
    std::fs::create_dir_all(&a_root).unwrap();
    std::fs::create_dir_all(&b_root).unwrap();

    let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    let a_bigger = a_root.join("photo.jpg");
    std::fs::write(&a_bigger, vec![1u8; 8192]).unwrap();
    let b_smaller = b_root.join("photo.jpg");
    std::fs::copy(&original, &b_smaller).unwrap();

    let res = app()
        .oneshot(form_request(
            "/similar/decide",
            format!(
                "decisions={}&a_root={}&b_root={}&purgatory={}&format={}",
                urlencode(&format!("{}\t{}\tkeep_both", a_bigger.display(), b_smaller.display())),
                urlencode(&a_root.display().to_string()),
                urlencode(&b_root.display().to_string()),
                urlencode(&purgatory.display().to_string()),
                urlencode("YYYY/MM-DD")
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;
    assert!(html.contains("Decisions Applied"), "{html}");
    assert!(html.contains("kept both"), "{html}");

    // A untouched; B moved into A at its EXIF destination (2022-08-17).
    assert!(a_bigger.exists());
    assert!(!b_smaller.exists());
    assert!(a_root.join("2022/08-17/photo.jpg").exists());
}

#[tokio::test]
async fn similar_compare_page_offers_per_row_decisions() {
    let root = tempfile::tempdir().unwrap();
    let a_root = root.path().join("library");
    let b_root = root.path().join("candidate");
    std::fs::create_dir_all(&a_root).unwrap();
    std::fs::create_dir_all(&b_root).unwrap();
    let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    std::fs::copy(&original, a_root.join("photo.jpg")).unwrap();
    // B: a downscaled copy with the fixture's EXIF segment spliced in, so it
    // is similar (not identical) AND carries a date for the destination.
    let fixture = std::fs::read(&original).unwrap();
    // Walk the segments to the real APP1 (EXIF); the first segment is APP0.
    let mut offset = 2usize;
    let app1 = loop {
        assert_eq!(fixture[offset], 0xFF, "jpeg marker expected");
        let marker = fixture[offset + 1];
        let len = u16::from_be_bytes([fixture[offset + 2], fixture[offset + 3]]) as usize;
        if marker == 0xE1 {
            break &fixture[offset..offset + 2 + len];
        }
        offset += 2 + len;
    };
    let rgb = image::open(&original).unwrap().to_rgb8();
    let small = image::imageops::resize(&rgb, 600, 800, image::imageops::FilterType::Lanczos3);
    let mut scaled = Vec::new();
    small
        .write_to(&mut std::io::Cursor::new(&mut scaled), image::ImageFormat::Jpeg)
        .unwrap();
    let mut b_bytes = vec![0xFF, 0xD8];
    b_bytes.extend_from_slice(app1);
    b_bytes.extend_from_slice(&scaled[2..]);
    std::fs::write(b_root.join("small.jpg"), b_bytes).unwrap();

    let res = app()
        .oneshot(form_request(
            "/similar/run",
            format!(
                "root={}&compare={}",
                urlencode(&a_root.display().to_string()),
                urlencode(&b_root.display().to_string())
            ),
        ))
        .await
        .unwrap();
    let html = body_string(res.into_body()).await;

    assert!(html.contains("id=\"similar-decide-form\""), "{html}");
    assert!(html.contains("<th>Use</th>"), "{html}");
    assert!(html.contains("<th>Action</th>"), "{html}");
    assert!(html.contains("Destination (EXIF of B)"), "{html}");
    assert!(html.contains("similar-decide-format"), "{html}");
    // The rows payload carries the B EXIF date for the destination preview.
    let marker = "id=\"similar-matches-data\" type=\"application/json\">";
    let start = html.find(marker).unwrap() + marker.len();
    let end = start + html[start..].find("</script>").unwrap();
    let payload = &html[start..end];
    assert!(payload.contains("\"date\":\"2022-08-17"), "{payload}");
}

/// Static consistency: the resizable-table machinery exists and the table CSS
/// keeps rows single-line (the squished-row complaint).
#[test]
fn resizable_table_machinery_is_wired() {
    let js = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static/compare.js"),
    )
    .unwrap();
    assert!(js.contains("Tabulator"), "table library wired");
    assert!(js.contains("resizableColumns: true"), "column resizing delegated");
    assert!(js.contains("rowSelection"), "select-all header checkbox");
    assert!(js.contains("paginationSize"), "pagination");
    assert!(js.contains("persistenceID"), "column widths persisted");
    assert!(js.contains("corner-resizer"), "corner handle wired");
    assert!(js.contains("fullscreen-toggle"), "fullscreen button wired");
    assert!(js.contains("fullscreen-exit"), "in-frame exit button");
    assert!(js.contains("Escape"), "escape exits fullscreen");
    assert!(js.contains("similar-similarity-rows"), "similarity slider wired (self)");
    assert!(js.contains("similar-similarity-matches"), "similarity slider wired (compare)");
    for id in ["similar-table-matches", "similar-table-rows", "compare-table-dups"] {
        assert!(js.contains(id), "default widths for {id}");
    }

    let css = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static/style.css"),
    )
    .unwrap();
    assert!(css.contains(".tabulator"), "table theme overrides");
    assert!(css.contains(".corner-resizer"), "corner handle styling");
    assert!(css.contains(".table-frame.fullscreen"), "fullscreen styling");
    assert!(css.contains(".similarity-slider"), "slider styling");
    assert!(css.contains("text-overflow: ellipsis"), "long paths stay on one line");

    // The library assets are vendored locally (served from /static/vendor).
    let base = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/base.html"),
    )
    .unwrap();
    assert!(base.contains("/static/vendor/tabulator.min.js"), "{base}");
    assert!(base.contains("/static/vendor/tabulator.min.css"), "{base}");
    assert!(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("static/vendor/tabulator.min.js")
            .exists()
    );
}

#[tokio::test]
async fn thumb_serves_the_fixture_and_rejects_non_images() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");

    let res = app()
        .oneshot(
            Request::builder()
                .uri(format!("/thumb?path={}", urlencode(&fixture.display().to_string())))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers().get("content-type").unwrap(), "image/jpeg");
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..2], &[0xFF, 0xD8], "preview body is a JPEG");

    for bad in ["/definitely/not/here.jpg", "/etc/passwd", "../../../etc/hosts"] {
        let res = app()
            .oneshot(
                Request::builder()
                    .uri(format!("/thumb?path={}", urlencode(bad)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "path {bad}");
    }
}

#[tokio::test]
async fn malformed_forms_never_return_a_server_error() {
    let cases = [
        ("/similar/run", "root=&compare="),
        ("/similar/decide", "decisions=&a_root=&b_root=&purgatory="),
        ("/similar/swap", "swaps=&a_root=&b_root=&purgatory="),
        ("/compare/exif", "originals=&a=&destination="),
        ("/compare/copy", "originals=&destination="),
        ("/compare/deep", "pairs=&originals="),
        ("/compare/transfer", "files=&destination="),
        ("/compare/purgatory", "files=&b_root=&purgatory="),
        ("/verify/rehome", "mismatches=&root="),
        ("/organize/scan", "source=&destination="),
        ("/organize/run", "files=&destination="),
        ("/consolidate/preview", "dir="),
        ("/consolidate/run", "dir="),
        ("/sets/run", "a=&candidates="),
    ];

    for (uri, body) in cases {
        let res = app().oneshot(form_request(uri, body.into())).await.unwrap();
        assert_ne!(
            res.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{uri} returned a server error for empty input"
        );
    }

    // Missing required fields are rejected by the extractor, not by a panic.
    for uri in ["/similar/run", "/organize/run", "/verify/rehome"] {
        let res = app().oneshot(form_request(uri, String::new())).await.unwrap();
        assert_ne!(res.status(), StatusCode::INTERNAL_SERVER_ERROR, "{uri}");
    }
}

#[tokio::test]
async fn cache_status_for_an_unknown_tree_reports_zeroes() {
    let res = app()
        .oneshot(
            Request::builder()
                .uri("/cache-status?path=/definitely/not/a/tree")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res.into_body()).await;
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    for key in ["cached_files", "shallow", "deep", "phash", "exif"] {
        assert_eq!(value[key], 0, "{key} must be zero for an unknown tree: {body}");
    }
}

#[tokio::test]
async fn verify_rehome_moves_a_misfiled_photo_and_reverifies() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_path_buf();
    let junk = root_path.join("junk");
    std::fs::create_dir_all(&junk).unwrap();
    let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    let misfiled = junk.join("photo.jpg");
    std::fs::copy(&original, &misfiled).unwrap();

    // The fixture's true EXIF date is 2022-08-17.
    let body = format!(
        "mismatches={}&root={}&format={}",
        urlencode(&format!("{}\t2022:08:17", misfiled.display())),
        urlencode(&root_path.display().to_string()),
        urlencode("YYYY/MM-DD")
    );
    let res = app().oneshot(form_request("/verify/rehome", body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res.into_body()).await;
    assert!(html.contains("Re-home Complete"), "{html}");
    assert!(html.contains("event: done"), "{html}");

    assert!(!misfiled.exists(), "misfiled source was moved");
    assert!(root_path.join("2022/08-17/photo.jpg").exists());
}
