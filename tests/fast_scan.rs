use std::fs;
use std::io::Write;
use std::path::Path;

use dedupe2::Scanner::fastScan::{collect_files, fast_scan};
use dedupe2::Scanner::scanner::ScanTarget;
use dedupe2::volumes::Volume;

fn write_file(dir: &Path, rel: &str, contents: &[u8]) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(contents).unwrap();
}

fn test_target(dir: &Path, extensions: &[&str]) -> ScanTarget {
    ScanTarget {
        name: "test".into(),
        paths: vec![dir.to_path_buf()],
        volume: Volume::new(dir.to_path_buf()),
        extensions: extensions.iter().map(|s| s.to_string()).collect(),
        prefix_bytes: 64 * 1024,
    }
}

#[test]
fn collect_filters_to_configured_extensions() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "a.jpg", b"1");
    write_file(dir.path(), "b.PNG", b"2");
    write_file(dir.path(), "c.txt", b"3");
    write_file(dir.path(), "noext", b"4");

    let target = test_target(dir.path(), &["jpg", "png"]);
    let files = collect_files(&target);
    let mut exts: Vec<String> = files
        .iter()
        .map(|(path, _, _)| {
            path.extension()
                .unwrap()
                .to_string_lossy()
                .to_lowercase()
        })
        .collect();
    exts.sort();

    assert_eq!(exts, vec!["jpg".to_string(), "png".to_string()]);
}

#[test]
fn collect_records_file_size() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "a.jpg", &[0u8; 1234]);

    let target = test_target(dir.path(), &["jpg"]);
    let files = collect_files(&target);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].1, 1234);
}

#[test]
fn fast_scan_returns_scanned_tree() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "a.jpg", b"1");
    write_file(dir.path(), "b.png", b"22");
    write_file(dir.path(), "c.txt", b"333");

    let target = test_target(dir.path(), &["jpg", "png"]);
    let tree = fast_scan(&target);

    let mut rel: Vec<String> = tree
        .files
        .iter()
        .map(|f| f.path.strip_prefix(dir.path()).unwrap().display().to_string())
        .collect();
    rel.sort();

    assert_eq!(rel, vec!["a.jpg".to_string(), "b.png".to_string()]);
    assert_eq!(tree.target.name, "test");
}
