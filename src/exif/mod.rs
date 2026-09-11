mod ciff;

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
    // Decoders can panic on malformed files; unwind per file and treat it as
    // unknown rather than poisoning the whole scan.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| creation_date_inner(path)))
        .unwrap_or(CreationDate::Unknown)
}

fn creation_date_inner(path: &Path) -> CreationDate {
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

    // Canon CRW (CIFF): nom-exif can't read it and rawler's CRW decoder does
    // not parse metadata yet, so the shot date comes from the CIFF tree.
    if let Some(date) = ciff::creation_date(path) {
        return date;
    }

    // MP4/MOV are nom-exif's domain; rawler's bmff decoder is for CR3 and
    // gains nothing here, so don't fall back into it for movie files.
    let movie = is_movie_ext(path);
    if !movie {
        let from_rawler = raw_creation_date(path);
        if from_rawler != CreationDate::Unknown {
            return from_rawler;
        }
    }

    // No EXIF anywhere: a date-bearing folder (`2013/07-07` or `2013-07-07`)
    // is a reasonable proxy for when the photo was taken.
    if let Some(date) = folder_proxy_date(path) {
        return date;
    }

    CreationDate::Unknown
}

/// Derive a date from the file's folder layout: a `MM-DD` event folder under a
/// `YYYY` year folder, or a folder named `YYYY-MM-DD` directly.
fn folder_proxy_date(path: &Path) -> Option<CreationDate> {
    let parent = path.parent()?.file_name()?.to_str()?.to_string();

    if let Some((y, m, d)) = parse_ymd_folder(&parent) {
        return Some(date_ymd(y, m, d));
    }

    if let Some((m, d)) = parse_md_folder(&parent) {
        let grand = path.parent()?.parent()?.file_name()?.to_str()?.to_string();
        if grand.len() == 4 && grand.bytes().all(|c| c.is_ascii_digit()) {
            if let Ok(y) = grand.parse::<u32>() {
                if (1..=12).contains(&m) && (1..=31).contains(&d) {
                    return Some(date_ymd(y, m, d));
                }
            }
        }
    }
    None
}

fn date_ymd(y: u32, m: u32, d: u32) -> CreationDate {
    CreationDate::DateCreated(format!("{y:04}-{m:02}-{d:02}"))
}

fn parse_ymd_folder(name: &str) -> Option<(u32, u32, u32)> {
    let b = name.as_bytes();
    let digit = |i: usize| b.get(i).map(|c| c.is_ascii_digit()).unwrap_or(false);
    if b.len() >= 10
        && (0..4).all(digit)
        && b[4] == b'-'
        && digit(5)
        && digit(6)
        && b[7] == b'-'
        && digit(8)
        && digit(9)
    {
        let y = name.get(0..4)?.parse().ok()?;
        let m = name.get(5..7)?.parse().ok()?;
        let d = name.get(8..10)?.parse().ok()?;
        return Some((y, m, d));
    }
    None
}

fn parse_md_folder(name: &str) -> Option<(u32, u32)> {
    let b = name.as_bytes();
    let digit = |i: usize| b.get(i).map(|c| c.is_ascii_digit()).unwrap_or(false);
    if b.len() >= 5 && digit(0) && digit(1) && b[2] == b'-' && digit(3) && digit(4) {
        let m = name.get(0..2)?.parse().ok()?;
        let d = name.get(3..5)?.parse().ok()?;
        return Some((m, d));
    }
    None
}

fn is_movie_ext(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(ext) if matches!(ext.to_ascii_lowercase().as_str(), "mp4" | "mov" | "m4v")
    )
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

pub fn resolve(dt_original: Option<String>, create_date: Option<String>) -> CreationDate {
    dt_original
        .or(create_date)
        .map(CreationDate::DateCreated)
        .unwrap_or(CreationDate::Unknown)
}
