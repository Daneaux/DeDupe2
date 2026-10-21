//! Malformed-input hardening: every decoder path must fail cleanly (no panic,
//! no unbounded work) on truncated or corrupt containers.

use std::path::{Path, PathBuf};

use dedupe2::image_reader::{hash_image_data_n, read_image, read_image_data, thumbnail_jpeg, ReadLimit};

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn png_without_idat(width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&[8, 2, 0, 0, 0]);
    out.extend_from_slice(&0u32.to_be_bytes());
    out
}

#[test]
fn corrupt_containers_never_panic_and_never_produce_previews() {
    let dir = tempfile::tempdir().unwrap();
    let mut riff = b"RIFF".to_vec();
    riff.extend_from_slice(&u32::MAX.to_le_bytes());
    riff.extend_from_slice(b"AVI LIST");
    riff.extend_from_slice(&[0u8; 8]);

    let mut mts = Vec::new();
    for _ in 0..3 {
        mts.push(0x47);
        mts.extend(std::iter::repeat(0xAB).take(187));
    }

    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty.jpg", Vec::new()),
        ("truncated.jpg", vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]),
        ("zero-dims.png", png_without_idat(0, 0)),
        ("truncated.png", vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
        ("truncated.heic", b"\0\0\0\x18ftypheic\0\0\0\0heic".to_vec()),
        ("truncated.mov", b"\0\0\0\x08ftyp".to_vec()),
        ("garbage.crw", b"II\x1A\0\0\0HEAPCCDR".to_vec()),
        ("corrupt.avi", riff),
        ("corrupt.mts", mts),
        ("garbage.jpg", vec![0x13, 0x37, 0x00, 0xFF, 0x7F, 0x42]),
    ];

    for (name, bytes) in cases {
        let path = write(dir.path(), name, &bytes);
        let probe = path.clone();
        let result = std::panic::catch_unwind(move || {
            let _ = hash_image_data_n(&probe, 64 * 1024);
            let _ = read_image(&probe);
            let _ = read_image_data(&probe, ReadLimit::All);
            let _ = read_image_data(&probe, ReadLimit::First(64));
        });
        assert!(result.is_ok(), "{name} panicked while hashing/reading");
        assert!(
            thumbnail_jpeg(&path).is_none(),
            "{name} produced a preview from corrupt data"
        );
    }
}

#[test]
fn missing_files_are_handled_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.jpg");

    assert!(read_image_data(&missing, ReadLimit::All).is_err());
    assert!(thumbnail_jpeg(&missing).is_none());
    // Raw-bytes fallback is deterministic for unreadable paths.
    assert_eq!(hash_image_data_n(&missing, 64 * 1024), 0);
}

#[test]
fn thumbnail_is_a_small_jpeg_of_a_real_photo() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    let bytes = thumbnail_jpeg(&source).expect("fixture preview");
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "preview is a JPEG");

    let preview = image::load_from_memory(&bytes).expect("preview decodes");
    let original = image::open(&source).expect("fixture decodes");
    assert!(preview.width() <= 256 && preview.height() <= 256);
    assert!(
        preview.width() < original.width() || preview.height() < original.height(),
        "preview must be smaller than the source"
    );
}

#[test]
fn truncated_real_photo_is_unreadable_not_fatal() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/jpg-exif-mod/image1.JPG");
    let bytes = std::fs::read(&source).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "cut.jpg", &bytes[..16]);

    assert!(read_image_data(&path, ReadLimit::All).is_err());
    assert!(thumbnail_jpeg(&path).is_none());
    let _ = hash_image_data_n(&path, 64 * 1024);
}
