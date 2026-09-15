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

#[test]
fn transfer_uses_carried_dates_without_reextraction() {
    use dedupe2::filemover::{transfer_dated_with_progress, Operation};
    use dedupe2::exif::{creation_date, CreationDate};

    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("candidate/09-27");
    std::fs::create_dir_all(&candidate).unwrap();

    // This file's real EXIF date is 2022-08-17...
    std::fs::write(candidate.join("IMG_0500.jpg"), &sample("jpg-exif-mod/image1.JPG")).unwrap();

    // ...but we CARRY a different (parseable) date. If the transfer re-extracted,
    // the file would land in 2022/08-17 instead.
    let carried = vec![(
        candidate.join("IMG_0500.jpg"),
        CreationDate::DateCreated("2010-05-30 06:59:32".into()),
    )];

    let outcome = transfer_dated_with_progress(
        &carried,
        &root.path().join("library"),
        "YYYY/MM-DD <folder description>",
        Operation::Move,
        &|_, _| {},
    )
    .unwrap();

    assert_eq!(outcome.copied, 1);
    assert!(root
        .path()
        .join("library/2010/05-30/IMG_0500.jpg")
        .exists(), "carried date must be used, not re-extracted");
    assert!(!root
        .path()
        .join("library/2022/08-17/IMG_0500.jpg")
        .exists());
    assert!(!candidate.join("IMG_0500.jpg").exists()); // moved
}

#[test]
fn transfer_falls_back_to_extraction_for_files_without_carried_dates() {
    use dedupe2::filemover::{transfer_dated_with_progress, Operation};
    use dedupe2::exif::{creation_date, CreationDate};

    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("candidate");
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::write(candidate.join("dated.jpg"), &sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(candidate.join("undated.jpg"), b"\xFF\xD8 no exif").unwrap();

    // Carry a date only for the first file; the second must fall back to
    // on-demand extraction (which fails for this garbage jpeg -> skipped).
    let dated = vec![(
        candidate.join("dated.jpg"),
        CreationDate::DateCreated("2013-07-07".into()),
    )];

    let outcome = transfer_dated_with_progress(
        &dated,
        &root.path().join("library"),
        "YYYY/MM-DD <folder description>",
        Operation::Move,
        &|_, _| {},
    )
    .unwrap();

    assert_eq!(outcome.copied, 1);
    // Files absent from the carried list are never processed at all —
    // they simply stay where they are.
    assert_eq!(outcome.skipped_no_exif, 0);
    assert!(root
        .path()
        .join("library/2013/07-07 candidate/dated.jpg")
        .exists());
    assert!(!candidate.join("dated.jpg").exists()); // moved
    assert!(candidate.join("undated.jpg").exists()); // skipped, still there
}

#[test]
fn description_tokens_survive_date_substitution() {
    use dedupe2::filemover::{destination_for, Operation, transfer_dated_with_progress};
    use dedupe2::exif::CreationDate;

    let root = tempfile::tempdir().unwrap();

    // A folder literally named "MM": its description must stay "MM", not
    // become the month number.
    let candidate = root.path().join("candidate/MM");
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::write(candidate.join("x.jpg"), &sample("jpg-exif-mod/image1.JPG")).unwrap();

    let date = CreationDate::DateCreated("2012-08-17 12:00:00".into());
    let target = destination_for(
        &candidate.join("x.jpg"),
        &date,
        &root.path().join("library"),
        "YYYY/MM-DD <folder description>",
    )
    .unwrap();
    let target_str = target.display().to_string();
    assert!(
        target_str.ends_with("library/2012/08-17 MM/x.jpg"),
        "description corrupted by token substitution: {target_str}"
    );

    // And the actual transfer lands there too.
    let outcome = transfer_dated_with_progress(
        &[(candidate.join("x.jpg"), date)],
        &root.path().join("library"),
        "YYYY/MM-DD <folder description>",
        Operation::Move,
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(outcome.copied, 1);
    assert!(root.path().join("library/2012/08-17 MM/x.jpg").exists());
}


fn boxed(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let size = (payload.len() as u32) + 8;
    let mut v = Vec::with_capacity(size as usize);
    v.extend_from_slice(&size.to_be_bytes());
    v.extend_from_slice(tag);
    v.extend_from_slice(payload);
    v
}

fn synthetic_mp4(mdat_payload: &[u8]) -> Vec<u8> {
    let ftyp = boxed(b"ftyp", b"isomisom");
    let moov = boxed(b"moov", &[0u8; 128]);
    let mdat = boxed(b"mdat", mdat_payload);
    [ftyp, moov, mdat].concat()
}

#[test]
fn deep_scan_treats_videos_differing_past_the_64kb_head_as_false_positives() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    let mut common = vec![0xABu8; 70 * 1024];

    let mut a_payload = common.clone();
    a_payload.extend_from_slice(b"AAAA-tails");
    let mut b_payload = common;
    b_payload.extend_from_slice(b"BBBB-tails");

    let a_path = root.path().join("a.mp4");
    let b_path = root.path().join("b.mp4");
    fs::write(&a_path, synthetic_mp4(&a_payload)).unwrap();
    fs::write(&b_path, synthetic_mp4(&b_payload)).unwrap();

    // The two differ only past the first 64kb, so a header scan pairs them.
    use dedupe2::image_reader::hash_image_data_status;
    assert_eq!(
        hash_image_data_status(&a_path, 64 * 1024).0,
        hash_image_data_status(&b_path, 64 * 1024).0,
        "fixture should share the 64kb head hash"
    );

    let outcome = deep_scan_pairs(&[DupPair { a: a_path, b: b_path }]);
    assert_eq!(outcome.checked, 1);
    assert_eq!(outcome.kept, 0, "videos with different full payloads are not duplicates");
    assert_eq!(outcome.removed.len(), 1);
}

#[test]
fn deep_scan_keeps_videos_with_identical_content() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    let payload = [0xCDu8; 80 * 1024];
    let movie = synthetic_mp4(&payload);

    let a_path = root.path().join("a.mp4");
    let b_path = root.path().join("b.mov");
    fs::write(&a_path, &movie).unwrap();
    fs::write(&b_path, &movie).unwrap();

    let outcome = deep_scan_pairs(&[DupPair { a: a_path, b: b_path }]);
    assert_eq!(outcome.kept, 1);
    assert!(outcome.removed.is_empty());
}

