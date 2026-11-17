//! Pure date and folder-name logic: parsing capture dates, recognizing
//! `MM-DD event` folders, and rendering destination folder templates.

use std::path::{Path, PathBuf};

use super::naming::file_name;
use crate::exif::CreationDate;

pub(super) fn parse_date(date: &CreationDate) -> Option<(u32, u32, u32)> {
    let s = match date {
        CreationDate::DateCreated(v) => v,
        CreationDate::Unknown => return None,
    };

    let digits: Vec<u32> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;

    if digits.len() < 3 {
        return None;
    }
    let (year, month, day) = (digits[0], digits[1], digits[2]);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

pub(super) fn extract_event(source_name: &str) -> String {
    let mut words: Vec<&str> = source_name
        .split([' ', '-', '_'])
        .filter(|w| !w.is_empty())
        .collect();

    while let Some(first) = words.first() {
        if is_date_token(first) {
            words.remove(0);
        } else {
            break;
        }
    }

    words.join("-")
}

fn is_date_token(word: &str) -> bool {
    if word.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    const MONTHS: [&str; 24] = [
        "jan",
        "feb",
        "mar",
        "apr",
        "may",
        "jun",
        "jul",
        "aug",
        "sep",
        "oct",
        "nov",
        "dec",
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    MONTHS.contains(&word.to_lowercase().as_str())
}

/// The descriptive part of the folder a file originated from, e.g.
/// `03-15 hawaii` -> `hawaii`; a folder without an `mm-dd` prefix uses its
/// whole name.
pub(super) fn folder_description(path: &Path) -> String {
    let name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
    // `YYYY-MM-DD <description>`: some candidate folders carry a full date;
    // interpret it as a date, not a description.
    if let Some(rest) = strip_full_date_prefix(name) {
        return rest.trim().to_string();
    }
    if is_event_name(name) {
        name[5..].trim().to_string()
    } else {
        name.trim().to_string()
    }
}

/// If `name` starts with `YYYY-MM-DD`, return what follows it.
pub(super) fn strip_full_date_prefix(name: &str) -> Option<&str> {
    let b = name.as_bytes();
    let digits = |i: usize| b.get(i).map(|c| c.is_ascii_digit()).unwrap_or(false);
    if b.len() >= 10
        && digits(0)
        && digits(1)
        && digits(2)
        && digits(3)
        && b[4] == b'-'
        && digits(5)
        && digits(6)
        && b[7] == b'-'
        && digits(8)
        && digits(9)
    {
        name.get(10..)
    } else {
        None
    }
}

/// The full target path an original would be copied to, or `None` when the
/// date isn't a valid calendar date. Pure computation: no file I/O.
pub fn destination_for(
    path: &Path,
    date: &CreationDate,
    destination: &Path,
    format: &str,
) -> Option<PathBuf> {
    let (year, month, day) = parse_date(date)?;
    let description = folder_description(path);
    let rendered = render_date_folder(format, year, month, day, &description);

    // If the destination root already ends with the year (e.g.
    // `.../AllPhotos/2014`), don't double it (`2014/2014/...`).
    let rendered = match (
        year.to_string(),
        destination.file_name().and_then(|n| n.to_str()),
    ) {
        (year_str, Some(root_tail)) if root_tail == year_str => rendered
            .split_once('/')
            .map(|x| x.1)
            .map(|s| s.to_string())
            .unwrap_or(rendered),
        _ => rendered,
    };

    Some(destination.join(rendered).join(file_name(path)))
}

pub fn render_date_folder(
    format: &str,
    year: u32,
    month: u32,
    day: u32,
    description: &str,
) -> String {
    // Bind the date tokens FIRST, against the raw template only — the
    // description is spliced in afterwards so its own text can never be
    // mistaken for a token (a description like "MM" must survive verbatim).
    let mut out = format
        .to_string()
        .replace("YYYY", &format!("{year:04}"))
        .replace("MM", &format!("{month:02}"))
        .replace("DD", &format!("{day:02}"));

    if description.is_empty() {
        out = out
            .replace("<folder description>", "")
            .replace("<desc>", "")
            .replace("DESC", "");
        out = out
            .split('/')
            .map(|seg| seg.trim().trim_matches(['-', '_', ' ']).trim().to_string())
            .collect::<Vec<_>>()
            .join("/");
    } else {
        out = out
            .replace("<folder description>", description)
            .replace("<desc>", description)
            .replace("DESC", description);
    }
    out
}

pub(super) fn is_event_name(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 5
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2] == b'-'
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_date_rejects_out_of_range_and_short_values() {
        assert_eq!(
            parse_date(&CreationDate::DateCreated("2022:08:17 10:00:00".into())),
            Some((2022, 8, 17))
        );
        assert_eq!(
            parse_date(&CreationDate::DateCreated("2022-12-31".into())),
            Some((2022, 12, 31))
        );
        assert_eq!(parse_date(&CreationDate::Unknown), None);
        assert_eq!(
            parse_date(&CreationDate::DateCreated("2022:13:01".into())),
            None,
            "month 13 is not a date"
        );
        assert_eq!(
            parse_date(&CreationDate::DateCreated("2022:01:32".into())),
            None,
            "day 32 is not a date"
        );
        assert_eq!(
            parse_date(&CreationDate::DateCreated("2022".into())),
            None,
            "a year alone is not enough"
        );
    }

    #[test]
    fn extract_event_strips_only_leading_date_tokens() {
        assert_eq!(extract_event("2023-03-15 hawaii wedding"), "hawaii-wedding");
        assert_eq!(extract_event("March 2023 trip"), "trip");
        assert_eq!(extract_event("hawaii 2023"), "hawaii-2023");
        assert_eq!(extract_event("03-15"), "");
    }

    #[test]
    fn strip_full_date_prefix_requires_a_full_date() {
        assert_eq!(strip_full_date_prefix("2023-03-15 hawaii"), Some(" hawaii"));
        assert_eq!(strip_full_date_prefix("2023-3-15 haiti"), None);
        assert_eq!(strip_full_date_prefix("spring 2023"), None);
        assert_eq!(strip_full_date_prefix("2023-03-15"), Some(""));
    }
}
