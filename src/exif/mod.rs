use std::fmt;
use std::path::Path;
use std::time::SystemTime;

use nom_exif::{read_metadata, ExifTag, Metadata, TrackInfoTag};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationDate {
    DateCreated(String),
    Unknown,
}

impl CreationDate {
    pub fn value(&self) -> Option<&str> {
        match self {
            CreationDate::DateCreated(v) => Some(v),
            CreationDate::Unknown => None,
        }
    }
}

impl fmt::Display for CreationDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CreationDate::DateCreated(v) => f.write_str(v),
            CreationDate::Unknown => f.write_str("unknown"),
        }
    }
}

pub fn creation_date(path: &Path) -> CreationDate {
    let from_exif = match read_metadata(path) {
        Ok(metadata) => match metadata {
            Metadata::Exif(exif) => resolve(
                exif.get(ExifTag::DateTimeOriginal).map(|v| v.to_string()),
                exif.get(ExifTag::CreateDate).map(|v| v.to_string()),
            ),
            Metadata::Track(track) => resolve(
                None,
                track.get(TrackInfoTag::CreateDate).map(|v| v.to_string()),
            ),
        },
        Err(_) => CreationDate::Unknown,
    };

    if from_exif != CreationDate::Unknown {
        return from_exif;
    }

    raw_creation_date(path)
}

fn raw_creation_date(path: &Path) -> CreationDate {
    let rawfile = match rawler::rawsource::RawSource::new(path) {
        Ok(rawfile) => rawfile,
        Err(_) => return CreationDate::Unknown,
    };

    let loader = rawler::RawLoader::new();
    let decoder = match loader.get_decoder(&rawfile) {
        Ok(decoder) => decoder,
        Err(_) => return CreationDate::Unknown,
    };

    let metadata = match decoder.raw_metadata(&rawfile, &rawler::decoders::RawDecodeParams::default()) {
        Ok(metadata) => metadata,
        Err(_) => return CreationDate::Unknown,
    };

    match metadata.last_modified() {
        Ok(Some(t)) => CreationDate::DateCreated(format_system_time(t)),
        _ => CreationDate::Unknown,
    }
}

fn format_system_time(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn resolve(dt_original: Option<String>, create_date: Option<String>) -> CreationDate {
    dt_original
        .or(create_date)
        .map(CreationDate::DateCreated)
        .unwrap_or(CreationDate::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