#[test]
fn deep_scan_keeps_photos_identical_except_metadata() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    let a_path = root.path().join("a.jpg");
    let b_path = root.path().join("b.jpg");
    write(&a_path, "a.jpg", &sample("jpg-exif-mod/image1.JPG"));

    // Same photo; flip one byte inside the EXIF (APP1) segment.
    let mut bytes = sample("jpg-exif-mod/image1.JPG");
    let exif = bytes
        .windows(4)
        .position(|w| w == b"Exif")
        .expect("fixture should carry an Exif segment");
    let flip = exif + 6;
    assert!(bytes[flip] != 0xFF);
    bytes[flip] ^= 0x01;
    write(&b_path, "b.jpg", &bytes);

    let outcome = deep_scan_pairs(&[DupPair { a: a_path, b: b_path }]);
    assert_eq!(outcome.kept, 1, "metadata-only differences must stay duplicates");
    assert!(outcome.removed.is_empty());
}

#[test]
fn cross_format_pairs_are_never_grouped_as_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");

    // Same photo, camera JPEG and its PNG re-encode.
    let jpg = sample("jpg-exif-mod/image1.JPG");
    let decoded = image::load_from_memory(&jpg).unwrap();
    let mut png = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();

    write(&a, "photo.jpg", &jpg);
    write(&b, "photo.png", &png);

    let cmp = compare_folders(&a, &b).unwrap();
    assert!(
        cmp.duplicates.is_empty(),
        "JPEG and PNG of the same photo must NOT pair: found {:?}",
        cmp.duplicates
    );
    assert_eq!(names(&cmp.a_only), vec!["photo.jpg".to_string()]);
}

#[test]
fn raw_and_jpg_sidecars_are_never_grouped_as_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");

    // The CR2's embedded preview is the same photo as image1.JPG. The RAW's
    // sensor data must never hash-match the JPEG's scan data.
    write(&a, "shot.CR2", &sample("canon-raw/CRW_6542.CRW"));
    write(&b, "shot.jpg", &sample("jpg-exif-mod/image1.JPG"));

    let cmp = compare_folders(&a, &b).unwrap();
    assert!(
        cmp.duplicates.is_empty(),
        "RAW and JPEG sidecars must NOT pair: found {:?}",
        cmp.duplicates
    );
    assert_eq!(names(&cmp.b_only), vec!["shot.jpg".to_string()]);
}

