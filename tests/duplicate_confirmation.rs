//! Merge and consolidate only purge files whose full content is proven
//! identical: a shallow (64 KB) hash collision or an unreadable file is never
//! enough to send a photo to purgatory.

use std::fs;
use std::path::{Path, PathBuf};

use dedupe2::filemover::{consolidate_events, preview_merge};

fn sample(rel: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages")
        .join(rel);
    fs::read(path).expect("sample image should exist")
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// `bytes` with one byte near the end flipped: the first 64 KB of image data
/// (and so the shallow hash) is unchanged, the full content is not.
fn lookalike(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let mut at = out.len() - 200;
    while out[at] == 0xFF || out[at - 1] == 0xFF || out[at] == 0xFE {
        at -= 1;
    }
    out[at] ^= 0x01;
    out
}

#[test]
fn merge_keeps_shallow_lookalikes_and_still_groups_true_copies() {
    let root = tempfile::tempdir().unwrap();
    let (a, b) = (root.path().join("a"), root.path().join("b"));
    let img = sample("jpg-exif-mod/image1.JPG");

    write(&a.join("2008/01-01 trip/one.jpg"), &img);
    write(&b.join("2008/01-01 trip/two.jpg"), &lookalike(&img));
    write(&b.join("2008/01-01 trip/copy.jpg"), &img);

    let plan = preview_merge(&[a, b], &root.path().join("out"), None).unwrap();

    assert_eq!(
        plan.duplicate_groups.len(),
        1,
        "only the true copy pairs up"
    );
    let group = &plan.duplicate_groups[0];
    let purged: Vec<&PathBuf> = group.purged.iter().map(|m| &m.source).collect();
    assert_eq!(purged.len(), 1);
    assert!(
        purged[0].ends_with("one.jpg") || purged[0].ends_with("copy.jpg"),
        "the lookalike is never purged: {purged:?}"
    );
    assert_eq!(plan.uniques.len(), 1);
    assert!(plan.uniques[0].source.ends_with("two.jpg"));
}

#[test]
fn consolidate_never_purges_a_shallow_lookalike() {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2008");
    let purgatory = root.path().join("purgatory");
    let img = sample("jpg-exif-mod/image1.JPG");

    write(&year.join("01-01/a.jpg"), &img);
    write(&year.join("01-01 Hawaii/b.jpg"), &lookalike(&img));

    let out = consolidate_events(&year, Some(&purgatory)).unwrap();

    assert_eq!(out.purged_files, 0);
    assert_eq!(out.moved_files, 1);
    assert!(year.join("01-01 Hawaii/a.jpg").exists());
    assert!(year.join("01-01 Hawaii/b.jpg").exists());
    assert!(!purgatory.exists(), "nothing went to purgatory");
}

#[cfg(unix)]
#[test]
fn consolidate_never_purges_unreadable_files() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2008");
    let purgatory = root.path().join("purgatory");
    let first = year.join("01-01/a.jpg");
    let second = year.join("01-01 Hawaii/b.jpg");
    write(&first, b"first unreadable file");
    write(&second, b"second, different, unreadable file");
    for p in [&first, &second] {
        fs::set_permissions(p, fs::Permissions::from_mode(0o000)).unwrap();
    }
    if fs::read(&first).is_ok() {
        eprintln!("skipping: permissions are not enforced (running as root?)");
        return;
    }

    let out = consolidate_events(&year, Some(&purgatory)).unwrap();

    assert_eq!(
        out.purged_files, 0,
        "unreadable files share a fallback hash, not content"
    );
    assert!(!purgatory.exists());
}

#[test]
fn deep_scan_never_confirms_a_pair_it_cannot_read() {
    use dedupe2::scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a.jpg");
    let b = root.path().join("b.jpg");
    write(&a, b"corrupt jpeg one");
    write(&b, b"corrupt jpeg two");

    let outcome = deep_scan_pairs(&[DupPair { a, b }]);

    assert_eq!(outcome.kept, 0, "undecidable is not a duplicate");
    assert!(outcome.confirmed_b.is_empty());
    assert!(outcome.confirmed.is_empty());
    assert_eq!(outcome.removed.len(), 1);
    assert!(
        outcome.removed[0].reason.contains("not proven"),
        "{}",
        outcome.removed[0].reason
    );
}

mod http {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn enc(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    async fn post(uri: &str, body: String) -> (StatusCode, String) {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        let res = dedupe2::web::app().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn compare_results_offer_no_purge_before_a_deep_scan() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = (root.path().join("a"), root.path().join("b"));
        let img = sample("jpg-exif-mod/image1.JPG");
        write(&a.join("photo.jpg"), &img);
        write(&b.join("photo.jpg"), &lookalike(&img));

        let body = format!(
            "a={}&b={}",
            enc(&a.display().to_string()),
            enc(&b.display().to_string())
        );
        let (_, html) = post("/compare/run", body).await;

        assert!(
            html.contains("photo.jpg"),
            "the shallow match is listed: {html}"
        );
        assert!(
            !html.contains("/compare/purgatory"),
            "no purge from shallow matches: {html}"
        );
        assert!(html.contains("deep-scan-required"), "{html}");
    }

    #[tokio::test]
    async fn deep_scan_then_purge_moves_only_proven_copies() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = (root.path().join("a"), root.path().join("b"));
        let purgatory = root.path().join("purgatory");
        let img = sample("jpg-exif-mod/image1.JPG");
        let keeper = a.join("photo.jpg");
        let copy = b.join("2020/copy.jpg");
        let fake = b.join("2020/lookalike.jpg");
        write(&keeper, &img);
        write(&copy, &img);
        write(&fake, &lookalike(&img));

        // Deep scan both shallow pairs: only the true copy is confirmed and
        // offered to the proven-duplicate purge.
        let pairs = format!(
            "{}\t{}\n{}\t{}",
            keeper.display(),
            copy.display(),
            keeper.display(),
            fake.display()
        );
        let body = format!(
            "pairs={}&originals=&destination={}&b={}",
            enc(&pairs),
            enc(&a.display().to_string()),
            enc(&b.display().to_string())
        );
        let (_, html) = post("/compare/deep", body).await;
        assert!(html.contains("1 confirmed duplicates"), "{html}");
        assert!(
            html.contains("action=\"/compare/purgatory-duplicates\""),
            "{html}"
        );

        // Even if a tampered or stale form names the lookalike, the server
        // re-proves every pair and leaves it in place.
        let body = format!(
            "pairs={}&b_root={}&purgatory={}",
            enc(&pairs),
            enc(&b.display().to_string()),
            enc(&purgatory.display().to_string())
        );
        let (status, html) = post("/compare/purgatory-duplicates", body).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains("Moved to Purgatory"), "{html}");

        assert!(!copy.exists());
        assert!(purgatory.join("2020/copy.jpg").exists());
        assert!(fake.exists(), "the lookalike is never purged");
        assert!(!purgatory.join("2020/lookalike.jpg").exists());
        assert!(keeper.exists());
    }

    #[tokio::test]
    async fn proven_purge_rejects_empty_input() {
        let (status, _) = post(
            "/compare/purgatory-duplicates",
            "pairs=&b_root=/tmp&purgatory=/tmp".into(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
