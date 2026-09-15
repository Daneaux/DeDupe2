use std::path::{Path, PathBuf};

use dedupe2::exif::{creation_date, creation_dates_batch, pick_exiftool_date, resolve, CreationDate};

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

fn xmp_packet(body: &str) -> Vec<u8> {
    format!(
        "<?xpacket begin=\"\"?><x:xmpmeta><rdf:RDF><rdf:Description rdf:about=\"\">{body}</rdf:Description></rdf:RDF></x:xmpmeta>"
    )
    .into_bytes()
}

/// Minimal little-endian TIFF whose IFD0 carries one 0x02bc (XMP) entry.
fn xmp_tiff(xmp: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"II\x2A\x00");
    v.extend_from_slice(&8u32.to_le_bytes()); // IFD0 at offset 8
    v.extend_from_slice(&1u16.to_le_bytes()); // one entry
    v.extend_from_slice(&0x02bcu16.to_le_bytes()); // tag: XMP
    v.extend_from_slice(&1u16.to_le_bytes()); // type: BYTE
    v.extend_from_slice(&(xmp.len() as u32).to_le_bytes());
    let payload_off = 8u32 + 2 + 12 + 4;
    v.extend_from_slice(&payload_off.to_le_bytes()); // value offset
    v.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
    assert_eq!(v.len() as u32, payload_off);
    v.extend_from_slice(xmp);
    v
}

#[test]
fn reads_date_from_embedded_xmp_in_processed_tiff() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("blend.tif");
    std::fs::write(
        &path,
        xmp_tiff(&xmp_packet(r#"<rdf:Description xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" photoshop:DateCreated="2011-06-15T10:20:30.03" />"#)),
    )
    .unwrap();

    assert_eq!(
        creation_date(&path),
        CreationDate::DateCreated("2011-06-15 10:20:30".into())
    );
}

#[test]
fn xmp_prefers_exif_datetimeoriginal_over_photoshop_datecreated() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("blend2.tif");
    std::fs::write(
        &path,
        xmp_tiff(&xmp_packet(
            r#"photoshop:DateCreated="2011-06-15T10:20:30.03" exif:DateTimeOriginal="2010-05-04T03:02:01""#,
        )),
    )
    .unwrap();

    assert_eq!(
        creation_date(&path),
        CreationDate::DateCreated("2010-05-04 03:02:01".into())
    );
}

#[test]
fn falls_back_to_xmp_in_jpeg_when_classic_exif_is_absent() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("xmpnoexif.jpg");
    let packet = xmp_packet(r#"xmp:CreateDate="2013-01-02T08:09:10""#);
    let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    let len = (packet.len() as u16 + 2);
    jpg.extend_from_slice(&len.to_be_bytes());
    jpg.extend_from_slice(b"http://ns.adobe.com/xap/1.0/\0");
    jpg.extend_from_slice(&packet);
    std::fs::write(&path, &jpg).unwrap();

    assert_eq!(
        creation_date(&path),
        CreationDate::DateCreated("2013-01-02 08:09:10".into())
    );
}