#[test]
fn deep_scan_confirms_byte_identical_duplicates_in_cross_product_groups() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let img = sample("jpg-exif-mod/image1.JPG");
    write(&a, "x1.jpg", &img);
    write(&a, "x2.jpg", &img);
    write(&b, "y1.jpg", &img);
    write(&b, "y2.jpg", &img);

    let cmp = compare_folders(&a, &b).unwrap();
    assert_eq!(cmp.duplicates.len(), 1);
    let group = &cmp.duplicates[0];
    assert_eq!(group.a.len(), 2);
    assert_eq!(group.b.len(), 2);

    let pairs: Vec<DupPair> = group
        .a
        .iter()
        .flat_map(|x| group.b.iter().map(move |y| DupPair { a: x.clone(), b: y.clone() }))
        .collect();
    assert_eq!(pairs.len(), 4);

    let outcome = deep_scan_pairs(&pairs);
    assert_eq!(outcome.kept, 4, "byte-identical cross pairs must all be confirmed");
    assert!(outcome.removed.is_empty(), "removed: {:?}", outcome.removed);
}

#[test]
fn deep_scan_confirms_byte_identical_raw_pairs() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let raw = sample("raw-exif-mod/raw_rw2-exif.rw2");
    write(&a, "shot.rw2", &raw);
    write(&b, "shot-copy.rw2", &raw);

    let outcome = deep_scan_pairs(&[DupPair {
        a: a.join("shot.rw2"),
        b: b.join("shot-copy.rw2"),
    }]);
    assert_eq!(outcome.kept, 1);
    assert!(outcome.removed.is_empty());
}

/// Environment-specific regression: the real IMG_0857.MOV pair that was once
/// (wrongly, per the user) reported as a false positive. Pinned to the
/// library owner's `4tbext` paths, so it is ignored by default — run with
/// `cargo test --test compare fixed_0857_pair -- --ignored --nocapture`.
/// Verifies the whole verdict chain on ~503mb videos: the shallow 64kb hash
/// matches (that is what groups them), the full byte and full-mdat hashes
/// agree, and deep scan therefore confirms — never removes — the pair.
#[test]
#[ignore]
fn fixed_0857_pair_shallow_then_full_hash_confirmed() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};
    use dedupe2::image_reader::{hash_all_bytes, hash_image_data_all, hash_image_data_status};

    let a = std::path::Path::new(
        "/Users/dannydalal/4tbext/SrcImageFolders/Single Events/Original 2000-2019/2013/2013-07-30/IMG_0857.MOV",
    );
    let b = std::path::Path::new("/Users/dannydalal/4tbext/AllPhotos/2013/07-30/IMG_0857.MOV");
    if !(a.exists() && b.exists()) {
        // These paths follow the library owner's organization workflow (the
        // copy may have been purged/renamed since the bug report).
        eprintln!("library files missing — skip");
        return;
    }

    // Layer 1: shallow 64kb image-data hash — what paired them in the UI.
    let (sha, sa_ok) = hash_image_data_status(a, 64 * 1024);
    let (shb, sb_ok) = hash_image_data_status(b, 64 * 1024);
    assert!(sa_ok && sb_ok, "both must decode ({sa_ok}, {sb_ok})");
    assert_eq!(sha, shb, "shallow hashes must be the reason they paired");

    // Layer 2: full byte hash.
    let ha = hash_all_bytes(a).unwrap();
    let hb = hash_all_bytes(b).unwrap();
    assert_eq!(ha, hb, "files must be byte-identical");

    // Layer 3: full image-data (complete mdat payload) hash.
    let ma = hash_image_data_all(a).unwrap();
    let mb = hash_image_data_all(b).unwrap();
    assert_eq!(ma, mb, "full video payloads must be identical");

    let outcome = deep_scan_pairs(&[DupPair {
        a: a.to_path_buf(),
        b: b.to_path_buf(),
    }]);
    assert_eq!(outcome.kept, 1, "confirmed duplicates: {outcome:?}");
    assert!(outcome.removed.is_empty(), "removed: {outcome:?}");
}

/// Build many distinct video payloads that share the same 64kb head, so the
/// shallow scan buckets them together even though their content differs.
fn head_shared_video(tail: &[u8]) -> Vec<u8> {
    let mut payload = vec![0xABu8; 70 * 1024];
    payload.extend_from_slice(tail);
    let ftyp = boxed(b"ftyp", b"isomisom");
    let moov = boxed(b"moov", &[0u8; 128]);
    let mdat = boxed(b"mdat", &payload);
    [ftyp, moov, mdat].concat()
}

