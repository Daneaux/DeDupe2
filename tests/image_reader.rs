use std::path::{Path, PathBuf};

use dedupe2::image_reader::{read_bytes, read_image, read_image_data, PixelData, ReadLimit};

fn heic_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/HEIC-exif-mod/heic1.HEIC")
}

fn heic_sample_copy() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/HEIC-exif-mod/heic1-copy.HEIC")
}
fn jpg_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/jpg-exif-mod/image1.JPG")
}
fn jpg_sample_mod_exif() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/jpg-exif-mod/image1-exif.JPG")
}
fn raf_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_raf.RAF")
}
fn raf_sample_mod_exif() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_raf-exif.RAF")
}
fn cr2_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_cr2.CR2")
}
fn cr2_sample_mod_exif() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_cr2-exif.CR2")
}


#[test]
fn heic_reads_first_64kb_of_image_data_skipping_headers() {
    let data = read_image_data(&heic_sample(), ReadLimit::First(64 * 1024)).unwrap();

    assert_eq!(data.len(), 64 * 1024);
}

#[test]
fn heic_reads_all_image_data() {
    let data = read_image_data(&heic_sample(), ReadLimit::All).unwrap();

    assert!(!data.is_empty());
    assert!(data.len() > 64 * 1024);
}

#[test]
fn heic_image_data_skips_headers() {
    let path = heic_sample();

    let file_header = read_bytes(&path, ReadLimit::First(64 * 1024)).unwrap();
    let image_data = read_image_data(&path, ReadLimit::First(64 * 1024)).unwrap();

    assert_ne!(file_header, image_data);
}

#[test]
fn heic_compare_image_data_and_decoded_pixels() {
    let path = heic_sample();
    let n = 64 * 1024;

    let raw = read_image_data(&path, ReadLimit::First(n)).unwrap();
    assert_eq!(raw.len(), n);

    let image = read_image(&path).unwrap();
    let decoded: Vec<u8> = match image.data {
        PixelData::U8(bytes) => bytes,
        PixelData::U16(samples) => samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
    };
    let decoded_first = &decoded[..decoded.len().min(n)];

    assert!(!decoded_first.is_empty());
    assert_ne!(raw.as_slice(), decoded_first);
}

#[test]
fn heic_exif_data_different_but_image_data_same() {
    let path1 = heic_sample();
//    let path2 = heic_sample_mod_exif();
    let path2 = heic_sample_copy();

    let limit = ReadLimit::First(64 * 1024);

    let imagedata1 = read_image_data(&path1, limit).unwrap();
    let imagedata2 = read_image_data(&path2, limit).unwrap();

    let filedata1 = read_bytes(&path1, limit).unwrap();
    let filedata2 = read_bytes(&path2, limit).unwrap();

    assert_eq!(imagedata1, imagedata2);
    assert_ne!(filedata1, filedata2);
}

#[test]
fn jpg_exif_data_different_but_image_data_same() {
    let path1 = jpg_sample();
    let path2 = jpg_sample_mod_exif();

    let limit = ReadLimit::First(64 * 1024);

    let imagedata1 = read_image_data(&path1, limit).unwrap();
    let imagedata2 = read_image_data(&path2, limit).unwrap();

    let filedata1 = read_bytes(&path1, limit).unwrap();
    let filedata2 = read_bytes(&path2, limit).unwrap();

    assert_eq!(imagedata1, imagedata2);
    assert_ne!(filedata1, filedata2);
}

#[test]
fn raf_exif_data_different_but_image_data_same() {
    let limit = ReadLimit::First(64 * 1024);

    let imagedata1 = read_image_data(&raf_sample(), limit).unwrap();
    let imagedata2 = read_image_data(&raf_sample_mod_exif(), limit).unwrap();

    let filedata1 = read_bytes(&raf_sample(), limit).unwrap();
    let filedata2 = read_bytes(&raf_sample_mod_exif(), limit).unwrap();

    assert_eq!(imagedata1, imagedata2);
    assert_ne!(filedata1, filedata2);
}

#[test]
fn cr2_exif_data_different_but_image_data_same() {
    let limit = ReadLimit::First(64 * 1024);

    let imagedata1 = read_image_data(&cr2_sample(), limit).unwrap();
    let imagedata2 = read_image_data(&cr2_sample_mod_exif(), limit).unwrap();

    let filedata1 = read_bytes(&cr2_sample(), limit).unwrap();
    let filedata2 = read_bytes(&cr2_sample_mod_exif(), limit).unwrap();

    assert_eq!(imagedata1, imagedata2);
    assert_ne!(filedata1, filedata2);
}

#[test]
fn raf_decoded_image_data_same() {
    let a = read_image(&raf_sample()).unwrap();
    let b = read_image(&raf_sample_mod_exif()).unwrap();

    match (&a.data, &b.data) {
        (PixelData::U16(x), PixelData::U16(y)) => assert_eq!(x, y),
        _ => panic!("expected raw U16 samples"),
    }
}
