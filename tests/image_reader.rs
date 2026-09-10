use std::fs;
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
fn cr3_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_cr3.CR3")
}
fn cr3_sample_mod_exif() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_cr3-exif.CR3")
}
fn crw_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_crw.CRW")
}
fn mov_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/mov/test.MOV")
}
fn mov_sample_mod_exif() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/mov/test_exif.MOV")
}


fn crw_sample_mod_exif() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_crw-exif.CRW")
}
fn orf_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_orf.ORF")
}
fn orf_sample_mod_exif() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_orf-exif.ORF")
}

fn tiff_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_tiff.tif")
}
fn tiff_sample_mod_exif() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_tiff-exif.tif")
}

fn rw2_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_rw2.RW2")
}
fn rw2_sample_mod_exif() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/raw-exif-mod/raw_rw2-exif.RW2")
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

#[test]
fn cr3_exif_data_different_but_image_data_same() {
    let limit = ReadLimit::First(64 * 1024);

    let imagedata1 = read_image_data(&cr3_sample(), limit).unwrap();
    let imagedata2 = read_image_data(&cr3_sample_mod_exif(), limit).unwrap();

    let filedata1 = read_bytes(&cr3_sample(), limit).unwrap();
    let filedata2 = read_bytes(&cr3_sample_mod_exif(), limit).unwrap();

    assert_eq!(imagedata1, imagedata2);
    assert_ne!(filedata1, filedata2);
}

#[test]
fn crw_exif_data_different_but_image_data_same() {
    let limit = ReadLimit::First(64 * 1024);

    let imagedata1 = read_image_data(&crw_sample(), limit).unwrap();
    let imagedata2 = read_image_data(&crw_sample_mod_exif(), limit).unwrap();

    let filedata1 = read_bytes(&crw_sample(), ReadLimit::All).unwrap();
    let filedata2 = read_bytes(&crw_sample_mod_exif(), ReadLimit::All).unwrap();

    assert_eq!(imagedata1, imagedata2);
    assert_ne!(filedata1, filedata2);
}

#[test]
fn orf_exif_data_different_but_image_data_same() {
    let limit = ReadLimit::First(64 * 1024);

    let imagedata1 = read_image_data(&orf_sample(), limit).unwrap();
    let imagedata2 = read_image_data(&orf_sample_mod_exif(), limit).unwrap();

    let filedata1 = read_bytes(&orf_sample(), ReadLimit::All).unwrap();
    let filedata2 = read_bytes(&orf_sample_mod_exif(), ReadLimit::All).unwrap();

    assert_eq!(imagedata1, imagedata2);
    assert_ne!(filedata1, filedata2);
}

#[test]
fn tiff_exif_data_different_but_image_data_same() {
    let limit = ReadLimit::First(64 * 1024);

    let imagedata1 = read_image_data(&tiff_sample(), limit).unwrap();
    let imagedata2 = read_image_data(&tiff_sample_mod_exif(), limit).unwrap();

    let filedata1 = read_bytes(&tiff_sample(), ReadLimit::All).unwrap();
    let filedata2 = read_bytes(&tiff_sample_mod_exif(), ReadLimit::All).unwrap();

    assert_eq!(imagedata1, imagedata2);
    assert_ne!(filedata1, filedata2);
}

#[test]
fn rw2_exif_data_different_but_image_data_same() {
    let limit = ReadLimit::First(64 * 1024);

    let imagedata1 = read_image_data(&rw2_sample(), limit).unwrap();
    let imagedata2 = read_image_data(&rw2_sample_mod_exif(), limit).unwrap();

    let filedata1 = read_bytes(&rw2_sample(), ReadLimit::All).unwrap();
    let filedata2 = read_bytes(&rw2_sample_mod_exif(), ReadLimit::All).unwrap();

    assert_eq!(imagedata1, imagedata2);
    assert_ne!(filedata1, filedata2);
}


#[test]
fn hash_image_data_n_falls_back_to_raw_bytes_for_undecodable_files() {
    use std::fs;
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("garbage.cr2");
    let mut f = fs::File::create(&path).unwrap();
    f.write_all(&[0xFF, 0x44, 0x00, 0x17, 0xAB, 0xCD, 0xEF, 0x01]).unwrap();
    drop(f);

    let hash = dedupe2::image_reader::hash_image_data_n(&path, 64 * 1024);
    let bytes = fs::read(&path).unwrap();
    assert_eq!(hash, seahash::hash(&bytes));
}

// --- mp4 / mov support ------------------------------------------------------

fn bmff_box(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(typ);
    v.extend_from_slice(body);
    v
}

