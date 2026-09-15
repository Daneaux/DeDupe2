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

#[test]
fn purgatory_move_preserves_tree_structure_and_prunes_empty_dirs() {
    use dedupe2::filemover::move_to_purgatory_with_progress;

    let src = tempfile::tempdir().unwrap();
    let purg = tempfile::tempdir().unwrap();
    let root = src.path();

    std::fs::create_dir_all(root.join("2013/07-30")).unwrap();
    std::fs::create_dir_all(root.join("2013/07-31")).unwrap();
    std::fs::create_dir_all(root.join("2012")).unwrap();

    let a = root.join("2013/07-30/IMG_1.jpg");
    let b = root.join("2013/07-31/IMG_2.jpg");
    let keep = root.join("2012/keep.jpg");
    std::fs::write(&a, b"content-a").unwrap();
    std::fs::write(&b, b"content-b").unwrap();
    std::fs::write(&keep, b"keep-me").unwrap();

    let outcome = move_to_purgatory_with_progress(
        &[a.clone(), b.clone()],
        root,
        purg.path(),
        &|_, _| {},
    );

    assert_eq!(outcome.planned, 2);
    assert_eq!(outcome.moved, 2, "failures: {:?}", outcome.failures);
    assert!(outcome.failures.is_empty());

    // Same structure under purgatory, bytes intact.
    assert_eq!(
        std::fs::read(purg.path().join("2013/07-30/IMG_1.jpg")).unwrap(),
        b"content-a"
    );
    assert_eq!(
        std::fs::read(purg.path().join("2013/07-31/IMG_2.jpg")).unwrap(),
        b"content-b"
    );

    // Sources gone; emptied folders pruned; non-empty folder and root kept.
    assert!(!a.exists());
    assert!(!b.exists());
    assert!(!root.join("2013/07-30").exists());
    assert!(!root.join("2013/07-31").exists());
    assert!(!root.join("2013").exists(), "year folder became empty and must go");
    assert!(keep.exists());
    assert!(root.join("2012").exists());
    assert!(root.exists(), "the candidate root itself is never removed");
}

#[test]
fn purgatory_move_renames_collisions_and_never_overwrites() {
    use dedupe2::filemover::move_to_purgatory_with_progress;

    let src = tempfile::tempdir().unwrap();
    let purg = tempfile::tempdir().unwrap();
    let root = src.path();

    std::fs::create_dir_all(root.join("ev")).unwrap();
    let src_file = root.join("ev/IMG_0500.jpg");
    std::fs::write(&src_file, b"new bytes").unwrap();

    // Case-insensitive collision: an existing different file with the same
    // name in a different case must not be overwritten.
    std::fs::create_dir_all(purg.path().join("ev")).unwrap();
    std::fs::write(purg.path().join("ev/IMG_0500.JPG"), b"old bytes").unwrap();

    let outcome = move_to_purgatory_with_progress(
        &[src_file.clone()],
        root,
        purg.path(),
        &|_, _| {},
    );

    assert_eq!(outcome.moved, 1);
    assert_eq!(outcome.renamed_on_collision, 1);
    assert_eq!(
        std::fs::read(purg.path().join("ev/IMG_0500.JPG")).unwrap(),
        b"old bytes",
        "existing file must be untouched"
    );
    assert_eq!(
        std::fs::read(purg.path().join("ev/IMG_0500 (1).jpg")).unwrap(),
        b"new bytes"
    );
    assert!(!src_file.exists());
}

#[test]
fn purgatory_move_reports_failures_and_leaves_sources_in_place() {
    use dedupe2::filemover::move_to_purgatory_with_progress;

    let src = tempfile::tempdir().unwrap();
    let purg = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let root = src.path();

    let ok_file = root.join("ok.jpg");
    std::fs::write(&ok_file, b"ok").unwrap();

    let missing = root.join("missing.jpg");
    let outside = other.path().join("outside.jpg");
    std::fs::write(&outside, b"outside").unwrap();

    let outcome = move_to_purgatory_with_progress(
        &[ok_file.clone(), missing.clone(), outside.clone()],
        root,
        purg.path(),
        &|_, _| {},
    );

    assert_eq!(outcome.moved, 1);
    assert_eq!(outcome.failures.len(), 2);
    assert!(outcome.failures.iter().any(|(p, r)| p == &missing && r.contains("not readable")));
    assert!(outcome
        .failures
        .iter()
        .any(|(p, r)| p == &outside && r.contains("not inside the candidate tree")));
    assert!(outside.exists(), "file outside the tree is never touched");
}
