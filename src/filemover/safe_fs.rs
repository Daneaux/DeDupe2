//! The only code in the crate that copies, renames or removes media files.
//!
//! Guarantees, for every function here:
//! - a destination that already exists is never written to or removed;
//! - a source is removed only after its copy is confirmed byte-identical
//!   (same length and same full-file hash);
//! - any failure before that point leaves the source untouched and removes
//!   only the partial copy this call created.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use super::naming::{file_name, NameRegistry};
use super::{FileMoverError, Operation};
use crate::image_reader::hash_all_bytes;

/// Why a verified transfer did not complete. For every variant except
/// [`TransferError::RemoveSource`] the source is intact and no destination
/// file was left behind.
#[derive(Debug)]
pub enum TransferError {
    SourceUnreadable(io::Error),
    SourceHash(String),
    CreateDir(io::Error),
    TargetExists(PathBuf),
    Copy(io::Error),
    VerifyMismatch,
    /// The copy is verified but the source could not be removed: both
    /// copies now exist.
    RemoveSource(io::Error),
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceUnreadable(e) => write!(f, "source not readable: {e}"),
            Self::SourceHash(e) => write!(f, "source hash failed: {e}"),
            Self::CreateDir(e) => write!(f, "cannot create destination folder: {e}"),
            Self::TargetExists(p) => write!(f, "destination already exists: {}", p.display()),
            Self::Copy(e) => write!(f, "copy failed: {e}"),
            Self::VerifyMismatch => write!(f, "verification failed: destination bytes differ"),
            Self::RemoveSource(e) => write!(
                f,
                "copied and verified, but source removal failed (both copies exist): {e}"
            ),
        }
    }
}

impl std::error::Error for TransferError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SourceUnreadable(e)
            | Self::CreateDir(e)
            | Self::Copy(e)
            | Self::RemoveSource(e) => Some(e),
            _ => None,
        }
    }
}

/// A transfer that landed: where the file now lives and whether its name had
/// to change to avoid a collision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Placed {
    pub target: PathBuf,
    pub renamed: bool,
}

/// A transfer that failed, reported against the path the user should look
/// at: the destination when both copies exist, otherwise the source.
#[derive(Debug)]
pub(super) struct Failed {
    pub path: PathBuf,
    pub error: TransferError,
}

impl Failed {
    fn new(src: &Path, target: &Path, error: TransferError) -> Self {
        let path = match error {
            TransferError::RemoveSource(_) => target,
            _ => src,
        };
        Self {
            path: path.to_path_buf(),
            error,
        }
    }

    /// `(path, message)` as the public outcome structs record failures.
    pub fn into_row(self) -> (PathBuf, String) {
        (self.path, self.error.to_string())
    }
}

/// Copy `src` to `target`, which must not exist, then confirm the copy is
/// byte-identical. Missing parent directories are created. On any failure
/// the partial copy is removed and the source is untouched.
pub(super) fn verified_copy(src: &Path, target: &Path) -> Result<(), TransferError> {
    let expected_len = std::fs::metadata(src)
        .map_err(TransferError::SourceUnreadable)?
        .len();
    let expected_hash =
        hash_all_bytes(src).map_err(|e| TransferError::SourceHash(e.to_string()))?;

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(TransferError::CreateDir)?;
    }
    // Checked last so a destination created by an earlier step of the same
    // batch is still caught. Also covers `src == target`, where `fs::copy`
    // would truncate the only copy.
    if exists(target) {
        return Err(TransferError::TargetExists(target.to_path_buf()));
    }

    if let Err(e) = std::fs::copy(src, target) {
        let _ = std::fs::remove_file(target);
        return Err(TransferError::Copy(e));
    }
    if !is_identical(target, expected_len, expected_hash) {
        let _ = std::fs::remove_file(target);
        return Err(TransferError::VerifyMismatch);
    }
    Ok(())
}

/// [`verified_copy`], then for [`Operation::Move`] remove the source.
pub(super) fn verified_transfer(
    src: &Path,
    target: &Path,
    op: Operation,
) -> Result<(), TransferError> {
    verified_copy(src, target)?;
    if op == Operation::Move {
        std::fs::remove_file(src).map_err(TransferError::RemoveSource)?;
    }
    Ok(())
}

