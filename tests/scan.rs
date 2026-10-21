use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use dedupe2::Scanner::cache::ScanCache;
use dedupe2::Scanner::scan::{scan_tree, scan_trees};

fn write_file(dir: &Path, rel: &str, contents: &[u8]) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(contents).unwrap();
}

fn fixture(rel: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages")
            .join(rel),
    )
    .unwrap()
}

#[test]
fn scan_finds_supported_images_only() {
    let dir = tempfile::tempdir().unwrap();
    let jpg = fixture("jpg-exif-mod/image1.JPG");
    write_file(dir.path(), "a.jpg", &jpg);
    write_file(dir.path(), "b.PNG", b"not a real png");
    write_file(dir.path(), "c.txt", b"text");
    write_file(dir.path(), "noext", b"data");

    let tree = scan_tree(dir.path(), 64 * 1024, None, &|_, _| {}).unwrap();

    let names: Vec<String> = tree
        .files
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["a.jpg".to_string(), "b.PNG".to_string()]);
    assert_eq!(tree.stats.files, 2);
    assert_eq!(tree.stats.computed, 2);
    assert_eq!(tree.stats.reused, 0);
    assert_eq!(tree.stats.unreadable, 1, "the fake PNG cannot be decoded");
    assert!(tree.files[0].decoded);
    assert_eq!(tree.files[0].size, jpg.len() as u64);
    assert!(tree.files[0].modified.is_some());
}

#[test]
fn scan_trees_keeps_roots_separate_and_in_order() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    write_file(a.path(), "a.jpg", &fixture("jpg-exif-mod/image1.JPG"));
    write_file(b.path(), "b.jpg", &fixture("jpg-exif-mod/image1.JPG"));

    let trees = scan_trees(
        &[b.path().to_path_buf(), a.path().to_path_buf()],
        64 * 1024,
        None,
        &|_, _| {},
    )
    .unwrap();

    assert_eq!(trees.len(), 2);
    assert_eq!(trees[0].root, b.path());
    assert_eq!(trees[1].root, a.path());
    assert_eq!(trees[0].files[0].path, b.path().join("b.jpg"));
    assert_eq!(trees[1].files[0].path, a.path().join("a.jpg"));
}

#[test]
fn scan_reuses_cached_shallow_hashes() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "a.jpg", &fixture("jpg-exif-mod/image1.JPG"));
    let cache = Mutex::new(ScanCache::new());

    let first = scan_tree(dir.path(), 64 * 1024, Some(&cache), &|_, _| {}).unwrap();
    let second = scan_tree(dir.path(), 64 * 1024, Some(&cache), &|_, _| {}).unwrap();

    assert_eq!(first.stats.computed, 1);
    assert_eq!(first.stats.reused, 0);
    assert_eq!(second.stats.computed, 0);
    assert_eq!(second.stats.reused, 1);
    assert_eq!(first.files[0].hash, second.files[0].hash);
    assert!(first.files[0].decoded);
}

#[cfg(unix)]
#[test]
fn scan_skips_symlinked_files_and_directory_loops() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "real/a.jpg",
        &fixture("jpg-exif-mod/image1.JPG"),
    );
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();

    // File symlink and a directory symlink that points back at the tree.
    symlink(dir.path().join("real/a.jpg"), dir.path().join("linked.jpg")).unwrap();
    symlink(dir.path(), dir.path().join("sub/loop")).unwrap();

    let tree = scan_tree(dir.path(), 64 * 1024, None, &|_, _| {}).unwrap();
    assert_eq!(tree.stats.files, 1);
    assert_eq!(tree.files[0].path, dir.path().join("real/a.jpg"));
}

#[test]
fn scan_reports_progress_once_per_file_across_roots() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let img = fixture("jpg-exif-mod/image1.JPG");
    write_file(a.path(), "a1.jpg", &img);
    write_file(a.path(), "a2.jpg", &img);
    write_file(b.path(), "b1.jpg", &img);

    let calls = Mutex::new(Vec::new());
    let trees = scan_trees(
        &[a.path().to_path_buf(), b.path().to_path_buf()],
        64 * 1024,
        None,
        &|done, total| calls.lock().unwrap().push((done, total)),
    )
    .unwrap();

    let mut calls = calls.into_inner().unwrap();
    calls.sort();
    assert_eq!(trees.iter().map(|t| t.stats.files).sum::<usize>(), 3);
    assert_eq!(
        calls,
        vec![(1, 3), (2, 3), (3, 3)],
        "one callback per file, with the combined total"
    );

    // An empty tree completes without inventing any progress.
    let empty = tempfile::tempdir().unwrap();
    let calls = Mutex::new(Vec::new());
    let tree = scan_tree(
        empty.path(),
        64 * 1024,
        None,
        &|done, total| calls.lock().unwrap().push((done, total)),
    )
    .unwrap();
    assert!(tree.files.is_empty());
    assert!(calls.into_inner().unwrap().is_empty());
}
