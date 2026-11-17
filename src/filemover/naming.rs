//! Collision-free file naming. Names are compared lowercased because macOS
//! and Windows filesystems are case-insensitive: `IMG_0500.jpg` and
//! `IMG_0500.JPG` are the same file there.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Tracks every name already present (on disk) or reserved (earlier in this
/// run) per destination directory, so a batch never hands out the same name
/// twice and never targets a file that already exists.
#[derive(Debug, Default)]
pub(super) struct NameRegistry {
    taken: HashMap<PathBuf, HashSet<String>>,
}

impl NameRegistry {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Reserve a name for `filename` inside `dir`: the name itself when it is
    /// free, otherwise `stem (n).ext` with the smallest free `n`. The
    /// directory is read from disk the first time it is seen.
    pub(super) fn claim(&mut self, dir: &Path, filename: &str) -> String {
        let names = self
            .taken
            .entry(dir.to_path_buf())
            .or_insert_with(|| existing_names(dir));
        let unique = unique_name(filename, names);
        names.insert(unique.to_lowercase());
        unique
    }
}

/// Length of a path's final component (0 when there is none).
pub(super) fn name_len(path: &Path) -> usize {
    path.file_name().map(|n| n.len()).unwrap_or(0)
}

/// A path's final component as UTF-8, or `"file"` when it has none.
pub(super) fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string()
}

/// Filenames currently present in `dir`, **lowercased** (empty when the
/// directory does not exist yet).
pub(super) fn existing_names(dir: &Path) -> HashSet<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_lowercase))
        .collect()
}

/// `filename` if no lowercased entry of `taken` matches it, otherwise the
/// first free `stem (n).ext`.
pub(super) fn unique_name(filename: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(&filename.to_lowercase()) {
        return filename.to_string();
    }

    let (stem, ext) = match filename.rfind('.') {
        Some(idx) => filename.split_at(idx),
        None => (filename, ""),
    };

    (1..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .expect("an unbounded range always yields a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_name_never_reuses_existing_names_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("IMG_0001.jpg"), b"x").unwrap();
        std::fs::write(dir.path().join("IMG_0001 (1).JPG"), b"y").unwrap();

        let taken = existing_names(dir.path());
        assert_eq!(unique_name("img_0001.jpg", &taken), "img_0001 (2).jpg");
        assert_eq!(unique_name("other.jpg", &taken), "other.jpg");
    }

    #[test]
    fn unique_name_handles_files_without_an_extension() {
        let taken: HashSet<String> = HashSet::from(["photo".to_string()]);
        assert_eq!(unique_name("photo", &taken), "photo (1)");
    }

    #[test]
    fn existing_names_of_a_missing_directory_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(existing_names(&dir.path().join("nope")).is_empty());
    }

    #[test]
    fn registry_seeds_from_disk_and_reserves_within_a_run() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), b"x").unwrap();

        let mut names = NameRegistry::new();
        assert_eq!(
            names.claim(dir.path(), "A.JPG"),
            "A (1).JPG",
            "on-disk name is taken"
        );
        assert_eq!(
            names.claim(dir.path(), "a.jpg"),
            "a (2).jpg",
            "earlier claim is taken"
        );
        assert_eq!(names.claim(dir.path(), "b.jpg"), "b.jpg");
        assert_eq!(names.claim(dir.path(), "b.jpg"), "b (1).jpg");
    }

    #[test]
    fn registry_tracks_directories_independently() {
        let root = tempfile::tempdir().unwrap();
        let (one, two) = (root.path().join("one"), root.path().join("two"));

        let mut names = NameRegistry::new();
        assert_eq!(names.claim(&one, "x.jpg"), "x.jpg");
        assert_eq!(names.claim(&two, "x.jpg"), "x.jpg");
        assert_eq!(names.claim(&one, "x.jpg"), "x (1).jpg");
    }

    #[test]
    fn file_name_falls_back_for_paths_without_a_name() {
        assert_eq!(file_name(Path::new("/a/b.jpg")), "b.jpg");
        assert_eq!(file_name(Path::new("/")), "file");
        assert_eq!(name_len(Path::new("/a/abc")), 3);
        assert_eq!(name_len(Path::new("/")), 0);
    }
}
