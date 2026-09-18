use std::path::Path;

use dedupe2::image_reader::{distance, lossy_kind, phash, SIMILAR_MAX_DISTANCE};

fn sample(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages")
        .join(rel)
}

/// Re-encode a fixture at a smaller size / lower quality, the way a
/// "same picture, lower resolution" copy in a library would look.
fn rescaled_jpeg(source: &Path, scale: f32, quality: u8) -> Vec<u8> {
    let img = image::open(source).expect("fixture decodes");
    let (w, h) = (img.width() as f32 * scale, img.height() as f32 * scale);
    let small = img.resize_exact(w as u32, h as u32, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder.encode_image(&small).expect("encodes");
    out
}

#[test]
fn scaled_down_copy_is_within_similar_distance() {
    let source = sample("jpg-exif-mod/image1.JPG");
    let original = phash(&source).expect("phash");
    let scaled = phash(&sample("jpg-exif-mod/image1.JPG")).expect("phash");
    assert_eq!(original, scaled, "phash is deterministic");
    assert!(original.width > 0 && original.height > 0, "dims carried");

    let dir = tempfile::tempdir().unwrap();
    for (name, scale, quality) in [
        ("half.jpg", 0.5, 85),
        ("third.jpg", 0.34, 70),
        ("thumb.jpg", 0.2, 60),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, rescaled_jpeg(&source, scale, quality)).unwrap();
        let small = phash(&path).expect("phash of rescaled");
        assert!(
            small.pixels() < original.pixels(),
            "{name}: fixture must be smaller than the original"
        );
        let distance = distance(original.hash, small.hash);
        assert!(
            distance <= SIMILAR_MAX_DISTANCE,
            "{name}: distance {distance} must be within {SIMILAR_MAX_DISTANCE}"
        );
    }
}

#[test]
fn unrelated_photos_are_far_apart() {
    let a = phash(&sample("jpg-exif-mod/image1.JPG")).unwrap();
    let b = phash(&sample("jpg-dupe-diff-size/IMG_2571.jpg")).unwrap();
    let distance = distance(a.hash, b.hash);
    assert!(
        distance > SIMILAR_MAX_DISTANCE,
        "unrelated photos must not look similar (distance {distance})"
    );
}

#[test]
fn lossy_kind_groups_only_matching_families() {
    assert_eq!(lossy_kind(Path::new("a.jpg")), lossy_kind(Path::new("b.JPEG")));
    assert_eq!(lossy_kind(Path::new("a.heic")), lossy_kind(Path::new("b.HEIF")));
    assert!(lossy_kind(Path::new("a.jpg")) != lossy_kind(Path::new("b.heic")));
    assert!(lossy_kind(Path::new("a.png")).is_none(), "lossless is not phash-compared");
    assert!(lossy_kind(Path::new("a.cr2")).is_none(), "raw is not phash-compared");
}

#[test]
fn upscaled_and_near_one_scale_copies_are_within_similar_distance() {
    let source = sample("jpg-exif-mod/image1.JPG");
    let original = phash(&source).expect("phash");

    let dir = tempfile::tempdir().unwrap();
    for (name, scale, quality) in [
        ("double.jpg", 2.0, 85),
        ("one_and_a_half.jpg", 1.5, 80),
        ("slightly_up.jpg", 1.15, 75),
        ("slightly_down.jpg", 0.9, 75),
        ("tiny.jpg", 0.1, 60),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, rescaled_jpeg(&source, scale, quality)).unwrap();
        let scaled = phash(&path).expect("phash of rescaled");
        let distance = distance(original.hash, scaled.hash);
        assert!(
            distance <= SIMILAR_MAX_DISTANCE,
            "{name}: distance {distance} must be within {SIMILAR_MAX_DISTANCE}"
        );
    }
}