#[test]
fn deep_scan_reports_no_false_positives_or_originals_when_every_b_matches_some_a() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let v1 = head_shared_video(b"AAAA-actual-content"); // a1 == b1
    let v2 = head_shared_video(b"BBBB-different-content"); // a2 == b2

    write(&a, "a1.mp4", &v1);
    write(&a, "a2.mp4", &v2);
    write(&b, "b1.mp4", &v1);
    write(&b, "b2.mp4", &v2);

    // Shallow scan buckets all four together (their 64kb heads are equal).
    let cmp = compare_folders(&a, &b).unwrap();
    assert_eq!(cmp.duplicates.len(), 1);
    let group = &cmp.duplicates[0];
    assert_eq!(group.a.len(), 2);
    assert_eq!(group.b.len(), 2);

    // Full A x B cross product, exactly as the web layer posts it.
    let pairs: Vec<DupPair> = group
        .a
        .iter()
        .flat_map(|x| group.b.iter().map(move |y| DupPair { a: x.clone(), b: y.clone() }))
        .collect();
    assert_eq!(pairs.len(), 4);

    let outcome = deep_scan_pairs(&pairs);
    // (a1,b1) and (a2,b2) confirmed; (a1,b2)/(a2,b1) are shallow-only links.
    assert_eq!(outcome.kept, 2, "{outcome:?}");
    assert!(outcome.removed.is_empty(), "cross rows must be suppressed: {outcome:?}");
    assert_eq!(outcome.covered, 2);
    // No B file is a genuine false positive, so nothing feeds the originals.
    let promoted: Vec<_> = outcome.removed.iter().map(|p| p.b.to_string_lossy()).collect();
    assert!(promoted.is_empty());
}

#[test]
fn deep_scan_keeps_false_positive_when_b_matches_no_a_at_all() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    // Two A files, one B; B shares the head with both but duplicates neither.
    let a1 = root.path().join("a1.mp4");
    let a2 = root.path().join("a2.mp4");
    let b1 = root.path().join("b1.mp4");
    fs::write(&a1, head_shared_video(b"AAAA-actual-content")).unwrap();
    fs::write(&a2, head_shared_video(b"BBBB-different-content")).unwrap();
    fs::write(&b1, head_shared_video(b"CCCC-third-content")).unwrap();

    let outcome = deep_scan_pairs(&[
        DupPair { a: a1.clone(), b: b1.clone() },
        DupPair { a: a2.clone(), b: b1.clone() },
    ]);
    assert_eq!(outcome.kept, 0);
    assert_eq!(outcome.removed.len(), 2, "both comparisons genuinely disagreed: {outcome:?}");
    // B matched nothing, so exactly one B path surfaces for originals
    // promotion (deduplicated downstream, but each row must carry it).
    let promoted: Vec<_> = outcome.removed.iter().map(|p| p.b.to_string_lossy()).collect();
    assert_eq!(promoted, vec![b1.display().to_string(), b1.display().to_string()]);
}

#[test]
fn deep_scan_keeps_false_positive_rows_for_b_matching_nothing_even_with_confirmed_siblings() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    // Two A files, two B files: b1 duplicates a1; b2 matches NOTHING.
    let a1 = root.path().join("a1.mp4");
    let a2 = root.path().join("a2.mp4");
    let b1 = root.path().join("b1.mp4");
    let b2 = root.path().join("b2.mp4");
    fs::write(&a1, head_shared_video(b"AAAA-actual-content")).unwrap();
    fs::write(&a2, head_shared_video(b"BBBB-different-content")).unwrap();
    fs::write(&b1, head_shared_video(b"AAAA-actual-content")).unwrap();
    fs::write(&b2, head_shared_video(b"CCCC-third-content")).unwrap();

    let outcome = deep_scan_pairs(&[
        DupPair { a: a1, b: b1 },
        DupPair { a: a2, b: b2 },
    ]);
    assert_eq!(outcome.kept, 1, "{outcome:?}");
    assert_eq!(outcome.removed.len(), 1, "{outcome:?}");
    assert_eq!(
        outcome.removed[0].b.file_name().unwrap().to_string_lossy(),
        "b2.mp4",
        "only the genuinely unmatched B may surface: {outcome:?}"
    );
}

