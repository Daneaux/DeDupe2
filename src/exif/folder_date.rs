//! Dates written in folder names, for files that carry no date of their own.
//!
//! Recognised shapes (case-insensitive, anywhere in the name):
//!
//! | Shape | Example |
//! |---|---|
//! | year-month-day, `-` `_` or `.` separated | `2013-07-30`, `2013_07_30`, `2013.07.30` |
//! | compact year-month-day | `20130730` |
//! | day-month-year / month-day-year | `30-07-2013`, `07-30-2013` |
//! | year-month (no day) | `2013-07` |
//! | day, month name, year | `30 July 2013`, `30-Jul-2013` |
//! | month name, day, year | `July 30 2013`, `July 30, 2013` |
//! | month name, year (no day) | `July 2013`, `Jul 2013` |
//!
//! Rules that keep non-dates out:
//! - a date must be bounded by the start/end of the name or a non-letter,
//!   non-digit character: `IMG_20130730` parses, `IMG20130730` and
//!   `201307301` do not;
//! - years are 1900–2100, months 1–12, and the day must exist in that month;
//! - numeric day/month order is only decided when it is unambiguous:
//!   `30-07-2013` is 30 July and `07-30-2013` is 30 July, but `07-08-2013`
//!   (7 Aug or 8 Jul?) is not a date. Equal parts (`07-07-2013`) are fine.
//!
//! A full date anywhere in the name wins over a month-only one.

use std::ops::Range;

/// A date read from a folder name. `day` is `None` for month-precision
/// names such as `2013-07` or `July 2013`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderDate {
    pub year: u32,
    pub month: u32,
    pub day: Option<u32>,
}

impl FolderDate {
    /// `YYYY-MM-DD`, or `YYYY-MM` without a day.
    pub fn render(&self) -> String {
        match self.day {
            Some(day) => format!("{:04}-{:02}-{day:02}", self.year, self.month),
            None => format!("{:04}-{:02}", self.year, self.month),
        }
    }

    /// A full date, if it exists on the calendar (years 1900–2100).
    pub fn full(year: u32, month: u32, day: u32) -> Option<Self> {
        (valid_year(year) && (1..=12).contains(&month) && day >= 1 && day <= days_in(year, month))
            .then_some(Self {
                year,
                month,
                day: Some(day),
            })
    }

    fn month_only(year: u32, month: u32) -> Option<Self> {
        (valid_year(year) && (1..=12).contains(&month)).then_some(Self {
            year,
            month,
            day: None,
        })
    }
}

/// The date in a folder name and the byte range it occupies (so callers can
/// remove it to get the description). Full dates beat month-only ones;
/// otherwise the leftmost match wins.
pub fn parse_folder_date(name: &str) -> Option<(FolderDate, Range<usize>)> {
    let tokens = tokenize(name);
    let mut month_only = None;
    for start in 0..tokens.len() {
        if !tokens[start].begins_word(&tokens, start) {
            continue;
        }
        for shape in SHAPES {
            if let Some((date, end)) = shape(&tokens, start) {
                if !ends_word(&tokens, end) {
                    continue;
                }
                let span = tokens[start].span.start..tokens[end - 1].span.end;
                if date.day.is_some() {
                    return Some((date, span));
                }
                month_only.get_or_insert((date, span));
            }
        }
    }
    month_only
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Digits,
    Letters,
    Other,
}

#[derive(Debug)]
struct Token<'a> {
    kind: Kind,
    text: &'a str,
    span: Range<usize>,
}

impl Token<'_> {
    fn begins_word(&self, tokens: &[Token], index: usize) -> bool {
        self.kind != Kind::Other && (index == 0 || tokens[index - 1].kind == Kind::Other)
    }

    fn number(&self, len: Range<usize>) -> Option<u32> {
        (self.kind == Kind::Digits && len.contains(&self.text.len()))
            .then(|| self.text.parse().ok())
            .flatten()
    }

    /// Exactly one separator character, out of `allowed`.
    fn separator(&self, allowed: &str) -> Option<char> {
        let mut chars = self.text.chars();
        match (self.kind, chars.next(), chars.next()) {
            (Kind::Other, Some(c), None) if allowed.contains(c) => Some(c),
            _ => None,
        }
    }

    /// A run of spaces and at most one of `-_.,` — the glue between words in
    /// `30 July 2013`, `July 30, 2013` or `30-Jul-2013`.
    fn is_word_gap(&self) -> bool {
        self.kind == Kind::Other
            && self.text.chars().all(|c| " -_.,".contains(c))
            && self.text.chars().filter(|c| *c != ' ').count() <= 1
    }

    fn month_name(&self) -> Option<u32> {
        if self.kind != Kind::Letters {
            return None;
        }
        let word = self.text.to_ascii_lowercase();
        MONTHS
            .iter()
            .position(|(full, short)| {
                word == *full || word == *short || (*short == "sep" && word == "sept")
            })
            .map(|i| i as u32 + 1)
    }
}

fn ends_word(tokens: &[Token], end: usize) -> bool {
    end == tokens.len() || tokens[end].kind == Kind::Other
}