/// Minimal mp4/mov bytes: ftyp + moov(mvhd) + mdat(payload).
fn synthesize_movie(payload: &[u8], creation_time: u32) -> Vec<u8> {
    let mut mvhd = Vec::new();
    mvhd.push(0u8); // version
    mvhd.extend_from_slice(&[0, 0, 0]); // flags
    mvhd.extend_from_slice(&creation_time.to_be_bytes()); // creation time
    mvhd.extend_from_slice(&creation_time.to_be_bytes()); // modification time
    mvhd.extend_from_slice(&600u32.to_be_bytes()); // timescale
    mvhd.extend_from_slice(&1200u32.to_be_bytes()); // duration
    mvhd.extend_from_slice(&0x00010000u32.to_be_bytes()); // rate
    mvhd.extend_from_slice(&[0, 1]); // volume
    mvhd.extend_from_slice(&[0u8; 10]); // reserved
    mvhd.extend_from_slice(&[0u8; 36]); // matrix
    mvhd.extend_from_slice(&[0u8; 24]); // pre-defined
    mvhd.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes()); // next track id

    let mut file = bmff_box(b"ftyp", b"isom");
    file.extend(bmff_box(b"moov", &mvhd));
    file.extend(bmff_box(b"mdat", payload));
    file
}

fn movie_payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i as u64 * 7 + 13) % 251) as u8).collect()
}

#[test]
fn mp4_hash_uses_movie_data_skipping_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let payload = movie_payload(50_000);
    let path = dir.path().join("clip.mp4");
    fs::write(&path, synthesize_movie(&payload, 3_744_878_400)).unwrap();

    let data = read_image_data(&path, ReadLimit::First(64 * 1024)).unwrap();

    // The returned bytes are exactly the mdat head — no ftyp/moov metadata.
    assert_eq!(data, payload);
}

#[test]
fn mp4_metadata_changes_do_not_change_hash() {
    let dir = tempfile::tempdir().unwrap();
    let payload = movie_payload(10_000);

    let a = dir.path().join("a.mp4");
    let b = dir.path().join("b.mp4");
    fs::write(&a, synthesize_movie(&payload, 3_000_000_000)).unwrap();
    fs::write(&b, synthesize_movie(&payload, 3_744_878_400)).unwrap();

    let ha = dedupe2::image_reader::hash_image_data_n(&a, 64 * 1024);
    let hb = dedupe2::image_reader::hash_image_data_n(&b, 64 * 1024);
    assert_eq!(ha, hb);

    // ...and it is the mdat content, not the whole file.
    let expected = seahash::hash(&payload);
    assert_eq!(ha, expected);
}

#[test]
fn mp4_64kb_limit_on_large_movie() {
    let dir = tempfile::tempdir().unwrap();
    let payload = movie_payload(200_000);
    let path = dir.path().join("big.mp4");
    fs::write(&path, synthesize_movie(&payload, 3_744_878_400)).unwrap();

    let data = read_image_data(&path, ReadLimit::First(64 * 1024)).unwrap();
    assert_eq!(data.len(), 64 * 1024);
    assert_eq!(data, payload[..64 * 1024]);
}

#[test]
fn mov_with_mdat_first_is_supported() {
    // QuickTime files often start with mdat and carry moov at the end.
    let dir = tempfile::tempdir().unwrap();
    let payload = movie_payload(20_000);
    let mut file = bmff_box(b"mdat", &payload);
    file.extend(bmff_box(b"moov", &[0u8; 4])); // placeholder moov body

    let path = dir.path().join("clip.mov");
    fs::write(&path, &file).unwrap();

    let data = read_image_data(&path, ReadLimit::First(64 * 1024)).unwrap();
    assert_eq!(data, payload);
}

#[test]
fn mov_exif_data_different_but_movie_data_same() {
    let limit = ReadLimit::First(64 * 1024);

    let movie1 = read_image_data(&mov_sample(), limit).unwrap();
    let movie2 = read_image_data(&mov_sample_mod_exif(), limit).unwrap();

    let filedata1 = read_bytes(&mov_sample(), ReadLimit::All).unwrap();
    let filedata2 = read_bytes(&mov_sample_mod_exif(), ReadLimit::All).unwrap();

    assert_eq!(movie1, movie2);
    assert_ne!(filedata1, filedata2);
}

#[test]
fn mov_hashes_of_exif_modified_variants_match() {
    let ha = dedupe2::image_reader::hash_image_data_n(&mov_sample(), 64 * 1024);
    let hb = dedupe2::image_reader::hash_image_data_n(&mov_sample_mod_exif(), 64 * 1024);
    assert_eq!(ha, hb);
}
