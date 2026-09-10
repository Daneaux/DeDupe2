use std::fs;
use std::path::{Path, PathBuf};

use dedupe2::Scanner::compare::compare_folders;

fn sample(rel: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages").join(rel);
    fs::read(path).expect("sample image should exist")
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

fn names(paths: &[PathBuf]) -> Vec<String> {
    let mut v: Vec<String> = paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn compare_reports_duplicates_and_originals() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");

    let img = sample("jpg-exif-mod/image1.JPG");
    let img_exif = sample("jpg-exif-mod/image1-exif.JPG");
    let a_only = sample("jpg-dupe-diff-size/IMG_2571.jpg");
    let b_only = sample("HEIC-exif-mod/heic1.HEIC");

    write(&a, "a1.jpg", &img);
    write(&a, "a2.jpg", &a_only);
    write(&b, "b1.jpg", &img_exif);
    write(&b, "b2.heic", &b_only);

    let cmp = compare_folders(&a, &b).unwrap();

    assert_eq!(cmp.duplicates.len(), 1);
    assert_eq!(names(&cmp.duplicates[0].a), vec!["a1.jpg".to_string()]);
    assert_eq!(names(&cmp.duplicates[0].b), vec!["b1.jpg".to_string()]);

    assert_eq!(names(&cmp.a_only), vec!["a2.jpg".to_string()]);
    assert_eq!(names(&cmp.b_only), vec!["b2.heic".to_string()]);
}



#[test]
fn compare_with_no_overlap_reports_all_originals() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");

    write(&a, "x.jpg", &sample("jpg-exif-mod/image1.JPG"));
    write(&b, "y.heic", &sample("HEIC-exif-mod/heic1.HEIC"));

    let cmp = compare_folders(&a, &b).unwrap();

    assert!(cmp.duplicates.is_empty());
    assert_eq!(names(&cmp.a_only), vec!["x.jpg".to_string()]);
    assert_eq!(names(&cmp.b_only), vec!["y.heic".to_string()]);
}

#[test]
fn render_date_folder_tokens() {
    use dedupe2::filemover::render_date_folder;

    assert_eq!(
        render_date_folder("YYYY/MM-DD <folder description>", 2020, 1, 1, "hawaii"),
        "2020/01-01 hawaii"
    );
    // no description: token and its separator are dropped
    assert_eq!(
        render_date_folder("YYYY/MM-DD <folder description>", 2020, 1, 1, ""),
        "2020/01-01"
    );
    assert_eq!(
        render_date_folder("YYYY-MM-DD DESC", 2023, 12, 5, "germany wedding"),
        "2023-12-05 germany wedding"
    );
}

#[test]
fn copy_originals_copies_only_valid_exif_into_formatted_folders() {
    use dedupe2::filemover::copy_originals;

    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("candidate/03-15 hawaii");
    let destination = root.path().join("library");

    std::fs::create_dir_all(&candidate).unwrap();
    let img = sample("jpg-exif-mod/image1.JPG"); // has EXIF date
    std::fs::write(candidate.join("new1.jpg"), &img).unwrap();
    std::fs::write(candidate.join("new2.jpg"), &img).unwrap();
    // a file with no recognizable EXIF
    std::fs::write(candidate.join("noexif.jpg"), b"\xFF\xD8 truncated").unwrap();

    let originals = vec![candidate.join("new1.jpg"), candidate.join("new2.jpg"), candidate.join("noexif.jpg")];
    let outcome = copy_originals(&originals, &destination, "YYYY/MM-DD <folder description>").unwrap();

    assert_eq!(outcome.copied, 2);
    assert_eq!(outcome.skipped_no_exif, 1);

    // folder comes from EXIF date + source folder description
    let dest_dir = destination.join("2022/08-17 hawaii");
    assert!(dest_dir.join("new1.jpg").exists());
    assert!(dest_dir.join("new2.jpg").exists());
    assert!(!destination.join("2022/08-17 hawaii/noexif.jpg").exists());

    // source files remain (copy, not move)
    assert!(candidate.join("new1.jpg").exists());
}

#[test]
fn undecodable_files_are_listed_as_unreadable() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");

    write(&a, "ok.jpg", &sample("jpg-exif-mod/image1.JPG"));
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("broken.cr2"), b"\xFF\x44\x00\x17\xAB\xCD\xEF\x01").unwrap();
    std::fs::write(a.join("broken-too.cr2"), b"\xFF\x44\x00\x17\xAB\xCD\xEF\x02").unwrap();

    let cmp = compare_folders(&a, &b).unwrap();

    // Only candidate-tree (B) files are surfaced as unreadable; the broken
    // file in the destination is ignored.
    assert_eq!(names(&cmp.unreadable), vec!["broken.cr2".to_string()]);
    assert!(cmp.b_only.is_empty());
    assert!(cmp.duplicates.is_empty());
    assert_eq!(names(&cmp.a_only), vec!["ok.jpg".to_string()]);
}

#[test]
fn transfer_never_overwrites_preexisting_destination_files() {
    use dedupe2::filemover::transfer_originals_with_progress;
    use dedupe2::filemover::Operation;

    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("candidate/03-15 hawaii");
    let destination = root.path().join("library/2022/08-17 hawaii");

    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::create_dir_all(&destination).unwrap();

    let img = sample("jpg-exif-mod/image1.JPG");
    // Pre-existing destination file with the SAME name the transfer will use.
    std::fs::write(destination.join("new1.jpg"), b"PRE-EXISTING-DO-NOT-LOSE").unwrap();
    std::fs::write(candidate.join("new1.jpg"), &img).unwrap();

    let originals = vec![candidate.join("new1.jpg")];
    let outcome = transfer_originals_with_progress(
        &originals,
        &root.path().join("library"),
        "YYYY/MM-DD <folder description>",
        Operation::Move,
        &|_, _| {},
    )
    .unwrap();

    assert_eq!(outcome.copied, 1);

    // The pre-existing file is intact, the transferred one was auto-renamed.
    assert_eq!(
        std::fs::read(destination.join("new1.jpg")).unwrap(),
        b"PRE-EXISTING-DO-NOT-LOSE"
    );
    assert!(destination.join("new1 (1).jpg").exists());
    assert!(!candidate.join("new1.jpg").exists()); // moved
}