/// Transfer `src` into `dir` under a name reserved from `names` (its own
/// name unless that collides).
pub(super) fn transfer_into(
    src: &Path,
    dir: &Path,
    names: &mut NameRegistry,
    op: Operation,
) -> Result<Placed, Failed> {
    let original = file_name(src);
    let unique = names.claim(dir, &original);
    let target = dir.join(&unique);
    match verified_transfer(src, &target, op) {
        Ok(()) => Ok(Placed {
            target,
            renamed: unique != original,
        }),
        Err(error) => Err(Failed::new(src, &target, error)),
    }
}

/// Move or copy `src` to the exact path `dst`. A move is a rename when
/// possible (same volume: instant, atomic) and a verified copy + remove
/// otherwise; a copy is always verified. `dst` must not exist.
pub(super) fn relocate_file(src: &Path, dst: &Path, op: Operation) -> Result<(), FileMoverError> {
    if op == Operation::Copy {
        return Ok(verified_copy(src, dst)?);
    }

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // `rename` silently replaces an existing destination on Unix.
    if exists(dst) {
        return Err(TransferError::TargetExists(dst.to_path_buf()).into());
    }
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        // Typically a cross-volume move.
        Err(_) => Ok(verified_transfer(src, dst, Operation::Move)?),
    }
}

/// Remove every empty directory below `dir`, bottom-up. `dir` itself is
/// kept.
pub(super) fn prune_empty_children(dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            prune_empty_children(&path);
            let _ = std::fs::remove_dir(&path);
        }
    }
}

/// Remove `dir` and its descendants if they hold no files.
pub(super) fn remove_if_empty(dir: &Path) {
    prune_empty_children(dir);
    let _ = std::fs::remove_dir(dir);
}