#[test]
fn exiftool_rows_parse_to_dates() {
    let row = serde_json::json!({
        "SourceFile": "/x/y.tif",
        "ExifIFD:DateTimeOriginal": "2011:11:05 16:56:37",
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2011-11-05 16:56:37".into()))
    );
}

#[test]
fn exiftool_joins_iptc_datecreated_with_timecreated() {
    let row = serde_json::json!({
        "IPTC:DateCreated": "2011:11:05",
        "IPTC:TimeCreated": "16:56:37+00:00",
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2011-11-05".into()))
    );
}

#[test]
fn exiftool_reads_iso_create_date_with_timezone() {
    let row = serde_json::json!({
        "XMP:CreateDate": "2011-11-05T20:55:20+01:00",
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2011-11-05 20:55:20".into()))
    );
}

#[test]
fn exiftool_prefers_exif_datetimeoriginal_over_iptc() {
    let row = serde_json::json!({
        "IPTC:DigitalCreationDate": "2010:01:02",
        "ExifIFD:DateTimeOriginal": "2012:03:04 05:06:07",
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2012-03-04 05:06:07".into()))
    );
}

/// End-to-end through the real exiftool binary on a TIFF whose only date
/// metadata is an IPTC-NAA (0x83bb) record. Requires exiftool on PATH.
#[test]
#[ignore]
fn reads_iptc_date_through_exiftool_binary() {
    use std::process::Command;

    // IPTC IIM records: DateCreated (2:55), DigitalCreationDate (2:62)
    let mut iptc = Vec::new();
    for (dataset, value) in [(0x37u8, b"20111105"), (0x3E, b"20111105")] {
        iptc.push(0x1C);
        iptc.push(2);
        iptc.push(dataset);
        iptc.extend_from_slice(&(value.len() as u16).to_be_bytes());
        iptc.extend_from_slice(value);
    }

    let mut v = Vec::new();
    v.extend_from_slice(b"II\x2A\x00");
    v.extend_from_slice(&8u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&0x83bbu16.to_le_bytes()); // tag: IPTC-NAA
    v.extend_from_slice(&1u16.to_le_bytes()); // type: BYTE
    v.extend_from_slice(&(iptc.len() as u32).to_le_bytes());
    let payload_off = 8u32 + 2 + 12 + 4;
    v.extend_from_slice(&payload_off.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&iptc);

    let ok = Command::new("exiftool")
        .arg("-ver")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return; // exiftool not installed — skip silently
    }

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("iptc_only.tif");
    std::fs::write(&path, &v).unwrap();

    // The exiftool path is only reachable through the batched resolver.
    let dates = creation_dates_batch(&[path.clone()], &|_, _| {});
    assert_eq!(dates[0], CreationDate::DateCreated("2011-11-05".into()));
}

#[test]
fn batch_resolves_xmp_files_and_folder_proxies_the_rest() {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2013");
    let event = year.join("07-07-field day");
    std::fs::create_dir_all(&event).unwrap();

    let xmp_path = root.path().join("blend.tif");
    std::fs::write(
        &xmp_path,
        xmp_tiff(&xmp_packet(r#"photoshop:DateCreated="2012-01-02T03:04:05.06""#)),
    )
    .unwrap();

    let undated_path = event.join("shot.bin");
    std::fs::write(&undated_path, b"no dates here").unwrap();

    let dates = creation_dates_batch(&[xmp_path.clone(), undated_path.clone()], &|_, _| {});
    assert_eq!(dates[0], CreationDate::DateCreated("2012-01-02 03:04:05".into()));
    assert_eq!(dates[1], CreationDate::DateCreated("2013-07-07".into()));
}

/// Build a TIFF whose only date metadata is an IPTC-NAA (0x83bb) record.
fn iptc_only_tiff(date_bytes: &[u8; 8]) -> Vec<u8> {
    let mut iptc = Vec::new();
    for dataset in [0x37u8, 0x3E] {
        // IPTC IIM: DateCreated (2:55) and DigitalCreationDate (2:62)
        iptc.push(0x1C);
        iptc.push(2);
        iptc.push(dataset);
        iptc.extend_from_slice(&8u16.to_be_bytes());
        iptc.extend_from_slice(date_bytes);
    }

    let mut v = Vec::new();
    v.extend_from_slice(b"II\x2A\x00");
    v.extend_from_slice(&8u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&0x83bbu16.to_le_bytes()); // tag: IPTC-NAA
    v.extend_from_slice(&1u16.to_le_bytes()); // type: BYTE
    v.extend_from_slice(&(iptc.len() as u32).to_le_bytes());
    let payload_off = 8u32 + 2 + 12 + 4;
    v.extend_from_slice(&payload_off.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&iptc);
    v
}

/// Environment-specific: runs the real exiftool binary through the `-@`
/// argsfile batch with several files at once — including paths with spaces
/// and a file that resolves only via the folder-name proxy — and asserts the
/// per-file mapping comes back in order. Requires exiftool on PATH.
#[test]
#[ignore]
fn exiftool_batch_maps_multiple_files_in_one_run() {
    use std::process::Command;

    if !Command::new("exiftool")
        .arg("-ver")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return; // exiftool not installed — skip silently
    }

    let root = tempfile::tempdir().unwrap();
    let with_space = root.path().join("event with spaces");
    std::fs::create_dir_all(&with_space).unwrap();

    let p1 = with_space.join("first shot.tif");
    let p2 = with_space.join("second shot.tif");
    std::fs::write(&p1, iptc_only_tiff(b"20110101")).unwrap();
    std::fs::write(&p2, iptc_only_tiff(b"20141231")).unwrap();

    // No embedded date at all; its folder name must provide the date.
    let proxy_dir = root.path().join("2013/07-07-trip");
    std::fs::create_dir_all(&proxy_dir).unwrap();
    let p3 = proxy_dir.join("nocamera.bin");
    std::fs::write(&p3, b"bytes only").unwrap();

    let paths = vec![p1.clone(), p2.clone(), p3.clone()];
    let dates = creation_dates_batch(&paths, &|_, _| {});

    assert_eq!(dates[0], CreationDate::DateCreated("2011-01-01".into()));
    assert_eq!(dates[1], CreationDate::DateCreated("2014-12-31".into()));
    assert_eq!(dates[2], CreationDate::DateCreated("2013-07-07".into()));
}

#[test]
fn exiftool_reads_quicktime_creatdate_from_mts() {
    let row = serde_json::json!({
        "SourceFile": "/x/clip.mts",
        "M2TS:CreateDate": "2014:06:01 12:34:56",
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2014-06-01 12:34:56".into()))
    );

    let row = serde_json::json!({
        "SourceFile": "/x/clip.mp4",
        "QuickTime:CreateDate": "2015-02-03T04:05:06",
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2015-02-03 04:05:06".into()))
    );
}

#[test]
fn exiftool_prefers_exiftag_group_for_equal_tag_names() {
    let row = serde_json::json!({
        "QuickTime:CreateDate": "2015-02-03T04:05:06",
        "XMP:CreateDate": "2016-03-04T05:06:07",
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2016-03-04 05:06:07".into())),
        "XMP ranks above QuickTime for the same tag name"
    );
}

/// Environment-specific: a real AVCHD `.MTS` clip whose only date tag is the
/// H.264 stream's DateTimeOriginal (`2010:09:28 12:54:38+01:00`) — readable
/// only through the batched exiftool fallback; also proves it participates in
/// scanning (decodes instead of landing in the unreadable list). Ignored by
/// default.
#[test]
#[ignore]
fn reads_avchd_mts_date_via_exiftool() {
    let path = Path::new(
        "/Users/dannydalal/4tbext/SrcImageFolders/Single Events/Original 2000-2019/2010/2010-09  Paris Turkey Lebanon Boston/2010-09-28/00000.MTS",
    );
    if !path.exists() {
        return;
    }

    let (_, decoded) = dedupe2::image_reader::hash_image_data_status(path, 64 * 1024);
    assert!(decoded, "mts must decode for scan/compare");

    let dates = creation_dates_batch(&[path.to_path_buf()], &|_, _| {});
    assert_eq!(
        dates[0],
        CreationDate::DateCreated("2010-09-28 12:54:38".into())
    );
}

#[test]
fn creation_dates_batch_reports_progress_for_every_file() {
    use std::sync::Mutex;

    let root = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for i in 0..6 {
        let p = root.path().join(format!("file{i}.jpg"));
        std::fs::write(&p, b"not really an image").unwrap();
        paths.push(p);
    }

    let ticks: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    let totals: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    let dates = creation_dates_batch(&paths, &|done, total| {
        ticks.lock().unwrap().push(done);
        totals.lock().unwrap().push(total);
    });
    assert_eq!(dates.len(), paths.len());

    let mut seen = ticks.into_inner().unwrap();
    seen.sort();
    assert_eq!(
        seen,
        (1..=paths.len()).collect::<Vec<_>>(),
        "every file must tick exactly once"
    );
    assert!(totals.into_inner().unwrap().iter().all(|&t| t == paths.len()));
}

#[test]
fn exiftool_reads_riff_datetimeoriginal_from_avi() {
    let row = serde_json::json!({
        "SourceFile": "/x/clip.avi",
        "RIFF:DateTimeOriginal": "2006:07:15 10:49:34",
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2006-07-15 10:49:34".into()))
    );
}

/// Environment-specific: a real Canon AVI whose only date is the RIFF
/// DateTimeOriginal. Ignored by default.
#[test]
#[ignore]
fn reads_real_avi_date_via_exiftool() {
    let path = Path::new(
        "/Users/dannydalal/4tbext/SrcImageFolders/Single Events/Original 2000-2019/photography/pics to organize/beeenie/149CANON/MVI_4987.AVI",
    );
    if !path.exists() {
        return;
    }
    let (_, decoded) = dedupe2::image_reader::hash_image_data_status(path, 64 * 1024);
    assert!(decoded, "avi must decode for scan/compare");

    let dates = creation_dates_batch(&[path.to_path_buf()], &|_, _| {});
    assert_eq!(
        dates[0],
        CreationDate::DateCreated("2006-07-15 10:49:34".into())
    );
}