fn tokenize(name: &str) -> Vec<Token<'_>> {
    let kind = |c: char| {
        if c.is_ascii_digit() {
            Kind::Digits
        } else if c.is_alphabetic() {
            Kind::Letters
        } else {
            Kind::Other
        }
    };
    let mut tokens: Vec<Token> = Vec::new();
    for (i, c) in name.char_indices() {
        let k = kind(c);
        match tokens.last_mut() {
            Some(last) if last.kind == k => {
                last.span.end = i + c.len_utf8();
                last.text = &name[last.span.clone()];
            }
            _ => tokens.push(Token {
                kind: k,
                text: &name[i..i + c.len_utf8()],
                span: i..i + c.len_utf8(),
            }),
        }
    }
    tokens
}

/// A shape tries to match starting at token `i`; on success it returns the
/// date and the index one past its last token.
type Shape = fn(&[Token], usize) -> Option<(FolderDate, usize)>;

const SHAPES: &[Shape] = &[
    year_month_day,
    compact,
    day_month_year,
    day_name_year,
    name_day_year,
    year_month,
    name_year,
];

const NUMERIC_SEPARATORS: &str = "-_.";

/// `2013-07-30`, `2013_07_30`, `2013.07.30` (one separator kind).
fn year_month_day(t: &[Token], i: usize) -> Option<(FolderDate, usize)> {
    let year = t.get(i)?.number(4..5)?;
    let sep = t.get(i + 1)?.separator(NUMERIC_SEPARATORS)?;
    let month = t.get(i + 2)?.number(2..3)?;
    (t.get(i + 3)?.separator(NUMERIC_SEPARATORS)? == sep).then_some(())?;
    let day = t.get(i + 4)?.number(2..3)?;
    Some((FolderDate::full(year, month, day)?, i + 5))
}

/// `20130730`.
fn compact(t: &[Token], i: usize) -> Option<(FolderDate, usize)> {
    let token = t.get(i)?;
    token.number(8..9)?;
    let n = |r: Range<usize>| token.text[r].parse::<u32>().ok();
    Some((FolderDate::full(n(0..4)?, n(4..6)?, n(6..8)?)?, i + 1))
}

/// `30-07-2013` (day first) or `07-30-2013` (month first), only when the
/// order is unambiguous.
fn day_month_year(t: &[Token], i: usize) -> Option<(FolderDate, usize)> {
    let a = t.get(i)?.number(2..3)?;
    let sep = t.get(i + 1)?.separator(NUMERIC_SEPARATORS)?;
    let b = t.get(i + 2)?.number(2..3)?;
    (t.get(i + 3)?.separator(NUMERIC_SEPARATORS)? == sep).then_some(())?;
    let year = t.get(i + 4)?.number(4..5)?;
    let (day, month) = match (a > 12, b > 12) {
        (true, false) => (a, b),
        (false, true) => (b, a),
        (false, false) if a == b => (a, b),
        _ => return None, // ambiguous, or neither can be a month
    };
    Some((FolderDate::full(year, month, day)?, i + 5))
}

/// `30 July 2013`, `30-Jul-2013`.
fn day_name_year(t: &[Token], i: usize) -> Option<(FolderDate, usize)> {
    let day = t.get(i)?.number(1..3)?;
    t.get(i + 1)?.is_word_gap().then_some(())?;
    let month = t.get(i + 2)?.month_name()?;
    t.get(i + 3)?.is_word_gap().then_some(())?;
    let year = t.get(i + 4)?.number(4..5)?;
    Some((FolderDate::full(year, month, day)?, i + 5))
}

/// `July 30 2013`, `July 30, 2013`.
fn name_day_year(t: &[Token], i: usize) -> Option<(FolderDate, usize)> {
    let month = t.get(i)?.month_name()?;
    t.get(i + 1)?.is_word_gap().then_some(())?;
    let day = t.get(i + 2)?.number(1..3)?;
    t.get(i + 3)?.is_word_gap().then_some(())?;
    let year = t.get(i + 4)?.number(4..5)?;
    Some((FolderDate::full(year, month, day)?, i + 5))
}

/// `2013-07` (month precision). A following `-DD` makes it a full date,
/// handled by [`year_month_day`]; here it must not be followed by more
/// numeric parts.
fn year_month(t: &[Token], i: usize) -> Option<(FolderDate, usize)> {
    let year = t.get(i)?.number(4..5)?;
    t.get(i + 1)?.separator(NUMERIC_SEPARATORS)?;
    let month = t.get(i + 2)?.number(2..3)?;
    // `2013-07-30` is a full date and `2013-07_30` is malformed: neither is
    // a month-only `2013-07`.
    let continues = t
        .get(i + 3)
        .and_then(|s| s.separator(NUMERIC_SEPARATORS))
        .is_some()
        && t.get(i + 4).is_some_and(|n| n.kind == Kind::Digits);
    (!continues).then_some(())?;
    Some((FolderDate::month_only(year, month)?, i + 3))
}

