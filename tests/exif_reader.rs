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
