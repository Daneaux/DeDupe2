use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dedupe2::Scanner::scanner::ScannedFile;
use dedupe2::exif::CreationDate;
use dedupe2::filemover::{
    find_duplicates, merge_dirs, organize_by_date, relocate_tree, DuplicateStrategy, Operation,
};
use dedupe2::volumes::FileType;

fn scanned_file(path: PathBuf, date: CreationDate) -> ScannedFile {
    ScannedFile {
        path,
        size: 0,
        modified: SystemTime::now(),
        file_type: FileType { ext: String::new() },
        hash: 0,
        creation_date: date,
    }
}

#[test]
fn relocate_tree_skips_empty_dirs() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();

    std::fs::create_dir_all(src.path().join("sub")).unwrap();
    std::fs::create_dir_all(src.path().join("empty")).unwrap();
    std::fs::write(src.path().join("a.txt"), "a").unwrap();
    std::fs::write(src.path().join("sub/b.txt"), "b").unwrap();

    let out = dst.path().join("out");
    relocate_tree(src.path(), &out, Operation::Copy).unwrap();

    assert!(out.join("a.txt").exists());
    assert!(out.join("sub/b.txt").exists());
    assert!(!out.join("empty").exists());
}

#[test]
fn organize_by_date_builds_event_structure() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();

    let photo = src.path().join("photo.jpg");
    std::fs::write(&photo, "photo").unwrap();

    let files = vec![scanned_file(
        photo.clone(),
        CreationDate::DateCreated("2023:03:15 14:30:00".into()),
    )];

    organize_by_date(&files, dst.path(), "jan hawaii wedding", Operation::Copy).unwrap();

    assert!(dst.path().join("2023/03-15-hawaii-wedding/photo.jpg").exists());
}

#[test]
fn organize_by_date_unknown_goes_to_unknown() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();

    let photo = src.path().join("photo.jpg");
    std::fs::write(&photo, "photo").unwrap();

    let files = vec![scanned_file(photo.clone(), CreationDate::Unknown)];

    organize_by_date(&files, dst.path(), "", Operation::Copy).unwrap();

    assert!(dst.path().join("unknown/photo.jpg").exists());
}

#[test]
fn merge_keeps_one_copy_and_shortest_name() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();

    std::fs::write(a.path().join("same.txt"), "identical").unwrap();
    std::fs::write(b.path().join("same-copy.txt"), "identical").unwrap();
    std::fs::write(a.path().join("a-only.txt"), "a").unwrap();
    std::fs::write(b.path().join("b-only.txt"), "b").unwrap();

    merge_dirs(
        a.path(),
        b.path(),
        dst.path(),
        Operation::Copy,
        DuplicateStrategy::ExactHash,
    )
    .unwrap();

    let names: Vec<String> = std::fs::read_dir(dst.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();

    assert_eq!(names.len(), 3);
    assert!(names.iter().any(|n| n == "a-only.txt"));
    assert!(names.iter().any(|n| n == "b-only.txt"));
    assert!(names.iter().any(|n| n == "same.txt"));
    assert!(!names.iter().any(|n| n == "same-copy.txt"));
}

#[test]
fn find_duplicates_exact_hash_groups_identical_content() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    let c = dir.path().join("c.txt");
    std::fs::write(&a, "same").unwrap();
    std::fs::write(&b, "same").unwrap();
    std::fs::write(&c, "different").unwrap();

    let dupes = find_duplicates(&[a, b, c], DuplicateStrategy::ExactHash).unwrap();

    assert_eq!(dupes.len(), 1);
    assert_eq!(dupes[0].len(), 2);
}

#[test]
fn image_data_strategy_ignores_exif_but_exact_does_not() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let a = manifest.join("tests/TestImages/HEIC-exif-mod/heic1.HEIC");
    let b = manifest.join("tests/TestImages/HEIC-exif-mod/heic1-copy.HEIC");
    let paths = vec![a, b];

    let image = find_duplicates(&paths, DuplicateStrategy::ImageData).unwrap();
    assert_eq!(image.len(), 1);
    assert_eq!(image[0].len(), 2);

    let exact = find_duplicates(&paths, DuplicateStrategy::ExactHash).unwrap();
    assert!(exact.is_empty());
}
