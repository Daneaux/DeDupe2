use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dedupe2::Scanner::scanner::{shallow_scan, ScanTarget, HEADER_HASH_BYTES};
use dedupe2::volumes::Volume;

fn write_file(dir: &Path, rel: &str, contents: &[u8]) -> PathBuf {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(contents).unwrap();
    path
}

fn expected_hash(contents: &[u8]) -> String {
    let n = contents.len().min(HEADER_HASH_BYTES);
    format!("{:016x}", seahash::hash(&contents[..n]))
}

fn test_target(dir: &Path) -> ScanTarget {
    ScanTarget {
        name: "test".into(),
        paths: vec![dir.to_path_buf()],
        volume: Volume::new(dir.to_path_buf()),
    }
}

#[test]
fn scans_files_recursively() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "a.txt", b"hello");
    write_file(dir.path(), "sub/b.txt", b"world");
    write_file(dir.path(), "sub/deep/c.bin", &[0u8; 100]);

    let target = test_target(dir.path());
    let tree = shallow_scan(&target);

    let mut rel: Vec<String> = tree
        .files
        .iter()
        .map(|f| f.path.strip_prefix(dir.path()).unwrap().display().to_string())
        .collect();
    rel.sort();

    assert_eq!(
        rel,
        vec![
            "a.txt".to_string(),
            Path::new("sub").join("b.txt").display().to_string(),
            Path::new("sub").join("deep").join("c.bin").display().to_string()
        ]
    );
}

#[test]
fn records_size_and_hash_of_first_64kb() {
    let dir = tempfile::tempdir().unwrap();
    let contents = vec![7u8; 100_000];
    write_file(dir.path(), "big.bin", &contents);

    let target = test_target(dir.path());
    let tree = shallow_scan(&target);

    assert_eq!(tree.files.len(), 1);
    let file = &tree.files[0];
    assert_eq!(file.size, 100_000);
    assert_eq!(file.hash64kb, expected_hash(&contents));
}

#[test]
fn detects_file_type_from_extension() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "photo.JPG", b"x");

    let target = test_target(dir.path());
    let tree = shallow_scan(&target);

    assert_eq!(tree.files[0].file_type.ext, "jpg");
}

#[test]
fn captures_volume_root_and_scan_time() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "a.txt", b"x");

    let before = SystemTime::now();
    let target = test_target(dir.path());
    let tree = shallow_scan(&target);
    let after = SystemTime::now();

    assert_eq!(tree.root, dir.path().to_path_buf());
    assert!(tree.scan_time >= before && tree.scan_time <= after);
    assert_eq!(tree.target.volume.path, dir.path().to_path_buf());
}