/// Minimal M2TS (AVCHD): 192-byte packets = 4-byte arrival timestamp +
/// 0x47 sync + 187 content bytes.
fn synthesize_m2ts(content: &[u8], timestamp_base: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ts = timestamp_base;
    for (i, chunk) in content.chunks(187).enumerate() {
        out.extend_from_slice(&ts.to_be_bytes());
        out.push(0x47);
        out.extend_from_slice(chunk);
        for _ in chunk.len()..187 {
            out.push(0);
        }
        ts += 1000 + i as u32;
    }
    out
}

#[test]
fn mts_files_are_scanned_compared_and_deep_scanned() {
    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");

    // Same clip, copy in each tree — different arrival timestamps.
    let content: Vec<u8> = (0..60_000u32).map(|i| ((i * 7 + 3) % 251) as u8).collect();
    write(&a, "clip.mts", &synthesize_m2ts(&content, 0));
    write(&b, "clip.mts", &synthesize_m2ts(&content, 500_000));

    // Shallow scan: found as duplicates despite different packet timestamps.
    let cmp = compare_folders(&a, &b).unwrap();
    assert_eq!(cmp.duplicates.len(), 1, "mts must participate in compare");
    assert!(cmp.unreadable.is_empty(), "mts must decode, not be unreadable");

    let group = &cmp.duplicates[0];
    let outcome = deep_scan_pairs(&[DupPair {
        a: group.a[0].clone(),
        b: group.b[0].clone(),
    }]);
    assert_eq!(outcome.kept, 1, "timestamp-only differences stay duplicates");
    assert!(outcome.removed.is_empty());
}

#[test]
fn mislabeled_jpeg_named_mov_still_pairs_with_its_jpg_twin() {
    // The real-world case: the same photo exists as photo.jpg in A and as a
    // .MOV-mislabeled copy in B. Content sniffing must hash both identically
    // so the compare finds the duplicate instead of listing it unreadable.
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let jpeg = sample("jpg-exif-mod/image1.JPG");

    write(&a, "photo.jpg", &jpeg);
    write(&b, "photo.mov", &jpeg);

    let cmp = compare_folders(&a, &b).unwrap();
    assert_eq!(cmp.duplicates.len(), 1, "mislabeled twin must pair: {cmp:?}");
    assert!(cmp.unreadable.is_empty(), "must not be unreadable: {:?}", cmp.unreadable);
}

#[test]
fn avi_copies_pair_across_trees_despite_metadata_differences() {
    // Same recorded content; different INFO/IDIT metadata. They must still be
    // found as duplicates (movi payload hashing) and stay confirmed in the
    // deep scan (byte-level differences do not matter for the content hash).
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let content = (0..40_000u32).map(|i| ((i * 5 + 1) % 253) as u8).collect::<Vec<_>>();

    write(&a, "clip.avi", &synthesize_avi(&content, b"2006:07:15 10:49:34"));
    write(&b, "clip.avi", &synthesize_avi(&content, b"2007:01:01 00:00:00"));

    let cmp = compare_folders(&a, &b).unwrap();
    assert_eq!(cmp.duplicates.len(), 1, "avi must pair: {cmp:?}");
    assert!(cmp.unreadable.is_empty());

    use dedupe2::Scanner::compare::{deep_scan_pairs, DupPair};
    let group = &cmp.duplicates[0];
    let outcome = deep_scan_pairs(&[DupPair {
        a: group.a[0].clone(),
        b: group.b[0].clone(),
    }]);
    assert_eq!(outcome.kept, 1, "movi-identical avi pair must stay confirmed: {outcome:?}");
}

fn synthesize_avi(movi: &[u8], idit: &[u8]) -> Vec<u8> {
    let riff_chunk = |id: &[u8; 4], payload: &[u8]| {
        let mut v = Vec::new();
        v.extend_from_slice(id);
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            v.push(0);
        }
        v
    };
    let hdrl = riff_chunk(b"LIST", &[b"hdrl".as_slice(), &riff_chunk(b"IDIT", idit)].concat());
    let movi_list = riff_chunk(b"LIST", &[b"movi".as_slice(), movi].concat());
    let body = [hdrl, movi_list].concat();

    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(body.len() as u32).to_le_bytes());
    v.extend_from_slice(b"AVI ");
    v.extend_from_slice(&body);
    v
}
