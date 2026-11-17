//! Helpers shared by the page modules: path validation, form parsing,
//! SSE progress streaming and table view rows.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use axum::response::sse::Event;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};

use crate::exif::{creation_date, CreationDate};

/// Resolve a path for comparison: canonicalize the deepest existing
/// ancestor (resolving symlinks, e.g. a symlinked `4tbext`) and keep the
/// non-existent tail attached, so a not-yet-created destination can still be
/// compared against an existing source.
pub(super) fn resolved_path(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !current.exists() {
        match current.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => break,
        }
        if !current.pop() {
            break;
        }
    }
    let mut resolved = std::fs::canonicalize(&current).unwrap_or(current);
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    resolved
}

/// Whether one tree contains the other (equal counts as contained).
pub(super) fn trees_nested(a: &Path, b: &Path) -> Option<(&'static str, PathBuf, PathBuf)> {
    let (ra, rb) = (resolved_path(a), resolved_path(b));
    if ra == rb {
        return Some(("", ra, rb));
    }
    if ra.starts_with(&rb) {
        return Some(("A is inside B", ra, rb));
    }
    if rb.starts_with(&ra) {
        return Some(("B is inside A", ra, rb));
    }
    None
}

/// Two-tree operations compare trees against each other, so nesting them
/// makes the intersection compare against itself (self-pairs, empty
/// originals). Reject before any scanning.
pub(super) fn require_disjoint_trees(a: &Path, b: &Path) -> Result<(), (StatusCode, String)> {
    match trees_nested(a, b) {
        None => Ok(()),
        Some(("", _, _)) => Err((
            StatusCode::BAD_REQUEST,
            format!("the two trees are the same: {}", a.display()),
        )),
        Some((which, ra, rb)) => Err((
            StatusCode::BAD_REQUEST,
            format!(
                "the trees overlap ({which}) — pick disjoint trees: {} vs {}",
                ra.display(),
                rb.display()
            ),
        )),
    }
}

/// Input trees must exist before a scan starts — otherwise the failure
/// surfaces only after the (long) scan of the other tree (or worse, silently
/// scans nothing thanks to the resilient collector).
pub(super) fn require_dir(path: &Path) -> Result<(), (StatusCode, String)> {
    if path.is_dir() {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            format!("not a directory: {}", path.display()),
        ))
    }
}

/// Resolve the transfer dates for a set of paths from the form's carried
/// dates: `unknown`-marked paths stay unknown (no re-read), paths not seen by
/// the scan fall back to extraction.
pub(super) fn dated_from_form(
    files: &[PathBuf],
    carried: &HashMap<String, CreationDate>,
    carried_unknown: &HashSet<String>,
) -> Vec<(PathBuf, CreationDate)> {
    files
        .iter()
        .map(|path| {
            let key = to_string(path);
            let date = if let Some(date) = carried.get(&key) {
                date.clone()
            } else if carried_unknown.contains(&key) {
                CreationDate::Unknown
            } else {
                creation_date(path)
            };
            (path.clone(), date)
        })
        .collect()
}

/// Parse the hidden `dates` field. Returns `path -> date` for dated files and
/// the set of paths the scan already resolved as `unknown` — those are known,
/// just undatable, and must NOT trigger a fallback re-extraction (rawler on
/// each of them is a silent, minutes-long stall before the first copy).
pub(super) fn parse_carried_dates(input: &str) -> (HashMap<String, CreationDate>, HashSet<String>) {
    let mut dated = HashMap::new();
    let mut unknown = HashSet::new();
    for line in input.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '\t');
        let path = parts.next().unwrap_or("").trim();
        let date = parts.next().unwrap_or("").trim();
        if path.is_empty() || date.is_empty() {
            continue;
        }
        if date.eq_ignore_ascii_case("unknown") {
            unknown.insert(path.to_string());
        } else {
            dated.insert(
                path.to_string(),
                CreationDate::DateCreated(date.to_string()),
            );
        }
    }
    (dated, unknown)
}

pub(super) struct PathRowView {
    pub path: String,
}

pub(super) struct DupRowView {
    pub tree: String,
    pub path: String,
}

pub(super) struct ExifRowView {
    pub path: String,
    pub date: String,
    pub destination: String,
    pub valid: bool,
}

pub(super) const MAX_TABLE_ROWS: usize = 20;

pub(super) const DEFAULT_FORMAT: &str = "YYYY/MM-DD <folder description>";

pub(super) enum Msg {
    Progress(usize, usize),
    Done(String),
    Error(String),
}

pub(super) fn parse_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

pub(super) fn to_string(p: impl AsRef<Path>) -> String {
    p.as_ref().display().to_string()
}

pub(super) fn sse_stream(
    rx: mpsc::UnboundedReceiver<Msg>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    UnboundedReceiverStream::new(rx).map(|msg| {
        let (event, data) = match msg {
            Msg::Progress(done, total) => ("progress", format!("{done} {total}")),
            Msg::Done(html) => ("done", html),
            Msg::Error(e) => ("error", e),
        };
        Ok(Event::default().event(event).data(data))
    })
}

pub(super) fn truncate_rows<T>(items: Vec<T>) -> (Vec<T>, usize) {
    if items.len() > MAX_TABLE_ROWS {
        let more = items.len() - MAX_TABLE_ROWS;
        (items.into_iter().take(MAX_TABLE_ROWS).collect(), more)
    } else {
        (items, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_rows;

    #[test]
    fn truncate_rows_reports_only_the_overflow() {
        let (rows, more) = truncate_rows((0..20).collect::<Vec<_>>());
        assert_eq!(rows.len(), 20);
        assert_eq!(more, 0);

        let (rows, more) = truncate_rows((0..21).collect::<Vec<_>>());
        assert_eq!(rows.len(), 20);
        assert_eq!(more, 1, "overflow count powers the 'more not shown' row");

        let (rows, more) = truncate_rows(Vec::<u8>::new());
        assert!(rows.is_empty());
        assert_eq!(more, 0);
    }
}
