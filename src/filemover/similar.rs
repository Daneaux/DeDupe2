use std::path::{Path, PathBuf};

use super::dates::destination_for;
use super::naming::NameRegistry;
use super::safe_fs::{prune_empty_children, transfer_into, Failed};
use super::{Operation, Progress};

#[derive(Debug)]
pub struct SwapOutcome {
    pub planned: usize,
    pub swapped: usize,
    pub purged: usize,
    /// Pairs that failed, with the reason. Both sides are left in place for
    /// any pair where a step failed.
    pub failures: Vec<(PathBuf, String)>,
}

/// Swap better B copies into A. Two-phase and byte-verified throughout:
///
///   1. move the worse A copy to purgatory (copy -> verify -> remove),
///   2. move the better B copy into the A path it vacated (copy -> verify ->
///      remove).
///
/// If phase 1 fails, the pair is skipped and nothing moved. If phase 2 fails
/// the A copy is safe in purgatory and the B copy is untouched, so nothing is
/// ever lost. Empty folders are pruned afterwards; collisions auto-rename.
pub fn swap_better_copies_with_progress(
    swaps: &[(PathBuf, PathBuf)],
    a_root: &Path,
    b_root: &Path,
    purgatory_root: &Path,
    progress: Progress,
) -> SwapOutcome {
    let mut outcome = SwapOutcome {
        planned: swaps.len(),
        swapped: 0,
        purged: 0,
        failures: Vec::new(),
    };
    let total = swaps.len() * 2;
    let mut names = NameRegistry::new();

    // Phase 1: purge the worse A copies to purgatory (B = the A-tree root for
    // relative structure).
    let mut purge_ok: Vec<bool> = vec![false; swaps.len()];
    for (i, (a_worse, _)) in swaps.iter().enumerate() {
        progress(i + 1, total);
        let dir = purgatory_dir(a_worse, a_root, purgatory_root);
        match move_into(a_worse, &dir, &mut names) {
            Ok(_) => {
                purge_ok[i] = true;
                outcome.purged += 1;
            }
            Err(row) => outcome.failures.push(row),
        }
    }

    // Phase 2: move the better B copies into the vacated A paths.
    for (i, (a_worse, b_better)) in swaps.iter().enumerate() {
        progress(swaps.len() + i + 1, total);
        if !purge_ok[i] {
            continue;
        }
        let Some(dir) = a_worse.parent().map(|p| p.to_path_buf()) else {
            outcome
                .failures
                .push((b_better.clone(), "A path has no parent directory".into()));
            continue;
        };
        match move_into(b_better, &dir, &mut names) {
            Ok(_) => outcome.swapped += 1,
            Err((path, e)) => outcome
                .failures
                .push((path, format!("{e} (the worse A copy is safe in purgatory)"))),
        }
    }

    // Empty folders left behind are pruned (roots themselves are kept).
    for root in [a_root, b_root] {
        prune_empty_children(root);
    }

    outcome
}

/// What to do with one similar pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimilarDecision {
    /// Keep both: B is renamed-in via unique naming and moved into A at its
    /// EXIF-date destination (format + A root); A stays where it is.
    KeepBoth,
    /// Keep neither in place: both copies move to purgatory.
    PurgatoryBoth,
    /// Apply the suggestion: the bigger copy stays in A. If B is bigger the
    /// pair is swapped (A -> purgatory, B -> the vacated A path); otherwise
    /// B goes to purgatory.
    TakeBigger,
}

#[derive(Debug)]
pub struct DecisionOutcome {
    pub planned: usize,
    pub kept_both: usize,
    pub purged_both: usize,
    pub bigger_taken: usize,
    pub failures: Vec<(PathBuf, String)>,
}

