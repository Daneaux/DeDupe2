use std::fs;
use std::io::Write;
use std::path::Path;

use dedupe2::Scanner::fastScan::{collect_photo_files, read_header_hash, HEADER_READ_BYTES};

fn write_file(dir: &Path, rel: &str, contents: &[u8]) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(contents).unwrap();
}

#[test]
fn collect_filters_to_photo_extensions() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "a.jpg", b"1");
    write_file(dir.path(), "b.PNG", b"2");
    write_file(dir.path(), "c.txt", b"3");
    write_file(dir.path(), "noext", b"4");

    let files = collect_photo_files(dir.path());
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

    let files = collect_photo_files(dir.path());

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].1, 1234);
}

#[test]
fn header_hash_matches_first_16kb() {
    let dir = tempfile::tempdir().unwrap();
    let contents = vec![5u8; 20_000];
    write_file(dir.path(), "a.jpg", &contents);

    let hash = read_header_hash(&dir.path().join("a.jpg")).unwrap();

    assert_eq!(hash, seahash::hash(&contents[..HEADER_READ_BYTES]));
}

#[test]
fn header_hash_handles_files_smaller_than_16kb() {
    let dir = tempfile::tempdir().unwrap();
    let contents = b"small file";
    write_file(dir.path(), "a.jpg", contents);

    let hash = read_header_hash(&dir.path().join("a.jpg")).unwrap();

    assert_eq!(hash, seahash::hash(contents));
}

#[test]
fn header_hash_is_none_for_missing_file() {
    let dir = tempfile::tempdir().unwrap();

    assert!(read_header_hash(&dir.path().join("missing.jpg")).is_none());
}