/// Whether anything (file, directory or dangling symlink) sits at `path`.
fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn is_identical(path: &Path, expected_len: u64, expected_hash: u64) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() == expected_len)
        && hash_all_bytes(path).is_ok_and(|h| h == expected_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn copy_creates_parents_and_keeps_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.jpg");
        let dst = dir.path().join("deep/nested/dst.jpg");
        write(&src, b"pixels");

        verified_copy(&src, &dst).unwrap();

        assert_eq!(std::fs::read(&src).unwrap(), b"pixels");
        assert_eq!(std::fs::read(&dst).unwrap(), b"pixels");
    }

    #[test]
    fn move_removes_the_source_only_after_copying() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.jpg");
        let dst = dir.path().join("out/dst.jpg");
        write(&src, b"pixels");

        verified_transfer(&src, &dst, Operation::Move).unwrap();

        assert!(!src.exists());
        assert_eq!(std::fs::read(&dst).unwrap(), b"pixels");
    }

    #[test]
    fn an_existing_destination_is_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.jpg");
        let dst = dir.path().join("dst.jpg");
        write(&src, b"new");
        write(&dst, b"precious");

        for op in [Operation::Copy, Operation::Move] {
            let err = verified_transfer(&src, &dst, op).unwrap_err();
            assert!(matches!(err, TransferError::TargetExists(_)), "{err}");
            assert_eq!(std::fs::read(&dst).unwrap(), b"precious");
            assert_eq!(std::fs::read(&src).unwrap(), b"new");
        }
    }

    #[test]
    fn transferring_a_file_onto_itself_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("only.jpg");
        write(&src, b"only copy");

        let err = verified_transfer(&src, &src, Operation::Move).unwrap_err();
        assert!(matches!(err, TransferError::TargetExists(_)));
        assert_eq!(std::fs::read(&src).unwrap(), b"only copy");

        let err = relocate_file(&src, &src, Operation::Move).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(std::fs::read(&src).unwrap(), b"only copy");
    }

    #[test]
    fn a_missing_source_fails_without_creating_anything() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("out/dst.jpg");

        let err =
            verified_transfer(&dir.path().join("gone.jpg"), &dst, Operation::Move).unwrap_err();
        assert!(matches!(err, TransferError::SourceUnreadable(_)));
        assert!(
            !dst.parent().unwrap().exists(),
            "no folder created for a missing source"
        );
    }

    #[test]
    fn identity_check_detects_length_and_content_differences() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        write(&file, b"abcd");
        let hash = hash_all_bytes(&file).unwrap();

        assert!(is_identical(&file, 4, hash));
        assert!(!is_identical(&file, 5, hash), "length mismatch");
        assert!(!is_identical(&file, 4, hash ^ 1), "content mismatch");
        assert!(!is_identical(&dir.path().join("missing"), 4, hash));
    }

    #[test]
    fn transfer_into_renames_on_collision_and_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        write(&out.join("IMG_1.jpg"), b"existing");
        write(&dir.path().join("a/IMG_1.jpg"), b"first");
        write(&dir.path().join("b/img_1.JPG"), b"second");
        write(&dir.path().join("c/fresh.jpg"), b"third");

        let mut names = NameRegistry::new();
        let first = transfer_into(
            &dir.path().join("a/IMG_1.jpg"),
            &out,
            &mut names,
            Operation::Move,
        )
        .unwrap();
        let second = transfer_into(
            &dir.path().join("b/img_1.JPG"),
            &out,
            &mut names,
            Operation::Move,
        )
        .unwrap();
        let third = transfer_into(
            &dir.path().join("c/fresh.jpg"),
            &out,
            &mut names,
            Operation::Copy,
        )
        .unwrap();

        assert_eq!(first.target, out.join("IMG_1 (1).jpg"));
        assert!(first.renamed);
        assert_eq!(second.target, out.join("img_1 (2).JPG"));
        assert!(!third.renamed);
        assert_eq!(std::fs::read(out.join("IMG_1.jpg")).unwrap(), b"existing");
        assert_eq!(std::fs::read(&first.target).unwrap(), b"first");
        assert_eq!(std::fs::read(&second.target).unwrap(), b"second");
        assert!(
            dir.path().join("c/fresh.jpg").exists(),
            "copy keeps the source"
        );
    }

    #[test]
    fn failures_are_reported_against_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.jpg");
        let mut names = NameRegistry::new();

        let failed = transfer_into(&missing, dir.path(), &mut names, Operation::Move).unwrap_err();
        let (path, message) = failed.into_row();
        assert_eq!(path, missing);
        assert!(message.starts_with("source not readable"), "{message}");
    }

    #[test]
    fn removal_failures_point_at_the_surviving_destination() {
        let src = Path::new("/src.jpg");
        let dst = Path::new("/dst.jpg");
        let removal = io::Error::new(io::ErrorKind::PermissionDenied, "denied");

        let failed = Failed::new(src, dst, TransferError::RemoveSource(removal));
        assert_eq!(failed.path, dst);
        assert!(failed.error.to_string().contains("both copies exist"));

        let failed = Failed::new(src, dst, TransferError::VerifyMismatch);
        assert_eq!(failed.path, src);
    }

    #[test]
    fn relocate_renames_within_a_volume_and_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jpg");
        let dst = dir.path().join("sub/a.jpg");
        write(&src, b"a");

        relocate_file(&src, &dst, Operation::Move).unwrap();
        assert!(!src.exists());
        assert_eq!(std::fs::read(&dst).unwrap(), b"a");

        write(&src, b"other");
        assert!(relocate_file(&src, &dst, Operation::Move).is_err());
        assert!(relocate_file(&src, &dst, Operation::Copy).is_err());
        assert_eq!(std::fs::read(&dst).unwrap(), b"a");
        assert_eq!(std::fs::read(&src).unwrap(), b"other");
    }

    #[test]
    fn prune_keeps_the_root_and_non_empty_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("empty/deeper")).unwrap();
        write(&dir.path().join("full/keep.jpg"), b"x");

        prune_empty_children(dir.path());

        assert!(dir.path().exists());
        assert!(!dir.path().join("empty").exists());
        assert!(dir.path().join("full/keep.jpg").exists());

        remove_if_empty(&dir.path().join("full"));
        assert!(
            dir.path().join("full").exists(),
            "a folder with files is kept"
        );
    }
}
