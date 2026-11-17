# Issues

## 1. Recognise more folder-name date formats

**Status:** done (2026-11-16). Parser in `src/exif/folder_date.rs`; see
"Decisions" at the end of this issue.

When a file has no capture date of its own (EXIF, CIFF, RAW or XMP), or reading
its metadata times out, `creation_date()` falls back to a date taken from the
folder name (`folder_proxy_date` in `src/exif/mod.rs`). Organize,
Verify/rehome, the EXIF/copy step and Compare's dated originals all rely on this
fallback.

### Recognised today

Only the file's immediate parent folder is checked:

- `YYYY/MM-DD description`, e.g. `2013/07-30 Hawaii`
- `YYYY-MM-DD description`, e.g. `2013-07-30 Hawaii`

### Should also be recognised

| Shape | Example | Date |
|---|---|---|
| Date not at the start | `Hawaii 2013-07-30` | 2013-07-30 |
| Underscore separators | `2013_07_30 Hawaii` | 2013-07-30 |
| Dot separators | `2013.07.30 Hawaii` | 2013-07-30 |
| No separators | `20130730 Hawaii` | 2013-07-30 |
| Day-first order | `30-07-2013 Hawaii` | 2013-07-30 |
| Year and month only | `2013-07 Hawaii` | 2013-07 (no day) |
| Month name with year | `July 2013 trip`, `Jul 2013` | 2013-07 (no day) |
| Day, month name, year | `30 July 2013`, `July 30 2013` | 2013-07-30 |
| Date folder further up the tree | `2013-07-30 Hawaii/raw/IMG.CR2` | 2013-07-30 |

### Design questions to settle

- **Ambiguous numeric orders.** `07-08-2013` is 7 August day-first but 8 July
  month-first. Pick a rule (e.g. day-first only when the first number is above
  12, otherwise unknown) or make the order a setting. Never guess silently.
- **Partial dates.** `2013-07` and `July 2013` have no day. Decide how they are
  represented (`CreationDate` currently holds a full date string) and how the
  destination template renders `DD` for them.
- **Searching ancestors.** Choose how far up the tree to look (nearest dated
  folder wins), and stop at the scan root so a date in a parent of the library
  doesn't leak into every file.
- **False positives.** Numbers that aren't dates: `IMG_20130730`-style names
  are fine, but `Trip 2013 - 1200 photos` or `Room 1012` must not parse as
  dates. Require plausible ranges (year 1900–2100, month 1–12, day valid for
  the month) and word boundaries.
- **Description extraction.** `folder_description` (`src/filemover/dates.rs`)
  strips a leading date to get the event name; it needs the same formats, so
  `Hawaii 2013-07-30` yields `Hawaii`.

### Done when

- Every row in the table above parses to the listed date, with a test per row.
- Ambiguous orders and non-date numbers have tests showing they stay unknown.
- The embedded file date still takes priority over any folder date.
- `folder_description` handles the same shapes.

### Decisions

- **Ambiguous numeric orders:** decided only when one part can't be a
  month: `30-07-2013` and `07-30-2013` are both 30 July. `07-08-2013` stays
  unknown; equal parts (`07-07-2013`) are fine. Not a setting: there's no
  silent guess to configure.
- **Partial dates:** stored as `YYYY-MM` in `CreationDate`. Templates drop
  `DD` along with one adjacent separator (`YYYY/MM-DD trip` gives
  `2013/07 trip`), and `organize_by_date` names the folder `MM-event`.
  Verify treats a month-only folder as matching any day of that month and
  measures distances from its first day. Verify still treats a month-only
  capture date as undated, since it isn't a real capture time.
- **Searching ancestors:** nearest dated folder wins, searching up to
  `FOLDER_DATE_LEVELS` (3) folders above the file. It doesn't stop at the
  scan root yet: dates are cached per file, independent of which scan asked,
  so a root-dependent answer would make the cache inconsistent. The depth
  limit is what stops a date far above a library from leaking into every
  file. Revisit if a real layout needs more depth.
- **False positives:** years 1900–2100, months 1–12, days that exist in that
  month (leap years included). A date must be bounded by the start or end of
  the name or by a non-letter, non-digit character, so `IMG_20130730`
  parses but `IMG20130730`, `201307301`, `Room 1012` and
  `Trip 2013 - 1200 photos` don't. A malformed full date isn't downgraded:
  `32 July 2013` and `2013-07_30` are unknown, not July. A full date
  anywhere in the name beats a month-only one; otherwise the leftmost date
  wins.
- **Description extraction:** `folder_description` removes the date
  wherever it sits, plus any punctuation that only belonged to the date, so
  `Hawaii 2013-07-30` and `trip (July 30, 2013) beach` give `Hawaii` and
  `trip beach`. An ambiguous name is kept whole (`07-08-2013 party`).
- **Embedded dates first:** unchanged. The folder name is only a fallback
  when the file has no date of its own, or reading it times out.
