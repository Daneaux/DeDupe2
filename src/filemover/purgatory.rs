use std::path::{Path, PathBuf};

use super::naming::NameRegistry;
use super::safe_fs::{prune_empty_children, transfer_into};
use super::{Operation, Progress};

#[derive(Debug)]
pub struct PurgatoryOutcome {
    pub planned: usize,
    pub moved: usize,
    pub renamed_on_collision: usize,
    /// Files that could not be moved safely, with the reason. The source is
    /// always left intact for these.
    pub failures: Vec<(PathBuf, String)>,
}

/// Move duplicate files out of a tree into a side "purgatory" tree, keeping
/// each file's path relative to `source_root` under `purgatory_root`. Every
/// move is copy -> byte-verify -> remove; collisions auto-rename, never
/// overwrite; failures leave the source in place. Afterwards every directory
/// under `source_root` that became empty is removed — `source_root` itself is
/// always kept.
pub fn move_to_purgatory_with_progress(
    files: &[PathBuf],
    source_root: &Path,
    purgatory_root: &Path,
    progress: Progress,
) -> PurgatoryOutcome {
    let mut outcome = PurgatoryOutcome {
        planned: files.len(),
        moved: 0,
        renamed_on_collision: 0,
        failures: Vec::new(),
    };
    let mut names = NameRegistry::new();
    let total = files.len();

    for (i, path) in files.iter().enumerate() {
        progress(i + 1, total);

        let Ok(rel) = path.strip_prefix(source_root) else {
            outcome
                .failures
                .push((path.clone(), "file is not inside the candidate tree".into()));
            continue;
        };
        let Some(dir) = purgatory_root.join(rel).parent().map(Path::to_path_buf) else {
            outcome
                .failures
                .push((path.clone(), "target has no parent directory".into()));
            continue;
        };

        match transfer_into(path, &dir, &mut names, Operation::Move) {
            Ok(placed) => {
                outcome.moved += 1;
                outcome.renamed_on_collision += usize::from(placed.renamed);
            }
            Err(failed) => outcome.failures.push(failed.into_row()),
        }
    }

    prune_empty_children(source_root);
    outcome
}