/// Execute per-row decisions from the similar table. Every file move is a
/// verified copy -> remove; failures leave both copies where they are.
pub fn apply_similar_decisions_with_progress(
    decisions: &[(PathBuf, PathBuf, SimilarDecision)],
    a_root: &Path,
    b_root: &Path,
    purgatory_root: &Path,
    format: &str,
    progress: Progress,
) -> DecisionOutcome {
    let mut outcome = DecisionOutcome {
        planned: decisions.len(),
        kept_both: 0,
        purged_both: 0,
        bigger_taken: 0,
        failures: Vec::new(),
    };
    let mut names = NameRegistry::new();
    let total = decisions.len();

    for (i, (a_path, b_path, decision)) in decisions.iter().enumerate() {
        progress(i + 1, total);

        let purgatory_for = |path: &Path, root: &Path| purgatory_dir(path, root, purgatory_root);

        match decision {
            SimilarDecision::KeepBoth => {
                let date = crate::exif::creation_date(b_path);
                let Some(destination) = destination_for(b_path, &date, a_root, format) else {
                    outcome.failures.push((
                        b_path.clone(),
                        "no EXIF date: cannot compute a destination for the kept copy".into(),
                    ));
                    continue;
                };
                let dir = destination
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| a_root.to_path_buf());
                match move_into(b_path, &dir, &mut names) {
                    Ok(_) => outcome.kept_both += 1,
                    Err(row) => outcome.failures.push(row),
                }
            }
            SimilarDecision::PurgatoryBoth => {
                let mut ok = true;
                match move_into(a_path, &purgatory_for(a_path, a_root), &mut names) {
                    Ok(_) => {}
                    Err(row) => {
                        outcome.failures.push(row);
                        ok = false;
                    }
                }
                if ok {
                    match move_into(b_path, &purgatory_for(b_path, b_root), &mut names) {
                        Ok(_) => outcome.purged_both += 1,
                        Err(row) => outcome.failures.push(row),
                    }
                }
            }
            SimilarDecision::TakeBigger => {
                let size_a = std::fs::metadata(a_path).map(|m| m.len()).unwrap_or(0);
                let size_b = std::fs::metadata(b_path).map(|m| m.len()).unwrap_or(0);
                if size_b > size_a {
                    // Swap: A (smaller) to purgatory, then B into its place.
                    match move_into(a_path, &purgatory_for(a_path, a_root), &mut names) {
                        Ok(_) => {
                            let dir = a_path
                                .parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| a_root.to_path_buf());
                            match move_into(b_path, &dir, &mut names) {
                                Ok(_) => outcome.bigger_taken += 1,
                                Err((path, e)) => outcome.failures.push((
                                    path,
                                    format!("{e} (the smaller A copy is safe in purgatory)"),
                                )),
                            }
                        }
                        Err(row) => outcome.failures.push(row),
                    }
                } else {
                    // A already holds the bigger copy: B goes to purgatory.
                    match move_into(b_path, &purgatory_for(b_path, b_root), &mut names) {
                        Ok(_) => outcome.bigger_taken += 1,
                        Err(row) => outcome.failures.push(row),
                    }
                }
            }
        }
    }

    for root in [a_root, b_root] {
        prune_empty_children(root);
    }

    outcome
}

/// Where a file from `root` lands in purgatory: the same relative folder
/// under `purgatory_root`, or its top level for files outside `root`.
fn purgatory_dir(path: &Path, root: &Path, purgatory_root: &Path) -> PathBuf {
    path.strip_prefix(root)
        .ok()
        .and_then(|rel| purgatory_root.join(rel).parent().map(Path::to_path_buf))
        .unwrap_or_else(|| purgatory_root.to_path_buf())
}

/// Verified move of `src` into `dir` (collision-safe), with a failure as an
/// outcome row.
fn move_into(src: &Path, dir: &Path, names: &mut NameRegistry) -> Result<(), (PathBuf, String)> {
    transfer_into(src, dir, names, Operation::Move)
        .map(drop)
        .map_err(Failed::into_row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purgatory_dir_mirrors_the_relative_folder() {
        let purgatory = Path::new("/p");
        assert_eq!(
            purgatory_dir(Path::new("/a/2020/x.jpg"), Path::new("/a"), purgatory),
            Path::new("/p/2020")
        );
        assert_eq!(
            purgatory_dir(Path::new("/a/x.jpg"), Path::new("/a"), purgatory),
            Path::new("/p")
        );
        assert_eq!(
            purgatory_dir(Path::new("/elsewhere/x.jpg"), Path::new("/a"), purgatory),
            Path::new("/p"),
            "files outside the root land at the purgatory top level"
        );
    }
}
