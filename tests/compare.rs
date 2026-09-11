use std::fs;
use std::path::{Path, PathBuf};

use dedupe2::Scanner::compare::compare_folders;
use dedupe2::exif::creation_date;

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

#[test]
fn compare_sets_reports_the_four_numbers() {
    use dedupe2::Scanner::compare::compare_sets;

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let c = root.path().join("c");
    for d in [&a, &b, &c] {
        std::fs::create_dir_all(d).unwrap();
    }

    let img = sample("jpg-exif-mod/image1.JPG");      // shared photo
    let img_exif = sample("jpg-exif-mod/image1-exif.JPG"); // same image data
    let heic = sample("HEIC-exif-mod/heic1.HEIC");   // distinct image

    // Two distinct synthesized photos (different pixels -> different hashes).
    let synth = |seed: u8| {
        let img = image::DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(8, 8, |x, y| {
            image::Rgb([((x as u8 * 13).wrapping_add(seed)), ((y as u8 * 29).wrapping_add(seed)), seed])
        }));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)
            .unwrap();
        buf
    };
    let a_only_img = synth(1);
    let shared_img = synth(2);

    // A: ok.jpg (matched by B and C), a_only.jpg (unique to A)
    std::fs::write(a.join("ok.jpg"), &img).unwrap();
    std::fs::write(a.join("a_only.jpg"), &a_only_img).unwrap();

    // B: b_dup.jpg (in A), b_only.heic (unique to B), b_and_c.jpg (also in C)
    std::fs::write(b.join("b_dup.jpg"), &img_exif).unwrap();
    std::fs::write(b.join("b_only.heic"), &heic).unwrap();
    std::fs::write(b.join("b_and_c.jpg"), &shared_img).unwrap();

    // C: c_dup.jpg (in A), c_and_b.jpg (same bytes as B's b_and_c.jpg)
    std::fs::write(c.join("c_dup.jpg"), &img).unwrap();
    std::fs::write(c.join("c_and_b.jpg"), &shared_img).unwrap();

    let cmp = compare_sets(&a, &[b, c]).unwrap();

    // #1 unique to A
    assert_eq!(cmp.a_unique, 1);
    // #2 unique to B / unique to C (not in A, not in any other set)
    assert_eq!(cmp.candidates[0].unique_to_tree, 1); // b_only.jpg
    assert_eq!(cmp.candidates[1].unique_to_tree, 0); // c_and_b is shared with B
    // #3 candidates-only (not in A)
    assert_eq!(cmp.candidates_only, 3); // b_only + b_and_c + c_and_b
    // #4 duplicates between A and B+C
    assert_eq!(cmp.duplicates, 2); // b_dup + c_dup
}

