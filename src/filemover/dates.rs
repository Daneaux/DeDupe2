//! Pure date and folder-name logic: parsing capture dates, recognizing
//! `MM-DD event` folders, and rendering destination folder templates.

use std::path::{Path, PathBuf};

use super::naming::file_name;
use crate::exif::{parse_folder_date, CreationDate};

/// `(year, month, day)` from a capture date. The day is `None` for a
/// month-precision date (`YYYY-MM`, from a folder name such as `July 2013`).
pub(super) fn parse_date(date: &CreationDate) -> Option<(u32, u32, Option<u32>)> {
    let s = match date {
        CreationDate::DateCreated(v) => v,
        CreationDate::Unknown => return None,
    };

    let digits: Vec<u32> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;

    match digits[..] {
        [year, month, day, ..] if (1..=12).contains(&month) && (1..=31).contains(&day) => {
            Some((year, month, Some(day)))
        }
        [year, month] if (1..=12).contains(&month) => Some((year, month, None)),
        _ => None,
    }
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

/// The descriptive part of the folder a file originated from: the folder
/// name with its date removed, wherever the date sits (`Hawaii 2013-07-30`,
/// `2013_07_30 Hawaii` and `July 2013 Hawaii` all give `Hawaii`), or without
/// its `MM-DD` prefix (`03-15 hawaii` gives `hawaii`). A name without a date
/// is used whole.
pub(super) fn folder_description(path: &Path) -> String {
    let name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
    describe_folder(name)
}

/// [`folder_description`] for a bare folder name.
pub(super) fn describe_folder(name: &str) -> String {
    /// Punctuation that only belonged to the date (`2013-07-30 - Hawaii`,
    /// `(July 2013) party`).
    const GLUE: &[char] = &[' ', '-', '_', '.', ',', '(', ')', '[', ']'];

    if let Some((_, span)) = parse_folder_date(name) {
        let before = name[..span.start].trim_end_matches(GLUE);
        let after = name[span.end..].trim_start_matches(GLUE);
        return [before, after]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();
    }
    // `MM-DD event`, but not the start of a longer numeric date that was
    // left unparsed because it is ambiguous (`07-08-2013`).
    let longer_date = matches!(name.as_bytes().get(5), Some(b'-' | b'_' | b'.'));
    if is_event_name(name) && !longer_date {
        name[5..].trim().to_string()
    } else {
        name.trim().to_string()
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
    let rendered = render_date_folder_parts(format, year, month, day, &description);

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

/// Render a destination folder template (`YYYY`, `MM`, `DD`, and
/// `<folder description>` / `<desc>` / `DESC`) for a full date.
pub fn render_date_folder(
    format: &str,
    year: u32,
    month: u32,
    day: u32,
    description: &str,
) -> String {
    render_date_folder_parts(format, year, month, Some(day), description)
}

/// [`render_date_folder`] where the day may be unknown: then `DD` is
/// dropped together with one separator next to it, so `YYYY/MM-DD trip`
/// renders as `2013/07 trip`.
pub fn render_date_folder_parts(
    format: &str,
    year: u32,
    month: u32,
    day: Option<u32>,
    description: &str,
) -> String {
    // Bind the date tokens FIRST, against the raw template only — the
    // description is spliced in afterwards so its own text can never be
    // mistaken for a token (a description like "MM" must survive verbatim).
    let template = match day {
        Some(_) => format.to_string(),
        None => without_day_token(format),
    };
    let mut out = template
        .replace("YYYY", &format!("{year:04}"))
        .replace("MM", &format!("{month:02}"));
    if let Some(day) = day {
        out = out.replace("DD", &format!("{day:02}"));
    }

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

/// `format` with every `DD` removed, plus the separator just before it (or,
/// failing that, just after it).
fn without_day_token(format: &str) -> String {
    const SEPARATORS: &[char] = &['-', '_', '.', ' '];
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(at) = rest.find("DD") {
        let (head, tail) = (&rest[..at], &rest[at + 2..]);
        if head.ends_with(SEPARATORS) {
            out.push_str(&head[..head.len() - 1]);
            rest = tail;
        } else {
            out.push_str(head);
            rest = tail.strip_prefix(SEPARATORS).unwrap_or(tail);
        }
    }
    out.push_str(rest);
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
            Some((2022, 8, Some(17)))
        );
        assert_eq!(
            parse_date(&CreationDate::DateCreated("2022-12-31".into())),
            Some((2022, 12, Some(31)))
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
    fn parse_date_accepts_month_precision() {
        assert_eq!(
            parse_date(&CreationDate::DateCreated("2013-07".into())),
            Some((2013, 7, None))
        );
        assert_eq!(
            parse_date(&CreationDate::DateCreated("2013-13".into())),
            None
        );
    }

    #[test]
    fn descriptions_drop_the_date_wherever_it_is() {
        for (name, description) in [
            ("2023-03-15 hawaii", "hawaii"),
            ("2023-03-15", ""),
            ("Hawaii 2013-07-30", "Hawaii"),
            ("2013_07_30 Hawaii", "Hawaii"),
            ("2013.07.30 Hawaii", "Hawaii"),
            ("20130730 Hawaii", "Hawaii"),
            ("30-07-2013 Hawaii", "Hawaii"),
            ("2013-07 Hawaii", "Hawaii"),
            ("July 2013 trip", "trip"),
            ("30 July 2013 - beach", "beach"),
            ("trip (July 30, 2013) beach", "trip beach"),
            ("03-15 hawaii", "hawaii"),
            ("spring 2023", "spring 2023"),
            ("2023-3-15 haiti", "2023-3-15 haiti"),
            ("07-08-2013 party", "07-08-2013 party"),
        ] {
            assert_eq!(describe_folder(name), description, "{name}");
        }
    }

    #[test]
    fn month_only_dates_drop_the_day_token_and_its_separator() {
        let render = |format| render_date_folder_parts(format, 2013, 7, None, "trip");
        assert_eq!(render("YYYY/MM-DD <folder description>"), "2013/07 trip");
        assert_eq!(render("YYYY-MM-DD DESC"), "2013-07 trip");
        assert_eq!(render("YYYY/DD-MM"), "2013/07");
        assert_eq!(render("YYYY/MM/DD"), "2013/07/");
        assert_eq!(
            render_date_folder_parts("YYYY/MM-DD <folder description>", 2013, 7, Some(30), "trip"),
            render_date_folder("YYYY/MM-DD <folder description>", 2013, 7, 30, "trip")
        );
    }

    #[test]
    fn month_only_destinations() {
        let date = CreationDate::DateCreated("2013-07".into());
        let target = destination_for(
            Path::new("/src/July 2013 trip/a.jpg"),
            &date,
            Path::new("/dest"),
            "YYYY/MM-DD <folder description>",
        );
        assert_eq!(target, Some(PathBuf::from("/dest/2013/07 trip/a.jpg")));
    }
}
