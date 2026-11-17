use std::path::{Path, PathBuf};

use super::naming::NameRegistry;
use super::safe_fs::{prune_empty_children, transfer_into};
use super::{Operation, Progress};
use crate::Scanner::compare::{content_match, ContentMatch};

#[derive(Debug)]
pub struct PurgatoryOutcome {
    pub planned: usize,
    pub moved: usize,
    pub renamed_on_collision: usize,
    /// Files that could not be moved safely, with the reason. The source is
    /// always left intact for these.
    pub failures: Vec<(PathBuf, String)>,
}

/// Move the given files out of a tree into a side "purgatory" tree, keeping
/// each file's path relative to `source_root` under `purgatory_root`. Every
/// move is copy -> byte-verify -> remove; collisions auto-rename, never
/// overwrite; failures leave the source in place. Afterwards every directory
/// under `source_root` that became empty is removed — `source_root` itself is
/// always kept.
///
/// This moves exactly what it is given. To purge files *because they are
/// duplicates*, use [`purge_proven_duplicates_with_progress`], which proves
/// each one against its keeper first.
pub fn move_to_purgatory_with_progress(
    files: &[PathBuf],
    source_root: &Path,
    purgatory_root: &Path,
    progress: Progress,
) -> PurgatoryOutcome {
    let mut outcome = PurgatoryOutcome::planned(files.len());
    let mut names = NameRegistry::new();

    for (i, path) in files.iter().enumerate() {
        progress(i + 1, files.len());
        move_one(path, source_root, purgatory_root, &mut names, &mut outcome);
    }

    prune_empty_children(source_root);
    outcome
}

/// Purge duplicates, re-proving each one at move time: a `(keeper,
/// duplicate)` pair is moved only if, right now, the duplicate's full
/// content is identical to a keeper that is a different file and still
/// exists. Anything else — a keeper that was moved earlier in the batch, a
/// file that changed since the scan, a file that cannot be fully read — is
/// left in place and reported. Moves behave like
/// [`move_to_purgatory_with_progress`].
pub fn purge_proven_duplicates_with_progress(
    pairs: &[(PathBuf, PathBuf)],
    source_root: &Path,
    purgatory_root: &Path,
    progress: Progress,
) -> PurgatoryOutcome {
    let mut outcome = PurgatoryOutcome::planned(pairs.len());
    let mut names = NameRegistry::new();

    for (i, (keeper, duplicate)) in pairs.iter().enumerate() {
        progress(i + 1, pairs.len());
        match prove_duplicate(keeper, duplicate) {
            Ok(()) => move_one(
                duplicate,
                source_root,
                purgatory_root,
                &mut names,
                &mut outcome,
            ),
            Err(reason) => outcome
                .failures
                .push((duplicate.clone(), format!("kept in place: {reason}"))),
        }
    }

    prune_empty_children(source_root);
    outcome
}

impl PurgatoryOutcome {
    fn planned(planned: usize) -> Self {
        Self {
            planned,
            moved: 0,
            renamed_on_collision: 0,
            failures: Vec::new(),
        }
    }
}

/// `Ok` only when `duplicate` is a distinct file whose full content equals
/// `keeper`'s.
fn prove_duplicate(keeper: &Path, duplicate: &Path) -> Result<(), String> {
    if !keeper.exists() {
        return Err(format!("its keeper is gone: {}", keeper.display()));
    }
    if same_file(keeper, duplicate) {
        return Err(format!(
            "it is the same file on disk as its keeper {}",
            keeper.display()
        ));
    }
    match content_match(keeper, duplicate) {
        ContentMatch::Same => Ok(()),
        ContentMatch::Different(why) => {
            Err(format!("not identical to {} ({why})", keeper.display()))
        }
        ContentMatch::Unknown => Err(format!(
            "could not be fully read or decoded, so it is not proven identical to {}",
            keeper.display()
        )),
    }
}

/// Whether two paths name the same file (same path, a hard link, a symlink,
/// or a case variant on a case-insensitive filesystem). Errs on the side of
/// "same" when that cannot be ruled out.
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(ma), Ok(mb)) => ma.dev() == mb.dev() && ma.ino() == mb.ino(),
            _ => true,
        }
    }
    #[cfg(not(unix))]
    {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(ca), Ok(cb)) => ca == cb,
            _ => true,
        }
    }
}

