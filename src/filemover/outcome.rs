use std::path::{Path, PathBuf};

use super::safe_fs::{Failed, Placed};

/// What a batch of verified moves or copies did. Shared by purgatory,
/// re-home and table transfers, which all place files one at a time and
/// never let one failure stop the rest.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BatchOutcome {
    /// Files the batch was asked to handle.
    pub planned: usize,
    /// Files that reached their destination (moved, or copied for
    /// [`super::Operation::Copy`]).
    pub moved: usize,
    /// Of `moved`, how many needed a new name to avoid a collision.
    pub renamed_on_collision: usize,
    /// Files that were not handled, with the reason. Their sources are
    /// always left in place.
    pub failures: Vec<(PathBuf, String)>,
}

/// Outcome of [`super::move_to_purgatory_with_progress`] and
/// [`super::purge_proven_duplicates_with_progress`].
pub type PurgatoryOutcome = BatchOutcome;
/// Outcome of [`super::rehome_verified`].
pub type RehomeOutcome = BatchOutcome;
/// Outcome of [`super::transfer_files_with_progress`].
pub type TransferOutcome = BatchOutcome;

impl BatchOutcome {
    pub(super) fn planned(planned: usize) -> Self {
        Self {
            planned,
            ..Self::default()
        }
    }

    /// Count one file's transfer result.
    pub(super) fn record(&mut self, result: Result<Placed, Failed>) {
        match result {
            Ok(placed) => {
                self.moved += 1;
                self.renamed_on_collision += usize::from(placed.renamed);
            }
            Err(failed) => self.failures.push(failed.into_row()),
        }
    }

    /// Count a file that was refused before any transfer was attempted.
    pub(super) fn fail(&mut self, path: &Path, reason: impl Into<String>) {
        self.failures.push((path.to_path_buf(), reason.into()));
    }
}
