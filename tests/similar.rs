use std::path::Path;

use dedupe2::Scanner::cache::{lock, ScanCache};
use dedupe2::Scanner::similar::find_similar_with_progress;

fn sample(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages")
        .join(rel)
}

fn write_scaled(source: &Path, target: &Path, scale: f32, quality: u8) {
    let img = image::open(source).expect("fixture decodes");
    let (w, h) = (img.width() as f32 * scale, img.height() as f32 * scale);
    let small = img.resize_exact(w as u32, h as u32, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder.encode_image(&small).expect("encodes");
    std::fs::write(target, out).unwrap();
}

#[test]
fn similar_groups_keep_the_largest_copy_and_flag_smaller_ones() {
    let root = tempfile::tempdir().unwrap();
    let original = sample("jpg-exif-mod/image1.JPG");
    let keeper = root.path().join("original.jpg");
    std::fs::copy(&original, &keeper).unwrap();

    let half = root.path().join("half.jpg");
    let third = root.path().join("third.jpg");
    write_scaled(&original, &half, 0.5, 85);
    write_scaled(&original, &third, 0.34, 70);

    // Non-lossy and unrelated files must never be grouped.
    let unrelated = root.path().join("other.jpg");
    std::fs::copy(sample("jpg-dupe-diff-size/IMG_2571.jpg"), &unrelated).unwrap();
    let png = root.path().join("same-picture.png");
    image::open(&original)
        .unwrap()
        .save(&png)
        .expect("png written");

    let cache = std::sync::Mutex::new(ScanCache::new());
    let outcome = find_similar_with_progress(root.path(), &cache, &|_, _| {}).unwrap();

    assert_eq!(outcome.groups.len(), 1, "{outcome:?}");
    let group = &outcome.groups[0];
    assert_eq!(group.keeper, keeper, "largest copy is the keeper");
    let candidate_names: Vec<String> = group
        .candidates
        .iter()
        .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(candidate_names, vec!["half.jpg", "third.jpg"], "{outcome:?}");
    assert!(group.candidates.iter().all(|c| c.distance <= dedupe2::image_reader::SIMILAR_MAX_DISTANCE));
    assert!(group.candidates.iter().all(|c| c.pixels < group.keeper_pixels));
}

#[test]
fn similar_scan_reuses_cached_phashes() {
    let root = tempfile::tempdir().unwrap();
    let original = sample("jpg-exif-mod/image1.JPG");
    std::fs::copy(&original, root.path().join("a.jpg")).unwrap();
    write_scaled(&original, &root.path().join("b.jpg"), 0.5, 80);

    let cache = std::sync::Mutex::new(ScanCache::new());
    let first = find_similar_with_progress(root.path(), &cache, &|_, _| {}).unwrap();
    assert_eq!(first.groups.len(), 1);

    // Second pass: phashes all come from the cache.
    let before = lock(&cache).stats();
    let second = find_similar_with_progress(root.path(), &cache, &|_, _| {}).unwrap();
    let after = lock(&cache).stats();
    assert_eq!(second.groups.len(), 1);
    assert!(
        after.computed == before.computed,
        "second pass must not compute any new hash ({} -> {})",
        before.computed,
        after.computed
    );
    assert!(after.reused > before.reused);
    assert!(lock(&cache).branch_coverage(root.path()).phash >= 2);
}

#[test]
fn upscaled_copies_are_still_discovered_as_one_group() {
    let root = tempfile::tempdir().unwrap();
    let original = sample("jpg-exif-mod/image1.JPG");
    let keeper = root.path().join("original.jpg");
    std::fs::copy(&original, &keeper).unwrap();

    let up = root.path().join("upscaled-2x.jpg");
    let down = root.path().join("downscaled-half.jpg");
    write_scaled(&original, &up, 2.0, 80);
    write_scaled(&original, &down, 0.5, 80);

    let cache = std::sync::Mutex::new(ScanCache::new());
    let outcome = find_similar_with_progress(root.path(), &cache, &|_, _| {}).unwrap();

    // Both the upscaled and the downscaled copy are recognized as the same
    // picture: one group holding all three files.
    assert_eq!(outcome.groups.len(), 1, "{outcome:?}");
    let group = &outcome.groups[0];
    let mut members: Vec<String> = group
        .candidates
        .iter()
        .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    members.push(group.keeper.file_name().unwrap().to_string_lossy().into_owned());
    members.sort();
    assert_eq!(
        members,
        vec!["downscaled-half.jpg", "original.jpg", "upscaled-2x.jpg"]
    );

    // Size on disk decides: the 2x copy is the biggest file, so it is the
    // keeper; the original and the downscale are the removal candidates.
    assert_eq!(group.keeper, up, "biggest file on disk wins");
    assert!(group.keeper_bytes > group.candidates.iter().map(|c| c.bytes).max().unwrap());
    let mut candidate_names: Vec<String> = group
        .candidates
        .iter()
        .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    candidate_names.sort();
    assert_eq!(candidate_names, vec!["downscaled-half.jpg", "original.jpg"]);
    assert!(group.candidates.iter().all(|c| c.removable));
    assert!(outcome.removable.contains(&down));
    assert!(outcome.removable.contains(&keeper));
}

#[test]
fn size_first_keeper_beats_a_more_pixels_upscale() {
    // The 1.15x upscale has more pixels but is smaller on disk and smoother
    // than the source. On-disk size leads: the true original is the keeper,
    // and the interpolated copy is removable (removing it loses nothing).
    let root = tempfile::tempdir().unwrap();
    let original = sample("jpg-exif-mod/image1.JPG");
    let source = root.path().join("original.jpg");
    std::fs::copy(&original, &source).unwrap();
    let up = root.path().join("upscaled-1.15x.jpg");
    write_scaled(&original, &up, 1.15, 75);

    let cache = std::sync::Mutex::new(ScanCache::new());
    let outcome = find_similar_with_progress(root.path(), &cache, &|_, _| {}).unwrap();

    assert_eq!(outcome.groups.len(), 1, "{outcome:?}");
    let group = &outcome.groups[0];
    assert_eq!(group.keeper, source, "the larger file on disk is the keeper");
    assert!(group.keeper_bytes > group.candidates[0].bytes);
    let up_candidate = group
        .candidates
        .iter()
        .find(|c| c.path == up)
        .expect("upscale is a candidate");
    assert!(up_candidate.pixels > group.keeper_pixels, "more pixels…");
    assert!(up_candidate.removable, "…but a smaller file: removable");
    assert!(outcome.removable.contains(&up));
    assert!(!outcome.removable.contains(&source), "the keeper stays");
}

#[test]
fn tiny_downscale_is_a_plain_removal_candidate() {
    // Size on disk is the only criterion: the original is the biggest file
    // (the keeper) and the tiny copy is removable.
    let root = tempfile::tempdir().unwrap();
    let original = sample("jpg-exif-mod/image1.JPG");
    let source = root.path().join("original.jpg");
    std::fs::copy(&original, &source).unwrap();
    let tiny = root.path().join("tiny.jpg");
    write_scaled(&original, &tiny, 0.1, 60);

    let cache = std::sync::Mutex::new(ScanCache::new());
    let outcome = find_similar_with_progress(root.path(), &cache, &|_, _| {}).unwrap();

    assert_eq!(outcome.groups.len(), 1, "{outcome:?}");
    let group = &outcome.groups[0];
    assert_eq!(group.keeper, source, "biggest file is the keeper");
    let tiny_candidate = group
        .candidates
        .iter()
        .find(|c| c.path == tiny)
        .expect("tiny copy listed");
    assert!(tiny_candidate.removable);
    assert_eq!(outcome.removable, vec![tiny]);
}

#[test]
fn same_size_recompression_keeps_the_higher_quality_copy() {
    let root = tempfile::tempdir().unwrap();
    let source = sample("jpg-exif-mod/image1.JPG");

    // Same dimensions, very different JPEG quality.
    let high = root.path().join("high-quality.jpg");
    let low = root.path().join("low-quality.jpg");
    write_scaled(&source, &high, 1.0, 95);
    write_scaled(&source, &low, 1.0, 40);

    let cache = std::sync::Mutex::new(ScanCache::new());
    let outcome = find_similar_with_progress(root.path(), &cache, &|_, _| {}).unwrap();

    assert_eq!(outcome.groups.len(), 1, "{outcome:?}");
    let group = &outcome.groups[0];
    // Equal pixel counts: the tie-break picks the larger (higher quality)
    // file as the keeper and flags the low-quality copy for removal.
    assert_eq!(group.keeper, high, "{outcome:?}");
    assert_eq!(group.candidates.len(), 1);
    assert_eq!(group.candidates[0].path, low);
    assert!(
        group.candidates[0].bytes < group.keeper_bytes,
        "candidate is the smaller file"
    );
}

#[test]
fn cross_tree_compare_flags_only_worse_b_copies() {
    use dedupe2::Scanner::similar::find_similar_between_with_progress;

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("library");
    let b = root.path().join("candidate");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let original = sample("jpg-exif-mod/image1.JPG");
    let keep = a.join("original.jpg");
    std::fs::copy(&original, &keep).unwrap();
    let unrelated_a = a.join("other.jpg");
    std::fs::copy(sample("jpg-dupe-diff-size/IMG_2571.jpg"), &unrelated_a).unwrap();

    // B: a downscaled copy, an upscaled copy, an exact copy, and a
    // genuinely different photo (no match in A).
    let down = b.join("downscaled.jpg");
    let up = b.join("upscaled-2x.jpg");
    let exact = b.join("exact-copy.jpg");
    let unrelated_b = b.join("unrelated.jpg");
    write_scaled(&original, &down, 0.5, 80);
    write_scaled(&original, &up, 2.0, 80);
    std::fs::copy(&original, &exact).unwrap();

    // A synthetic photo unrelated to either A fixture.
    let mut distinct = image::RgbImage::new(320, 240);
    for (x, y, pixel) in distinct.enumerate_pixels_mut() {
        *pixel = image::Rgb([(x * 3 % 256) as u8, (y * 7 % 256) as u8, ((x + y) % 256) as u8]);
    }
    distinct
        .save_with_format(&unrelated_b, image::ImageFormat::Jpeg)
        .unwrap();

    let cache = std::sync::Mutex::new(ScanCache::new());
    let outcome = find_similar_between_with_progress(&a, &b, &cache, &|_, _| {}).unwrap();

    // Three matches against the A original; the unrelated photo matches
    // nothing. Size on disk decides: B's downscale and exact copy lose to A
    // (smaller / tie), while the upscaled B copy is the bigger file and is
    // kept.
    assert_eq!(outcome.matches.len(), 3, "{outcome:?}");
    assert!(outcome.matches.iter().all(|m| m.a == keep));
    assert!(outcome.removable_b.contains(&down));
    assert!(
        !outcome.removable_b.contains(&up),
        "the bigger B file is kept despite the extra pixels"
    );
    assert!(
        outcome.removable_b.contains(&exact),
        "an exact B copy of an A file is removable (A is the record)"
    );
}

#[test]
fn cross_tree_compare_never_flags_a_better_b_copy() {
    use dedupe2::Scanner::similar::find_similar_between_with_progress;

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("library");
    let b = root.path().join("candidate");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let original = sample("jpg-exif-mod/image1.JPG");
    // A holds the downscaled copy; B holds the true original.
    let down = a.join("downscaled.jpg");
    write_scaled(&original, &down, 0.5, 80);
    let keeper = b.join("original.jpg");
    std::fs::copy(&original, &keeper).unwrap();

    let cache = std::sync::Mutex::new(ScanCache::new());
    let outcome = find_similar_between_with_progress(&a, &b, &cache, &|_, _| {}).unwrap();

    assert_eq!(outcome.matches.len(), 1, "{outcome:?}");
    let m = &outcome.matches[0];
    assert!(m.b_is_better, "B has the information, A is the downscale");
    assert!(!m.removable);
    assert!(outcome.removable_b.is_empty());
}
