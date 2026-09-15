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

#[test]
fn png_image_data_is_decoded_and_metadata_free() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/png/IMG_5526.PNG");
    if !path.exists() {
        return; // sample image not present in this checkout
    }

    let data = read_image_data(&path, ReadLimit::First(64 * 1024)).unwrap();
    assert_eq!(data.len(), 64 * 1024);

    // The bytes are the IDAT stream (zlib-compressed picture data), not the
    // raw container bytes (which start with the PNG signature).
    let (hash, decoded) = dedupe2::image_reader::hash_image_data_status(&path, 64 * 1024);
    assert!(decoded);
    assert_eq!(hash, seahash::hash(&data));

    let raw = fs::read(&path).unwrap();
    assert_ne!(raw[..64], data[..64]);
}

#[test]
fn png_hash_ignores_chunks_after_iend() {
    let dir = tempfile::tempdir().unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages/png/IMG_5526.PNG");
    if !path.exists() {
        return; // sample image not present in this checkout
    }

    let original = fs::read(&path).unwrap();
    let path2 = dir.path().join("with_metadata.png");

    // Append a tEXt chunk after IEND: real to the file bytes, invisible to
    // the decoder, and excluded from the image-data hash.
    let mut with_metadata = original.clone();
    let text = b"Comment|some metadata that must not change the hash";
    with_metadata.extend_from_slice(&(text.len() as u32).to_be_bytes());
    with_metadata.extend_from_slice(b"tEXt");
    with_metadata.extend_from_slice(text);
    with_metadata.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // crc placeholder
    fs::write(&path2, &with_metadata).unwrap();

    assert_ne!(original, with_metadata);
    assert_eq!(
        read_image_data(&path, ReadLimit::First(64 * 1024)).unwrap(),
        read_image_data(&path2, ReadLimit::First(64 * 1024)).unwrap()
    );
}

#[test]
fn raster_bmp_hashes_decoded_pixels() {
    use image::{ImageBuffer, Rgb};

    let dir = tempfile::tempdir().unwrap();
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_fn(16, 16, |x, y| Rgb([x as u8 * 7, y as u8 * 11, 42]));
    let path = dir.path().join("test.bmp");
    img.save(&path).unwrap();

    let expected = image::open(&path).unwrap().to_rgba8().into_raw();
    let data = read_image_data(&path, ReadLimit::All).unwrap();
    assert_eq!(data, expected);

    let decoded = read_image(&path).unwrap();
    assert_eq!(decoded.width, 16);
    assert_eq!(decoded.height, 16);
}

/// Minimal M2TS (AVCHD) bytes: 192-byte packets, each a 4-byte arrival
/// timestamp followed by 0x47 and 187 payload bytes.
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
fn m2ts_hash_ignores_arrival_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let content = movie_payload(40_000);

    // Same stream content, different arrival timestamps per packet.
    let a = dir.path().join("a.mts");
    let b = dir.path().join("b.mts");
    fs::write(&a, synthesize_m2ts(&content, 0)).unwrap();
    fs::write(&b, synthesize_m2ts(&content, 999_999)).unwrap();

    let (ha, oka) = dedupe2::image_reader::hash_image_data_status(&a, 64 * 1024);
    let (hb, okb) = dedupe2::image_reader::hash_image_data_status(&b, 64 * 1024);
    assert!(oka && okb, "m2ts must decode ({oka}, {okb})");
    assert_eq!(ha, hb, "arrival timestamps are container bookkeeping");

    let fa = dedupe2::image_reader::hash_image_data_all(&a).unwrap();
    let fb = dedupe2::image_reader::hash_image_data_all(&b).unwrap();
    assert_eq!(fa, fb, "full payload hash must ignore timestamps too");
}

