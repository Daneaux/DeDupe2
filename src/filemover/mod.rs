use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::Scanner::scanner::ScannedFile;
use crate::exif::CreationDate;
use crate::image_reader::{hash_all_bytes, hash_image_data, ReadLimit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Move,
    Copy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateStrategy {
    ExactHash,
    ImageData,
}

#[derive(Debug)]
pub enum FileMoverError {
    Io(std::io::Error),
    Other(String),
}

impl fmt::Display for FileMoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileMoverError::Io(e) => write!(f, "io error: {e}"),
            FileMoverError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for FileMoverError {}

impl From<std::io::Error> for FileMoverError {
    fn from(e: std::io::Error) -> Self {
        FileMoverError::Io(e)
    }
}

impl From<walkdir::Error> for FileMoverError {
    fn from(e: walkdir::Error) -> Self {
        FileMoverError::Other(e.to_string())
    }
}

impl From<crate::image_reader::ImageReaderError> for FileMoverError {
    fn from(e: crate::image_reader::ImageReaderError) -> Self {
        FileMoverError::Other(e.to_string())
    }
}

#[derive(Debug)]
pub struct MergeOutcome {
    pub kept: Vec<PathBuf>,
    pub purged: Vec<PathBuf>,
}

pub fn merge_libraries(
    _sources: &[PathBuf],
    _destination: &Path,
    _purgatory: &Path,
    _op: Operation,
) -> Result<MergeOutcome, FileMoverError> {
    todo!("implement merge_libraries")
}

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

pub fn organize_by_date(
    files: &[ScannedFile],
    dst_root: &Path,
    source_name: &str,
    op: Operation,
) -> Result<(), FileMoverError> {
    let event = extract_event(source_name);
    let mut taken: HashSet<String> = HashSet::new();

    for file in files {
        let folder = match parse_date(&file.creation_date) {
            Some((year, month, day)) => {
                let name = if event.is_empty() {
                    format!("{month:02}-{day:02}")
                } else {
                    format!("{month:02}-{day:02}-{event}")
                };
                dst_root.join(format!("{year:04}")).join(name)
            }
            None => dst_root.join("unknown"),
        };

        let filename = file
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        let unique = unique_name(filename, &taken);
        taken.insert(unique.clone());

        let target = folder.join(unique);
        relocate_file(&file.path, &target, op)?;
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

    let mut taken: HashSet<String> = HashSet::new();
    for path in &files {
        if discarded.contains(path) {
            continue;
        }
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let unique = unique_name(filename, &taken);
        taken.insert(unique.clone());

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

fn relocate_file(src: &Path, dst: &Path, op: Operation) -> Result<(), FileMoverError> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match op {
        Operation::Move => move_file(src, dst)?,
        Operation::Copy => {
            std::fs::copy(src, dst)?;
        }
    }
    Ok(())
}

fn move_file(src: &Path, dst: &Path) -> Result<(), FileMoverError> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dst)?;
            std::fs::remove_file(src)?;
            Ok(())
        }
    }
}

fn parse_date(date: &CreationDate) -> Option<(u32, u32, u32)> {
    let s = match date {
        CreationDate::DateCreated(v) => v,
        CreationDate::Unknown => return None,
    };

    let digits: Vec<u32> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;

    if digits.len() < 3 {
        return None;
    }
    let (year, month, day) = (digits[0], digits[1], digits[2]);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

fn extract_event(source_name: &str) -> String {
    let mut words: Vec<&str> = source_name
        .split(|c: char| c == ' ' || c == '-' || c == '_')
        .filter(|w| !w.is_empty())
        .collect();

    while let Some(first) = words.first() {
        if is_date_token(first) {
            words.remove(0);
        } else {
            break;
        }
    }

    words.join("-")
}

fn is_date_token(word: &str) -> bool {
    if word.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    const MONTHS: [&str; 24] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
        "january", "february", "march", "april", "may", "june", "july", "august", "september",
        "october", "november", "december",
    ];
    MONTHS.contains(&word.to_lowercase().as_str())
}

fn unique_name(filename: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(filename) {
        return filename.to_string();
    }

    let (stem, ext) = match filename.rfind('.') {
        Some(idx) => (&filename[..idx], &filename[idx..]),
        None => (filename, ""),
    };

    let mut n = 1;
    loop {
        let candidate = format!("{stem} ({n}){ext}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}
