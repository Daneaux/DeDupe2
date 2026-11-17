//! Date-funnel edge cases: tag priority, timezone normalization, date-only
//! completion, invalid calendar values, and folder proxy parsing.

use std::path::Path;

use dedupe2::exif::{
    folder_proxy_date, parse_md_folder, parse_ymd_folder, pick_exiftool_date, resolve, CreationDate,
};
use serde_json::json;

#[test]
fn resolve_prefers_capture_time_then_create_date() {
    assert_eq!(
        resolve(
            Some("2020-01-02 03:04:05".into()),
            Some("2019-12-31 23:59:59".into())
        ),
        CreationDate::DateCreated("2020-01-02 03:04:05".into())
    );
    assert_eq!(
        resolve(None, Some("2019-12-31 23:59:59".into())),
        CreationDate::DateCreated("2019-12-31 23:59:59".into())
    );
    assert_eq!(resolve(None, None), CreationDate::Unknown);
}

#[test]
fn exiftool_dates_prefer_capture_tags_over_file_change_tags() {
    let row = json!({
        "ExifIFD:DateTimeOriginal": "2011:11:05 16:56:37",
        "ExifIFD:CreateDate": "2019:01:01 00:00:00",
        "XMP:DateCreated": "2018:02:02 02:02:02"
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2011-11-05 16:56:37".into()))
    );

    // For equal tag names the camera-adjacent group wins.
    let row = json!({
        "XMP:DateTimeOriginal": "2018:02:02 02:02:02",
        "ExifIFD:DateTimeOriginal": "2011:11:05 16:56:37"
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2011-11-05 16:56:37".into()))
    );
}

#[test]
fn exiftool_dates_normalize_offsets_and_fractions() {
    let cases = [
        ("2011:11:05 16:56:37", "2011-11-05 16:56:37"),
        ("2011-11-05T16:56:37", "2011-11-05 16:56:37"),
        ("2011-11-05T16:56:37.03", "2011-11-05 16:56:37"),
        ("2011-11-05T16:56:37.03+01:00", "2011-11-05 16:56:37"),
        ("2011-11-05T16:56:37-07:00", "2011-11-05 16:56:37"),
        ("2011-11-05 16:56:37+0100", "2011-11-05 16:56:37"),
        ("2011:11:05", "2011-11-05"),
    ];
    for (raw, expected) in cases {
        let row = json!({ "ExifIFD:DateTimeOriginal": raw });
        assert_eq!(
            pick_exiftool_date(&row),
            Some(CreationDate::DateCreated(expected.into())),
            "raw value {raw:?}"
        );
    }
}

#[test]
fn iptc_date_only_tags_are_completed_with_their_time_companion() {
    let row = json!({
        "IPTC:DateCreated": "2011:11:05",
        "IPTC:TimeCreated": "10:11:12"
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2011-11-05 10:11:12".into()))
    );

    // A broken companion clock leaves a date-only value instead of failing.
    let row = json!({
        "IPTC:DateCreated": "2011:11:05",
        "IPTC:TimeCreated": "not a clock"
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2011-11-05".into()))
    );
}

#[test]
fn invalid_or_non_string_exiftool_dates_are_ignored() {
    for raw in [
        "2022:13:01 00:00:00",
        "2022:01:32 00:00:00",
        "0000:00:00 00:00:00",
        "not a date",
        "",
    ] {
        let row = json!({ "ExifIFD:DateTimeOriginal": raw });
        assert_eq!(pick_exiftool_date(&row), None, "raw value {raw:?}");
    }

    let row = json!({
        "ExifIFD:DateTimeOriginal": 12345,
        "ExifIFD:CreateDate": "2020:01:02 03:04:05"
    });
    assert_eq!(
        pick_exiftool_date(&row),
        Some(CreationDate::DateCreated("2020-01-02 03:04:05".into())),
        "non-string values are skipped, later tags still count"
    );

    assert_eq!(pick_exiftool_date(&json!({})), None);
}

#[test]
fn folder_date_parsing_accepts_only_the_documented_shapes() {
    assert_eq!(parse_ymd_folder("2023-03-15"), Some((2023, 3, 15)));
    assert_eq!(parse_ymd_folder("2023-03-15 hawaii"), Some((2023, 3, 15)));
    assert_eq!(parse_ymd_folder("2023-3-15"), None);
    assert_eq!(parse_ymd_folder("March 2023"), None);
    assert_eq!(parse_md_folder("03-15"), Some((3, 15)));
    assert_eq!(parse_md_folder("03-15 hawaii"), Some((3, 15)));
    assert_eq!(parse_md_folder("3-15"), None);
}

#[test]
fn folder_proxy_dates_need_a_year_parent_and_valid_calendar_parts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let nested = root.join("2023").join("03-15");
    std::fs::create_dir_all(&nested).unwrap();
    let photo = nested.join("a.jpg");
    std::fs::write(&photo, b"x").unwrap();
    assert_eq!(
        folder_proxy_date(&photo),
        Some(CreationDate::DateCreated("2023-03-15".into()))
    );

    let full = root.join("2023-03-15");
    std::fs::create_dir_all(&full).unwrap();
    let photo = full.join("b.jpg");
    std::fs::write(&photo, b"x").unwrap();
    assert_eq!(
        folder_proxy_date(&photo),
        Some(CreationDate::DateCreated("2023-03-15".into()))
    );

    // MM-DD without a four-digit year parent is not a date.
    let orphan = root.join("03-15");
    std::fs::create_dir_all(&orphan).unwrap();
    let photo = orphan.join("c.jpg");
    std::fs::write(&photo, b"x").unwrap();
    assert_eq!(folder_proxy_date(&photo), None);

    // Out-of-range month/day are rejected by the proxy.
    let invalid = root.join("2023").join("13-45");
    std::fs::create_dir_all(&invalid).unwrap();
    let photo = invalid.join("d.jpg");
    std::fs::write(&photo, b"x").unwrap();
    assert_eq!(folder_proxy_date(&photo), None);
}

#[test]
fn plain_files_get_no_folder_proxy_date() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("a.jpg");
    std::fs::write(&photo, b"x").unwrap();
    assert_eq!(folder_proxy_date(&photo), None);
    assert!(Path::new(&photo).exists());
}

/// Issue 1: every folder-name shape, through the real fallback on a file
/// that has no date of its own.
mod folder_name_dates {
    use super::*;
    use std::path::PathBuf;

    fn undated_file(root: &Path, rel: &str) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"no metadata here").unwrap();
        path
    }

    fn proxy(root: &Path, rel: &str) -> Option<String> {
        folder_proxy_date(&undated_file(root, rel)).map(|d| d.to_string())
    }

    #[test]
    fn every_shape_in_the_issue_table() {
        let dir = tempfile::tempdir().unwrap();
        for (rel, expected) in [
            ("Hawaii 2013-07-30/a.jpg", "2013-07-30"),
            ("2013_07_30 Hawaii/a.jpg", "2013-07-30"),
            ("2013.07.30 Hawaii/a.jpg", "2013-07-30"),
            ("20130730 Hawaii/a.jpg", "2013-07-30"),
            ("30-07-2013 Hawaii/a.jpg", "2013-07-30"),
            ("2013-07 Hawaii/a.jpg", "2013-07"),
            ("July 2013 trip/a.jpg", "2013-07"),
            ("Jul 2013/a.jpg", "2013-07"),
            ("30 July 2013/a.jpg", "2013-07-30"),
            ("July 30 2013/a.jpg", "2013-07-30"),
            ("2013-07-30 Hawaii/raw/IMG.CR2", "2013-07-30"),
        ] {
            assert_eq!(proxy(dir.path(), rel).as_deref(), Some(expected), "{rel}");
        }
    }

    #[test]
    fn the_nearest_dated_folder_wins() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            proxy(dir.path(), "2013-07-30 Hawaii/2013-08-02 Maui/a.jpg").as_deref(),
            Some("2013-08-02")
        );
        assert_eq!(
            proxy(dir.path(), "2013/07-30 Hawaii/a.jpg").as_deref(),
            Some("2013-07-30"),
            "the YYYY/MM-DD layout still works"
        );
    }

    #[test]
    fn dates_far_above_the_file_do_not_leak_in() {
        let dir = tempfile::tempdir().unwrap();
        // Four folders up: beyond FOLDER_DATE_LEVELS (3).
        assert_eq!(dedupe2::exif::FOLDER_DATE_LEVELS, 3);
        assert_eq!(proxy(dir.path(), "2013-07-30 backup/a/b/c/x.jpg"), None);
        assert_eq!(
            proxy(dir.path(), "2013-07-30 backup/a/b/x.jpg").as_deref(),
            Some("2013-07-30")
        );
    }

    #[test]
    fn ambiguous_and_non_date_names_stay_unknown() {
        let dir = tempfile::tempdir().unwrap();
        for rel in [
            "07-08-2013 party/a.jpg",
            "Trip 2013 - 1200 photos/a.jpg",
            "Room 1012/a.jpg",
            "2013-02-30 trip/a.jpg",
        ] {
            assert_eq!(proxy(dir.path(), rel), None, "{rel}");
        }
    }

    #[test]
    fn the_file_s_own_date_beats_any_folder_date() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("1999-01-01 wrong folder/image1.JPG");
        std::fs::create_dir_all(photo.parent().unwrap()).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/jpg-exif-mod/image1.JPG"),
            &photo,
        )
        .unwrap();

        let date = dedupe2::exif::creation_date(&photo).to_string();
        assert!(date.starts_with("2022-08-17"), "{date}");
    }

    #[test]
    fn verify_compares_month_only_folders_by_month() {
        use dedupe2::scanner::compare::verify_tree_dates;

        let dir = tempfile::tempdir().unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/jpg-exif-mod/image1.JPG");
        // image1.JPG was taken 2022-08-17.
        for rel in ["August 2022/right.jpg", "July 2022/wrong.jpg"] {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::copy(&fixture, &path).unwrap();
        }

        let outcome = verify_tree_dates(dir.path()).unwrap();

        assert_eq!(outcome.ok, 1, "{outcome:?}");
        assert_eq!(outcome.mismatches.len(), 1);
        let mismatch = &outcome.mismatches[0];
        assert!(mismatch.path.ends_with("July 2022/wrong.jpg"));
        assert_eq!(mismatch.folder_date, "2022-07");
        assert_eq!(mismatch.days_off, 47, "measured from 1 July");
    }
}