fn move_one(
    path: &Path,
    source_root: &Path,
    purgatory_root: &Path,
    names: &mut NameRegistry,
    outcome: &mut PurgatoryOutcome,
) {
    let Ok(rel) = path.strip_prefix(source_root) else {
        outcome.failures.push((
            path.to_path_buf(),
            "file is not inside the candidate tree".into(),
        ));
        return;
    };
    let Some(dir) = purgatory_root.join(rel).parent().map(Path::to_path_buf) else {
        outcome
            .failures
            .push((path.to_path_buf(), "target has no parent directory".into()));
        return;
    };

    match transfer_into(path, &dir, names, Operation::Move) {
        Ok(placed) => {
            outcome.moved += 1;
            outcome.renamed_on_collision += usize::from(placed.renamed);
        }
        Err(failed) => outcome.failures.push(failed.into_row()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filemover::test_support::{fixture, lookalike};

    fn no_progress(_: usize, _: usize) {}

    /// `root/a/keeper.jpg` (the A copy) and `root/b/...` (the candidate tree).
    struct Trees {
        _dir: tempfile::TempDir,
        keeper: PathBuf,
        b: PathBuf,
        purgatory: PathBuf,
    }

    fn trees() -> Trees {
        let dir = tempfile::tempdir().unwrap();
        let keeper = dir.path().join("a/keeper.jpg");
        std::fs::create_dir_all(keeper.parent().unwrap()).unwrap();
        std::fs::copy(fixture("jpg-exif-mod/image1.JPG"), &keeper).unwrap();
        let b = dir.path().join("b");
        std::fs::create_dir_all(b.join("2020")).unwrap();
        Trees {
            purgatory: dir.path().join("purgatory"),
            keeper,
            b,
            _dir: dir,
        }
    }

    fn purge(t: &Trees, pairs: &[(PathBuf, PathBuf)]) -> PurgatoryOutcome {
        purge_proven_duplicates_with_progress(pairs, &t.b, &t.purgatory, &no_progress)
    }

    #[test]
    fn a_proven_copy_is_moved() {
        let t = trees();
        let dup = t.b.join("2020/copy.jpg");
        std::fs::copy(&t.keeper, &dup).unwrap();

        let out = purge(&t, &[(t.keeper.clone(), dup.clone())]);

        assert_eq!(out.moved, 1, "{:?}", out.failures);
        assert!(!dup.exists());
        assert!(t.purgatory.join("2020/copy.jpg").exists());
        assert!(t.keeper.exists());
    }

    #[test]
    fn a_metadata_only_variant_is_moved() {
        let t = trees();
        let dup = t.b.join("2020/exif.jpg");
        std::fs::copy(fixture("jpg-exif-mod/image1-exif.JPG"), &dup).unwrap();

        let out = purge(&t, &[(t.keeper.clone(), dup.clone())]);
        assert_eq!(out.moved, 1, "{:?}", out.failures);
    }

    #[test]
    fn a_shallow_lookalike_stays_in_place() {
        let t = trees();
        let dup = t.b.join("2020/lookalike.jpg");
        lookalike(&t.keeper, &dup);

        let out = purge(&t, &[(t.keeper.clone(), dup.clone())]);

        assert_eq!(out.moved, 0);
        assert!(dup.exists());
        assert!(!t.purgatory.exists());
        assert!(
            out.failures[0]
                .1
                .starts_with("kept in place: not identical"),
            "{:?}",
            out.failures
        );
    }

    #[test]
    fn an_unreadable_file_stays_in_place() {
        let t = trees();
        let dup = t.b.join("2020/broken.jpg");
        std::fs::write(&dup, b"not an image").unwrap();

        let out = purge(&t, &[(t.keeper.clone(), dup.clone())]);

        assert_eq!(out.moved, 0);
        assert!(dup.exists());
    }

    #[test]
    fn a_missing_keeper_protects_the_duplicate() {
        let t = trees();
        let dup = t.b.join("2020/copy.jpg");
        std::fs::copy(&t.keeper, &dup).unwrap();
        std::fs::remove_file(&t.keeper).unwrap();

        let out = purge(&t, &[(t.keeper.clone(), dup.clone())]);

        assert_eq!(out.moved, 0);
        assert!(dup.exists(), "the only remaining copy is never purged");
        assert!(
            out.failures[0].1.contains("keeper is gone"),
            "{:?}",
            out.failures
        );
    }

    #[test]
    fn a_file_is_never_purged_as_a_duplicate_of_itself() {
        let t = trees();
        let only = t.b.join("2020/only.jpg");
        std::fs::copy(&t.keeper, &only).unwrap();

        let out = purge(&t, &[(only.clone(), only.clone())]);
        assert_eq!(out.moved, 0);
        assert!(only.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_link_to_the_keeper_is_not_a_duplicate() {
        let t = trees();
        let link = t.b.join("2020/link.jpg");
        std::fs::hard_link(&t.keeper, &link).unwrap();

        let out = purge(&t, &[(t.keeper.clone(), link.clone())]);

        assert_eq!(out.moved, 0);
        assert!(link.exists());
        assert!(
            out.failures[0].1.contains("same file on disk"),
            "{:?}",
            out.failures
        );
    }

    #[test]
    fn a_chain_never_purges_the_last_copy() {
        // Two identical files, each named as the other's duplicate: once the
        // first is purged its partner's keeper is gone, so the partner stays.
        let t = trees();
        let y = t.b.join("2020/y.jpg");
        let z = t.b.join("2020/z.jpg");
        std::fs::copy(&t.keeper, &y).unwrap();
        std::fs::copy(&t.keeper, &z).unwrap();

        let out = purge(&t, &[(y.clone(), z.clone()), (z.clone(), y.clone())]);

        assert_eq!(out.moved, 1, "{:?}", out.failures);
        assert!(
            y.exists() ^ z.exists(),
            "exactly one copy remains in the tree"
        );
    }
}
