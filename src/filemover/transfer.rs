use std::path::{Path, PathBuf};

use super::dates::destination_for;
use super::naming::{file_name, NameRegistry};
use super::outcome::BatchOutcome;
use super::safe_fs::{relocate_file, transfer_into};
use super::{FileMoverError, Operation, Progress};
use crate::exif::{creation_date, CreationDate};

#[derive(Debug)]
pub struct CopyOutcome {
    pub copied: usize,
    pub skipped_no_exif: usize,
    pub targets: Vec<PathBuf>,
}

/// Transfer candidate originals that carry valid EXIF data into
/// `destination`, laid out per `format`. Tokens: `YYYY`, `MM`, `DD` come from
/// the EXIF date; `<folder description>` (or `DESC`) comes from the source
/// folder name. `Operation::Copy` leaves the source in place; `Move` removes it.
pub fn copy_originals(
    originals: &[PathBuf],
    destination: &Path,
    format: &str,
) -> Result<CopyOutcome, FileMoverError> {
    copy_originals_with_progress(originals, destination, format, &|_, _| {})
}

pub fn copy_originals_with_progress(
    originals: &[PathBuf],
    destination: &Path,
    format: &str,
    progress: Progress,
) -> Result<CopyOutcome, FileMoverError> {
    transfer_originals_with_progress(originals, destination, format, Operation::Copy, progress)
}

pub fn transfer_originals_with_progress(
    originals: &[PathBuf],
    destination: &Path,
    format: &str,
    op: Operation,
    progress: Progress,
) -> Result<CopyOutcome, FileMoverError> {
    // No carried dates: extract per file (the slow legacy path).
    let dated: Vec<(PathBuf, CreationDate)> = originals
        .iter()
        .map(|p| (p.clone(), creation_date(p)))
        .collect();
    transfer_dated_with_progress(&dated, destination, format, op, progress)
}

/// Transfer files to their exif-dated folders using **already-resolved**
/// dates — no per-file exif re-extraction, so same-volume moves are pure
/// renames. Dates that don't parse (e.g. `unknown`) count as skipped.
pub fn transfer_dated_with_progress(
    dated: &[(PathBuf, CreationDate)],
    destination: &Path,
    format: &str,
    op: Operation,
    progress: Progress,
) -> Result<CopyOutcome, FileMoverError> {
    let mut outcome = CopyOutcome {
        copied: 0,
        skipped_no_exif: 0,
        targets: Vec::new(),
    };
    let mut names = NameRegistry::new();
    let total = dated.len();

    for (i, (path, date)) in dated.iter().enumerate() {
        match destination_for(path, date, destination, format) {
            Some(target) => {
                let dir = target.parent().unwrap_or(destination);
                let target = dir.join(names.claim(dir, &file_name(path)));
                relocate_file(path, &target, op)?;
                outcome.copied += 1;
                outcome.targets.push(target);
            }
            None => outcome.skipped_no_exif += 1,
        }
        progress(i + 1, total);
    }

    Ok(outcome)
}

/// Copy or move a set of files into a single destination folder (flat, no
/// structure preserved). Copy is verified byte-for-byte before reporting
/// success; Move is copy -> verify -> remove, so a failure never loses a
/// file. Collisions auto-rename, never overwrite.
pub fn transfer_files_with_progress(
    files: &[PathBuf],
    destination: &Path,
    op: Operation,
    progress: Progress,
) -> BatchOutcome {
    let mut outcome = BatchOutcome::planned(files.len());
    let mut names = NameRegistry::new();
    let total = files.len();

    for (i, path) in files.iter().enumerate() {
        progress(i + 1, total);
        outcome.record(transfer_into(path, destination, &mut names, op));
    }

    outcome
}