#[test]
fn destination_root_ending_with_year_is_not_doubled() {
    use dedupe2::filemover::{destination_for, Operation, transfer_originals_with_progress};
    use dedupe2::exif::{creation_date, CreationDate};

    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("candidate/03-15 hawaii");
    std::fs::create_dir_all(&candidate).unwrap();

    let img = sample("jpg-exif-mod/image1.JPG"); // EXIF date 2022-08-17
    std::fs::write(candidate.join("new1.jpg"), &img).unwrap();

    let date = creation_date(&candidate.join("new1.jpg"));
    assert!(matches!(date, CreationDate::DateCreated(_)));

    // Destination root already ends with the year 2022.
    let destination = root.path().join("library/2022");
    let target = destination_for(
        &candidate.join("new1.jpg"),
        &date,
        &destination,
        "YYYY/MM-DD <folder description>",
    )
    .unwrap();
    let target_str = target.display().to_string();
    assert!(!target_str.contains("2022/2022"), "doubled year in {target_str}");
    assert!(target_str.ends_with("library/2022/08-17 hawaii/new1.jpg"), "{target_str}");

    // And the actual transfer lands there.
    let outcome = transfer_originals_with_progress(
        &[candidate.join("new1.jpg")],
        &destination,
        "YYYY/MM-DD <folder description>",
        Operation::Move,
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(outcome.copied, 1);
    assert!(destination.join("08-17 hawaii/new1.jpg").exists());
}

#[test]
fn full_date_source_folder_is_treated_as_date_not_description() {
    use dedupe2::filemover::{destination_for, Operation, transfer_originals_with_progress};

    let root = tempfile::tempdir().unwrap();
    let img = sample("jpg-exif-mod/image1.JPG"); // EXIF date 2022-08-17

    // Candidate folder named `YYYY-MM-DD` (no description).
    let candidate = root.path().join("candidate/2013-04-05");
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::write(candidate.join("new1.jpg"), &img).unwrap();

    let destination = root.path().join("library");
    let target = destination_for(
        &candidate.join("new1.jpg"),
        &creation_date(&candidate.join("new1.jpg")),
        &destination,
        "YYYY/MM-DD <folder description>",
    )
    .unwrap();
    let target_str = target.display().to_string();
    assert!(
        target_str.ends_with("library/2022/08-17/new1.jpg"),
        "got {target_str}"
    );

    // Candidate folder `YYYY-MM-DD <description>` keeps the description.
    let candidate2 = root.path().join("candidate2/2013-04-05 Hawaii");
    std::fs::create_dir_all(&candidate2).unwrap();
    std::fs::write(candidate2.join("new2.jpg"), &img).unwrap();

    let target2 = destination_for(
        &candidate2.join("new2.jpg"),
        &creation_date(&candidate2.join("new2.jpg")),
        &destination,
        "YYYY/MM-DD <folder description>",
    )
    .unwrap();
    assert!(
        target2.display().to_string().ends_with("library/2022/08-17 Hawaii/new2.jpg"),
        "got {}",
        target2.display()
    );
}

#[test]
fn verify_tree_flags_wrong_folder_placements() {
    use dedupe2::Scanner::compare::verify_tree_dates;

    let root = tempfile::tempdir().unwrap();
    let lib = root.path().join("library");
    let img = sample("jpg-exif-mod/image1.JPG"); // EXIF 2022-08-17
    let fake = b"\xFF\xD8 truncated, no exif";   // undetectable date

    // OK: folder date matches EXIF date.
    std::fs::create_dir_all(lib.join("2022/08-17 hawaii")).unwrap();
    std::fs::write(lib.join("2022/08-17 hawaii/ok.jpg"), &img).unwrap();

    // MISMATCH: file sits in 2010/05-30 but was taken 2022-08-17.
    std::fs::create_dir_all(lib.join("2010/05-30")).unwrap();
    std::fs::write(lib.join("2010/05-30/wrong.jpg"), &img).unwrap();

    // OUTSIDE: has EXIF but folder does not encode a date.
    std::fs::create_dir_all(lib.join("misc")).unwrap();
    std::fs::write(lib.join("misc/in_misc.jpg"), &img).unwrap();

    // UNDATED: no exif, folder does not encode a date.
    std::fs::write(lib.join("misc/noexif.jpg"), &fake).unwrap();

    let outcome = verify_tree_dates(&lib).unwrap();

    assert_eq!(outcome.total, 4);
    assert_eq!(outcome.ok, 1);
    assert_eq!(outcome.outside_structure, 1);
    assert_eq!(outcome.undated, 1);
    assert_eq!(outcome.mismatches.len(), 1);
    assert!(outcome.mismatches[0]
        .path
        .display()
        .to_string()
        .ends_with("2010/05-30/wrong.jpg"));
    assert_eq!(outcome.mismatches[0].exif_date, "2022-08-17");
    assert_eq!(outcome.mismatches[0].folder_date, "2010-05-30");
}

#[test]
fn rehome_is_safe_copy_verify_then_remove() {
    use dedupe2::filemover::{plan_rehome, rehome_verified};

    let root = tempfile::tempdir().unwrap();
    let lib = root.path().join("library");
    let img = sample("jpg-exif-mod/image1.JPG"); // EXIF 2022-08-17

    // Mismatch: file sits in 2010/05-30, belongs at 2022/08-17.
    std::fs::create_dir_all(lib.join("2010/05-30")).unwrap();
    std::fs::write(lib.join("2010/05-30/wrong.jpg"), &img).unwrap();

    // Pre-existing destination file with the same name — must NOT be lost.
    std::fs::create_dir_all(lib.join("2022/08-17")).unwrap();
    std::fs::write(lib.join("2022/08-17/wrong.jpg"), b"PRE-EXISTING-KEEP-ME").unwrap();

    let mismatches = vec![(
        lib.join("2010/05-30/wrong.jpg"),
        "2022-08-17".to_string(),
    )];
    let plans = plan_rehome(&mismatches, &lib, "YYYY/MM-DD <folder description>");
    assert_eq!(plans.len(), 1);
    assert!(plans[0]
        .target
        .display()
        .to_string()
        .ends_with("2022/08-17/wrong.jpg"));

    let outcome = rehome_verified(&plans, &|_, _| {});
    assert_eq!(outcome.moved, 1);
    assert_eq!(outcome.renamed_on_collision, 1);
    assert_eq!(outcome.failures.len(), 0);

    // The moved file is byte-identical at its new home.
    assert_eq!(fs::read(lib.join("2022/08-17/wrong (1).jpg")).unwrap(), img);
    // The pre-existing destination file is untouched.
    assert_eq!(
        fs::read(lib.join("2022/08-17/wrong.jpg")).unwrap(),
        b"PRE-EXISTING-KEEP-ME"
    );
    // The source is gone (only after verification).
    assert!(!lib.join("2010/05-30/wrong.jpg").exists());
}

#[test]
fn rehome_never_overwrites_case_insensitive_name_collision() {
    use dedupe2::filemover::{plan_rehome, rehome_verified};

    let root = tempfile::tempdir().unwrap();
    let lib = root.path().join("library");

    // The destination folder already contains IMG_0500.JPG (uppercase ext),
    // holding content A. The incoming file is IMG_0500.jpg (lowercase ext),
    // content B — on a case-insensitive filesystem these are the SAME file.
    std::fs::create_dir_all(lib.join("2012/09-27")).unwrap();
    std::fs::create_dir_all(lib.join("2012/11-06")).unwrap();
    std::fs::write(lib.join("2012/09-27/IMG_0500.JPG"), b"CONTENT-A-HIGH-RES").unwrap();
    let incoming = lib.join("2012/11-06/IMG_0500.jpg");
    std::fs::write(&incoming, b"content-b-low-res").unwrap();

    let mismatches = vec![(incoming.clone(), "2012-09-27".to_string())];
    let plans = plan_rehome(&mismatches, &lib, "YYYY/MM-DD <folder description>");
    let outcome = rehome_verified(&plans, &|_, _| {});

    assert_eq!(outcome.moved, 1);

    // Content A must be intact at its original (case-variant) name.
    assert_eq!(
        fs::read(lib.join("2012/09-27/IMG_0500.JPG")).unwrap(),
        b"CONTENT-A-HIGH-RES"
    );
    // Content B must be present under an auto-renamed name.
    let landed = lib.join("2012/09-27/IMG_0500 (1).jpg");
    assert!(landed.exists(), "incoming file must be renamed, not overwrite");
    assert_eq!(fs::read(landed).unwrap(), b"content-b-low-res");
}