/// `July 2013`, `Jul 2013` (month precision). Not when a day-like number
/// precedes the month: `32 July 2013` is a malformed full date, not July.
fn name_year(t: &[Token], i: usize) -> Option<(FolderDate, usize)> {
    let after_day = i >= 2 && t[i - 1].is_word_gap() && t[i - 2].number(1..3).is_some();
    (!after_day).then_some(())?;
    let month = t.get(i)?.month_name()?;
    t.get(i + 1)?.is_word_gap().then_some(())?;
    let year = t.get(i + 2)?.number(4..5)?;
    Some((FolderDate::month_only(year, month)?, i + 3))
}

const MONTHS: [(&str, &str); 12] = [
    ("january", "jan"),
    ("february", "feb"),
    ("march", "mar"),
    ("april", "apr"),
    ("may", "may"),
    ("june", "jun"),
    ("july", "jul"),
    ("august", "aug"),
    ("september", "sep"),
    ("october", "oct"),
    ("november", "nov"),
    ("december", "dec"),
];

fn valid_year(year: u32) -> bool {
    (1900..=2100).contains(&year)
}

fn days_in(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(name: &str) -> Option<String> {
        parse_folder_date(name).map(|(d, _)| d.render())
    }

    fn rest(name: &str) -> String {
        let (_, span) = parse_folder_date(name).unwrap();
        format!("{}|{}", &name[..span.start], &name[span.end..])
    }

    /// Every row of the table in issues.md.
    #[test]
    fn issue_table_rows() {
        let rows = [
            ("Hawaii 2013-07-30", "2013-07-30"),
            ("2013_07_30 Hawaii", "2013-07-30"),
            ("2013.07.30 Hawaii", "2013-07-30"),
            ("20130730 Hawaii", "2013-07-30"),
            ("30-07-2013 Hawaii", "2013-07-30"),
            ("2013-07 Hawaii", "2013-07"),
            ("July 2013 trip", "2013-07"),
            ("Jul 2013", "2013-07"),
            ("30 July 2013", "2013-07-30"),
            ("July 30 2013", "2013-07-30"),
        ];
        for (name, expected) in rows {
            assert_eq!(date(name).as_deref(), Some(expected), "{name}");
        }
    }

    #[test]
    fn shapes_already_supported_still_parse() {
        assert_eq!(date("2013-07-30 Hawaii").as_deref(), Some("2013-07-30"));
        assert_eq!(date("2013-07-30").as_deref(), Some("2013-07-30"));
    }

    #[test]
    fn more_spellings() {
        for (name, expected) in [
            ("July 30, 2013", "2013-07-30"),
            ("30-Jul-2013", "2013-07-30"),
            ("sept 2013", "2013-09"),
            ("SEPTEMBER 2013", "2013-09"),
            ("07-30-2013", "2013-07-30"),
            ("07-07-2013", "2013-07-07"),
            ("IMG_20130730", "2013-07-30"),
            ("(2013-07-30)", "2013-07-30"),
            ("trip 2020-02-29", "2020-02-29"),
        ] {
            assert_eq!(date(name).as_deref(), Some(expected), "{name}");
        }
    }

    #[test]
    fn ambiguous_numeric_orders_stay_unknown() {
        for name in ["07-08-2013", "01-12-2013 party", "12_11_2020"] {
            assert_eq!(date(name), None, "{name}");
        }
    }

    #[test]
    fn numbers_that_are_not_dates_stay_unknown() {
        for name in [
            "Trip 2013 - 1200 photos",
            "Room 1012",
            "2013",
            "Hawaii",
            "",
            "IMG20130730",
            "201307301",
            "20131330",
            "2013-13-01",
            "2013-02-30",
            "2019-02-29",
            "1850-07-30",
            "2101-01-01",
            "30-30-2013",
            "may day",
            "March madness 12",
            "2013-07-30x",
            "x2013-07-30",
            "2013-07_30",
            "2013-7-30",
            "2013 07 30",
            "32 July 2013",
            "July 2013x",
        ] {
            assert_eq!(date(name), None, "{name:?}");
        }
    }

    #[test]
    fn a_full_date_beats_a_month_only_one() {
        assert_eq!(
            date("July 2013 - 2013-07-30").as_deref(),
            Some("2013-07-30")
        );
        assert_eq!(
            date("2013-07 trip 30 July 2013").as_deref(),
            Some("2013-07-30")
        );
    }

    #[test]
    fn the_leftmost_date_wins_among_equals() {
        assert_eq!(
            date("2013-07-30 to 2013-08-02").as_deref(),
            Some("2013-07-30")
        );
    }

    #[test]
    fn spans_cover_exactly_the_date() {
        assert_eq!(rest("Hawaii 2013-07-30"), "Hawaii |");
        assert_eq!(rest("2013_07_30 Hawaii"), "| Hawaii");
        assert_eq!(rest("July 30, 2013 beach"), "| beach");
        assert_eq!(rest("trip July 2013"), "trip |");
        assert_eq!(rest("café 2013-07-30 été"), "café | été");
    }
}
