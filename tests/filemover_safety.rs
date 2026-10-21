//! Failure-path safety for the verified move/copy machinery: no operation may
//! lose a file, overwrite an existing one, or let one failed row block others.

use std::path::Path;

use dedupe2::filemover::{
    apply_similar_decisions_with_progress, transfer_files_with_progress, Operation, SimilarDecision,
};

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
fn read_only_dirs_enforced() -> bool {
    let probe = std::env::temp_dir().join(format!(
        "dedupe2-perm-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&probe).unwrap();
    set_mode(&probe, 0o555);
    let denied = std::fs::write(probe.join("probe"), b"x").is_err();
    set_mode(&probe, 0o755);
    let _ = std::fs::remove_dir_all(&probe);
    denied
}

#[cfg(unix)]
#[test]
fn transfer_keeps_source_when_destination_is_unwritable() {
    if !read_only_dirs_enforced() {
        eprintln!("skipping: permissions are not enforced for this user");
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("photo.jpg");
    std::fs::write(&source, vec![7u8; 256]).unwrap();
    let destination = root.path().join("locked");
    std::fs::create_dir_all(&destination).unwrap();
    set_mode(&destination, 0o555);

    let outcome =
        transfer_files_with_progress(&[source.clone()], &destination, Operation::Move, &|_, _| {});

    set_mode(&destination, 0o755);

    assert_eq!(outcome.transferred, 0, "{outcome:?}");
    assert_eq!(outcome.failures.len(), 1, "{outcome:?}");
    assert!(source.exists(), "source must survive a failed move");
    assert!(!destination.join("photo.jpg").exists());
}

#[cfg(unix)]
#[test]
fn transfer_reports_unreadable_source_and_keeps_it_in_place() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("unreadable.jpg");
    std::fs::write(&source, vec![1u8; 64]).unwrap();
    set_mode(&source, 0o000);
    let readable = std::fs::read(&source).is_ok();
    if readable {
        set_mode(&source, 0o644);
        eprintln!("skipping: permissions are not enforced for this user");
        return;
    }

    let destination = root.path().join("out");
    let outcome =
        transfer_files_with_progress(&[source.clone()], &destination, Operation::Move, &|_, _| {});

    set_mode(&source, 0o644);

    assert_eq!(outcome.transferred, 0, "{outcome:?}");
    assert_eq!(outcome.failures.len(), 1, "{outcome:?}");
    assert!(source.exists(), "unreadable source is left in place");
    assert!(!destination.exists() || !destination.join("unreadable.jpg").exists());
}

#[test]
fn transfer_mixed_batch_reports_missing_files_and_moves_the_rest() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("good.jpg");
    std::fs::write(&source, b"good").unwrap();
    let missing = root.path().join("missing.jpg");
    let destination = root.path().join("out");

    let outcome = transfer_files_with_progress(
        &[source.clone(), missing.clone()],
        &destination,
        Operation::Move,
        &|_, _| {},
    );

    assert_eq!(outcome.planned, 2);
    assert_eq!(outcome.transferred, 1, "{outcome:?}");
    assert_eq!(outcome.failures.len(), 1, "{outcome:?}");
    assert_eq!(outcome.failures[0].0, missing);
    assert!(!source.exists(), "valid file was moved");
    assert!(destination.join("good.jpg").exists());
}

#[test]
fn one_failed_decision_does_not_block_other_rows() {
    let root = tempfile::tempdir().unwrap();
    let a_root = root.path().join("library");
    let b_root = root.path().join("candidate");
    let purgatory = root.path().join("purgatory");
    std::fs::create_dir_all(&a_root).unwrap();
    std::fs::create_dir_all(&b_root).unwrap();

    let a_big = a_root.join("photo.jpg");
    std::fs::write(&a_big, vec![9u8; 4096]).unwrap();
    let b_small = b_root.join("photo.jpg");
    std::fs::write(&b_small, b"small").unwrap();

    let a_missing_row = a_root.join("other.jpg");
    std::fs::write(&a_missing_row, vec![8u8; 4096]).unwrap();
    let b_missing = b_root.join("other.jpg");

    let decisions = vec![
        (a_big.clone(), b_small.clone(), SimilarDecision::TakeBigger),
        (
            a_missing_row.clone(),
            b_missing.clone(),
            SimilarDecision::TakeBigger,
        ),
    ];
    let outcome = apply_similar_decisions_with_progress(
        &decisions,
        &a_root,
        &b_root,
        &purgatory,
        "YYYY/MM-DD",
        &|_, _| {},
    );

    assert_eq!(outcome.bigger_taken, 1, "{outcome:?}");
    assert_eq!(outcome.failures.len(), 1, "{outcome:?}");
    assert_eq!(outcome.failures[0].0, b_missing);
    assert!(a_big.exists(), "keeper side is untouched");
    assert!(!b_small.exists(), "smaller B went to purgatory");
    assert!(purgatory.join("photo.jpg").exists());
    assert!(a_missing_row.exists(), "failed row leaves its A copy alone");
}

#[test]
fn keep_both_collisions_are_renamed_not_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let a_root = root.path().join("library");
    let b_root = root.path().join("candidate");
    let purgatory = root.path().join("purgatory");
    std::fs::create_dir_all(&a_root).unwrap();

    let original = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    let fixture = std::fs::read(&original).unwrap();

    let a_one = a_root.join("one.jpg");
    std::fs::write(&a_one, b"a1").unwrap();
    let a_two = a_root.join("two.jpg");
    std::fs::write(&a_two, b"a2").unwrap();

    let b_one_dir = b_root.join("one");
    let b_two_dir = b_root.join("two");
    std::fs::create_dir_all(&b_one_dir).unwrap();
    std::fs::create_dir_all(&b_two_dir).unwrap();
    let b_one = b_one_dir.join("photo.jpg");
    let b_two = b_two_dir.join("photo.jpg");
    std::fs::write(&b_one, &fixture).unwrap();
    std::fs::write(&b_two, &fixture).unwrap();

    let decisions = vec![
        (a_one, b_one, SimilarDecision::KeepBoth),
        (a_two, b_two, SimilarDecision::KeepBoth),
    ];
    let outcome = apply_similar_decisions_with_progress(
        &decisions,
        &a_root,
        &b_root,
        &purgatory,
        "YYYY/MM-DD",
        &|_, _| {},
    );

    assert_eq!(outcome.kept_both, 2, "{outcome:?}");
    assert!(outcome.failures.is_empty(), "{outcome:?}");

    let dated = a_root.join("2022/08-17");
    let mut names: Vec<String> = std::fs::read_dir(&dated)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["photo (1).jpg".to_string(), "photo.jpg".to_string()],
        "both kept copies exist under unique names"
    );
    for name in names {
        assert_eq!(
            std::fs::read(dated.join(name)).unwrap(),
            fixture,
            "kept copy contents are intact"
        );
    }
}

#[test]
fn transfer_collision_renames_instead_of_overwriting() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("photo.jpg");
    std::fs::write(&source, b"new").unwrap();
    let destination = root.path().join("out");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("photo.jpg"), b"existing").unwrap();

    let outcome = transfer_files_with_progress(
        &[source.clone()],
        &destination,
        Operation::Copy,
        &|_, _| {},
    );

    assert_eq!(outcome.transferred, 1, "{outcome:?}");
    assert_eq!(outcome.renamed_on_collision, 1);
    assert_eq!(std::fs::read(destination.join("photo.jpg")).unwrap(), b"existing");
    assert_eq!(std::fs::read(destination.join("photo (1).jpg")).unwrap(), b"new");
}

