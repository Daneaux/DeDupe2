use std::path::{Path, PathBuf};

use dedupe2::exif::{creation_date, resolve, CreationDate};

fn test_image(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages").join(rel)
}

#[test]
fn prioritizes_datetime_original() {
    let result = resolve(Some("2023-01-01".into()), Some("2022-01-01".into()));
    assert_eq!(result, CreationDate::DateCreated("2023-01-01".into()));
}

#[test]
fn falls_back_to_create_date() {
    let result = resolve(None, Some("2022-01-01".into()));
    assert_eq!(result, CreationDate::DateCreated("2022-01-01".into()));
}

#[test]
fn unknown_when_both_missing() {
    let result = resolve(None, None);
    assert_eq!(result, CreationDate::Unknown);
}

#[test]
fn reads_ciff_datetime_original_from_crw() {
    let path = test_image("canon-raw/CRW_6542.CRW");
    if !path.exists() {
        return; // sample image not present in this checkout
    }

    let date = creation_date(&path);
    assert_eq!(
        date,
        CreationDate::DateCreated("2004-08-05 20:32:02".into())
    );
}

#[test]
fn reads_mp4_creation_date_from_mvhd() {
    let dir = tempfile::tempdir().unwrap();
    let payload = vec![0xABu8; 2000];

    // mvhd creation_time is seconds since 1904-01-01 UTC.
    let epoch_1904_offset: u32 = 2_082_844_800;
    let creation_time = 1_662_033_600u32 + epoch_1904_offset; // 2022-09-01 12:00:00 UTC

    let mut mvhd = Vec::new();
    mvhd.push(0u8);
    mvhd.extend_from_slice(&[0, 0, 0]);
    mvhd.extend_from_slice(&creation_time.to_be_bytes());
    mvhd.extend_from_slice(&creation_time.to_be_bytes());
    mvhd.extend_from_slice(&600u32.to_be_bytes());
    mvhd.extend_from_slice(&1200u32.to_be_bytes());
    mvhd.extend_from_slice(&0x00010000u32.to_be_bytes());
    mvhd.extend_from_slice(&[0, 1]);
    mvhd.extend_from_slice(&[0u8; 10]);
    mvhd.extend_from_slice(&[0u8; 36]);
    mvhd.extend_from_slice(&[0u8; 24]);
    mvhd.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes());

    let mut file = Vec::new();
    // ftyp body: major_brand(4) + minor_version(4) + compatible_brands(4)
    file.extend_from_slice(&((12u32 + 8).to_be_bytes()));
    file.extend_from_slice(b"ftyp");
    file.extend_from_slice(b"isom");
    file.extend_from_slice(&0u32.to_be_bytes());
    file.extend_from_slice(b"isom");
    // moov contains the mvhd box (size+type header + content)
    let mvhd_box_len = (mvhd.len() as u32) + 8;
    file.extend_from_slice(&((mvhd_box_len + 8).to_be_bytes()));
    file.extend_from_slice(b"moov");
    file.extend_from_slice(&mvhd_box_len.to_be_bytes());
    file.extend_from_slice(b"mvhd");
    file.extend_from_slice(&mvhd);
    file.extend_from_slice(&((payload.len() as u32 + 8).to_be_bytes()));
    file.extend_from_slice(b"mdat");
    file.extend_from_slice(&payload);

    let path = dir.path().join("clip.mp4");
    std::fs::write(&path, &file).unwrap();

    let date = creation_date(&path);
    assert!(
        matches!(date, CreationDate::DateCreated(ref s) if s.starts_with("2022-09-01")),
        "expected 2022-09-01, got {date:?}"
    );
}

#[test]
fn reads_mov_creation_date() {
    let path = test_image("mov/test.MOV");
    if !path.exists() {
        return; // sample not present in this checkout
    }

    let date = creation_date(&path);
    assert!(
        matches!(date, CreationDate::DateCreated(ref s) if s.starts_with("2013-06-30")),
        "expected 2013-06-30, got {date:?}"
    );
}

#[test]
fn reads_rw2_datetime_original() {
    let path = test_image("P1000673.RW2");
    if !path.exists() {
        return; // sample image not present in this checkout
    }

    // Panasonic RW2 carries only DateTimeOriginal/CreateDate (no ModifyDate),
    // so this exercises the capture-date priority over last_modified().
    let date = creation_date(&path);
    assert!(
        matches!(date, CreationDate::DateCreated(ref s) if s.starts_with("2010-05-30")),
        "expected 2010-05-30, got {date:?}"
    );
}

#[test]
fn rawler_capture_date_prefers_datetime_original_over_recent_modify_date() {
    use dedupe2::exif::select_capture_date;

    // The feared case: the file was edited yesterday (recent ModifyDate) but
    // was taken in 2010. ModifyDate must lose.
    let mut exif = rawler::exif::Exif::default();
    exif.date_time_original = Some("2010:05:30 06:59:32".into());
    exif.create_date = Some("2010:05:30 06:59:32".into());
    exif.modify_date = Some("2026:09:10 19:00:00".into());

    let date = select_capture_date(&exif);
    assert!(
        matches!(date, Some(CreationDate::DateCreated(ref s)) if s.starts_with("2010-05-30")),
        "expected 2010-05-30, got {date:?}"
    );

    // Fallback chain: only ModifyDate present -> it is used.
    let mut exif = rawler::exif::Exif::default();
    exif.modify_date = Some("2026:09:10 19:00:00".into());
    let date = select_capture_date(&exif);
    assert!(
        matches!(date, Some(CreationDate::DateCreated(ref s)) if s.starts_with("2026-09-10")),
        "expected 2026-09-10, got {date:?}"
    );

    // Priority between the two capture dates: DateTimeOriginal wins.
    let mut exif = rawler::exif::Exif::default();
    exif.date_time_original = Some("2010:05:30 06:59:32".into());
    exif.create_date = Some("2011:01:01 00:00:00".into());
    let date = select_capture_date(&exif);
    assert!(
        matches!(date, Some(CreationDate::DateCreated(ref s)) if s.starts_with("2010-05-30")),
        "expected 2010-05-30, got {date:?}"
    );
}
