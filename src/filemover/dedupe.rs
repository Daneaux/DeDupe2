use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use super::naming::NameRegistry;
use super::safe_fs::relocate_file;
use super::{DuplicateStrategy, FileMoverError, Operation};
use crate::image_reader::{hash_all_bytes, hash_image_data, ReadLimit};

pub fn relocate_tree(src: &Path, dst: &Path, op: Operation) -> Result<(), FileMoverError> {
    for entry in WalkDir::new(src) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(src)
            .map_err(|e| FileMoverError::Other(e.to_string()))?;
        let target = dst.join(rel);
        relocate_file(entry.path(), &target, op)?;
    }
    Ok(())
}

pub fn find_duplicates(
    paths: &[PathBuf],
    strategy: DuplicateStrategy,
) -> Result<Vec<Vec<PathBuf>>, FileMoverError> {
    let mut groups: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for path in paths {
        let hash = hash_for_strategy(path, strategy)?;
        groups.entry(hash).or_default().push(path.clone());
    }

    Ok(groups
        .into_values()
        .filter(|group| group.len() >= 2)
        .collect())
}

pub fn merge_dirs(
    dir_a: &Path,
    dir_b: &Path,
    dst: &Path,
    op: Operation,
    strategy: DuplicateStrategy,
) -> Result<(), FileMoverError> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_shallow(dir_a, &mut files)?;
    collect_shallow(dir_b, &mut files)?;

    let dupes = find_duplicates(&files, strategy)?;

    let mut discarded: HashSet<PathBuf> = HashSet::new();
    for set in &dupes {
        let best = select_best(set)?;
        for path in set {
            if *path != *best {
                discarded.insert(path.clone());
            }
        }
    }

    let mut names = NameRegistry::new();
    for path in &files {
        if discarded.contains(path) {
            continue;
        }
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let unique = names.claim(dst, filename);

        let target = dst.join(unique);
        relocate_file(path, &target, op)?;
    }

    Ok(())
}

fn hash_for_strategy(path: &Path, strategy: DuplicateStrategy) -> Result<u64, FileMoverError> {
    match strategy {
        DuplicateStrategy::ExactHash => Ok(hash_all_bytes(path)?),
        DuplicateStrategy::ImageData => match hash_image_data(path, ReadLimit::All) {
            Ok(hash) => Ok(hash),
            Err(_) => Ok(hash_all_bytes(path)?),
        },
    }
}

fn select_best(paths: &[PathBuf]) -> Result<&PathBuf, FileMoverError> {
    let mut best = &paths[0];
    let mut best_size = std::fs::metadata(best)?.len();
    let mut best_name_len = best.file_name().map(|n| n.len()).unwrap_or(usize::MAX);

    for p in &paths[1..] {
        let size = std::fs::metadata(p)?.len();
        let name_len = p.file_name().map(|n| n.len()).unwrap_or(usize::MAX);
        if size > best_size || (size == best_size && name_len < best_name_len) {
            best = p;
            best_size = size;
            best_name_len = name_len;
        }
    }

    Ok(best)
}

fn collect_shallow(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), FileMoverError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}