#[test]
fn m2ts_with_different_content_hashes_differ() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.mts");
    let b = dir.path().join("b.mts");
    fs::write(&a, synthesize_m2ts(&movie_payload(10_000), 0)).unwrap();
    fs::write(&b, synthesize_m2ts(&movie_payload(10_000).iter().map(|x| x ^ 0xFF).collect::<Vec<_>>(), 0)).unwrap();

    assert_ne!(
        dedupe2::image_reader::hash_image_data_all(&a).unwrap(),
        dedupe2::image_reader::hash_image_data_all(&b).unwrap()
    );
}

#[test]
fn m2ts_payload_skips_packet_headers() {
    let dir = tempfile::tempdir().unwrap();
    let content = movie_payload(187 * 3);
    let path = dir.path().join("clip.mts");
    fs::write(&path, synthesize_m2ts(&content, 123)).unwrap();

    let data = read_image_data(&path, ReadLimit::First(64 * 1024)).unwrap();
    // Three packets of 187 payload bytes, no 4-byte timestamps, no 0x47.
    assert_eq!(data.len(), 187 * 3);
    assert_eq!(data, content);
}

#[test]
fn mts_is_a_supported_video_extension() {
    assert!(dedupe2::image_reader::is_supported_image(Path::new("clip.mts")));
    assert!(dedupe2::image_reader::is_supported_image(Path::new("clip.MTS")));
    assert!(dedupe2::image_reader::is_supported_image(Path::new("clip.m2ts")));
    assert!(dedupe2::image_reader::is_video(Path::new("clip.mts")));
    assert!(!dedupe2::image_reader::is_video(Path::new("clip.jpg")));
}

#[test]
fn non_mpegts_file_named_mts_is_unreadable_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake.mts");
    fs::write(&path, b"definitely not a transport stream, just text").unwrap();

    let (_, decoded) = dedupe2::image_reader::hash_image_data_status(&path, 64 * 1024);
    assert!(!decoded, "mislabeled file falls back to a raw-bytes hash");
}

#[test]
fn jpeg_content_named_mov_is_rescued_by_content_sniffing() {
    let dir = tempfile::tempdir().unwrap();
    let jpeg = fs::read(jpg_sample()).unwrap();

    let as_jpg = dir.path().join("photo.jpg");
    let as_mov = dir.path().join("photo.mov");
    fs::write(&as_jpg, &jpeg).unwrap();
    fs::write(&as_mov, &jpeg).unwrap();

    let (ha, ok_a) = dedupe2::image_reader::hash_image_data_status(&as_jpg, 64 * 1024);
    let (hb, ok_b) = dedupe2::image_reader::hash_image_data_status(&as_mov, 64 * 1024);
    assert!(ok_a && ok_b, "both must decode ({ok_a}, {ok_b})");
    assert_eq!(ha, hb, "a mislabeled copy must hash like its true format");
}

#[test]
fn png_content_named_jpg_is_rescued_by_content_sniffing() {
    let dir = tempfile::tempdir().unwrap();
    let png = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/png/IMG_5526.PNG"),
    )
    .unwrap();

    let as_png = dir.path().join("image.png");
    let as_jpg = dir.path().join("image.jpg");
    fs::write(&as_png, &png).unwrap();
    fs::write(&as_jpg, &png).unwrap();

    let (ha, ok_a) = dedupe2::image_reader::hash_image_data_status(&as_png, 64 * 1024);
    let (hb, ok_b) = dedupe2::image_reader::hash_image_data_status(&as_jpg, 64 * 1024);
    assert!(ok_a && ok_b, "both must decode ({ok_a}, {ok_b})");
    assert_eq!(ha, hb);
}

#[test]
fn m2ts_content_named_mov_is_rescued_by_content_sniffing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mov");
    fs::write(&path, synthesize_m2ts(&movie_payload(20_000), 0)).unwrap();

    let (_, decoded) = dedupe2::image_reader::hash_image_data_status(&path, 64 * 1024);
    assert!(decoded, "transport stream content is recognized regardless of name");

    let full = dedupe2::image_reader::hash_image_data_all(&path).unwrap();
    let twin = dir.path().join("clip.mts");
    fs::write(&twin, synthesize_m2ts(&movie_payload(20_000), 0)).unwrap();
    assert_eq!(full, dedupe2::image_reader::hash_image_data_all(&twin).unwrap());
}

#[test]
fn garbage_named_mov_stays_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("garbage.mov");
    fs::write(&path, b"not any known container format at all").unwrap();

    let (_, decoded) = dedupe2::image_reader::hash_image_data_status(&path, 64 * 1024);
    assert!(!decoded);
}

/// RIFF chunk: id + little-endian size + payload + pad byte when odd.
fn riff_chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(id);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// Minimal AVI: RIFF header + LIST hdrl (with IDIT metadata) + LIST movi
/// (content) + idx1 index.
fn synthesize_avi(movi: &[u8], idit: &[u8]) -> Vec<u8> {
    let hdrl = riff_chunk(b"LIST", &[b"hdrl".as_slice(), &riff_chunk(b"IDIT", idit)].concat());
    let movi_list = riff_chunk(b"LIST", &[b"movi".as_slice(), movi].concat());
    let idx1 = riff_chunk(b"idx1", &[0u8; 16]);
    let body = [hdrl, movi_list, idx1].concat();

    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(body.len() as u32).to_le_bytes());
    v.extend_from_slice(b"AVI ");
    v.extend_from_slice(&body);
    v
}

#[test]
fn avi_hash_ignores_metadata_outside_movi() {
    let dir = tempfile::tempdir().unwrap();
    let content = movie_payload(30_000);

    let a = dir.path().join("a.avi");
    let b = dir.path().join("b.avi");
    fs::write(&a, synthesize_avi(&content, b"2006:07:15 10:49:34")).unwrap();
    fs::write(&b, synthesize_avi(&content, b"2011:11:11 11:11:11")).unwrap();

    let (ha, oka) = dedupe2::image_reader::hash_image_data_status(&a, 64 * 1024);
    let (hb, okb) = dedupe2::image_reader::hash_image_data_status(&b, 64 * 1024);
    assert!(oka && okb, "avi must decode ({oka}, {okb})");
    assert_eq!(ha, hb, "IDIT metadata must not affect the shallow hash");

    assert_eq!(
        dedupe2::image_reader::hash_image_data_all(&a).unwrap(),
        dedupe2::image_reader::hash_image_data_all(&b).unwrap(),
        "full movi payloads are equal"
    );
}

#[test]
fn avi_with_different_content_hashes_differ() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.avi");
    let b = dir.path().join("b.avi");
    fs::write(&a, synthesize_avi(&movie_payload(10_000), b"same")).unwrap();
    fs::write(
        &b,
        synthesize_avi(
            &movie_payload(10_000).iter().map(|x| x ^ 0xFF).collect::<Vec<_>>(),
            b"same",
        ),
    )
    .unwrap();

    assert_ne!(
        dedupe2::image_reader::hash_image_data_all(&a).unwrap(),
        dedupe2::image_reader::hash_image_data_all(&b).unwrap()
    );
}

#[test]
fn avi_payload_read_returns_movi_content() {
    let dir = tempfile::tempdir().unwrap();
    let content = movie_payload(5_000);
    let path = dir.path().join("clip.avi");
    fs::write(&path, synthesize_avi(&content, b"x")).unwrap();

    let data = read_image_data(&path, ReadLimit::First(64 * 1024)).unwrap();
    assert_eq!(data, content);
}

#[test]
fn avi_extension_is_supported_and_content_sniffed() {
    assert!(dedupe2::image_reader::is_supported_image(Path::new("clip.avi")));
    assert!(dedupe2::image_reader::is_supported_image(Path::new("clip.AVI")));
    assert!(dedupe2::image_reader::is_video(Path::new("clip.avi")));

    // Mislabeled: AVI content under a .mov name must also decode.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mov");
    fs::write(&path, synthesize_avi(&movie_payload(8_000), b"x")).unwrap();
    let (_, decoded) = dedupe2::image_reader::hash_image_data_status(&path, 64 * 1024);
    assert!(decoded);
}
