use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::dates::{destination_for, extract_event, parse_date};
use super::naming::{file_name, NameRegistry};
use super::safe_fs::relocate_file;
use super::{FileMoverError, Operation, Progress};
use crate::exif::CreationDate;
use crate::image_reader::collect_image_files;
use crate::Scanner::compare::{deep_scan_pairs_with_progress, DupPair};
use crate::Scanner::scan::hash_image_data_parallel;

/// `files` are `(source, capture date)` pairs: scanning deliberately does
/// not extract dates, so callers that organize by date resolve them in a
/// separate pass.
pub fn organize_by_date(
    files: &[(PathBuf, CreationDate)],
    dst_root: &Path,
    source_name: &str,
    op: Operation,
) -> Result<(), FileMoverError> {
    let event = extract_event(source_name);
    let mut names = NameRegistry::new();

    for (path, date) in files {
        let folder = match parse_date(date) {
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

        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let unique = names.claim(&folder, filename);
        let target = folder.join(unique);
        relocate_file(path, &target, op)?;
    }

    Ok(())
}

#[derive(Debug)]
pub struct OrganizeOutcome {
    pub planned: usize,
    pub moved: usize,
    pub skipped_no_date: usize,
    /// Sources rejected because they already exist in the destination folder,
    /// paired with the existing file they matched.
    pub rejected_duplicates: Vec<(PathBuf, PathBuf)>,
    /// Files that could not be moved, with the reason. Sources are untouched.
    pub failures: Vec<(PathBuf, String)>,
}

/// Organize dated source files into a destination tree, checking for
/// duplicates only where it matters: the exact destination folders the files
/// are heading to (the destination tree is not scanned up front). Existing
/// files there are shallow-hashed (64kb image data); matches are then
/// deep-verified automatically (full content), and only confirmed duplicates
/// are rejected from the move. Everything else is moved copy-verified in the
/// usual collision-safe way, with failures left in place.
pub fn organize_into_destination_with_progress(
    dated: &[(PathBuf, CreationDate)],
    destination_root: &Path,
    format: &str,
    op: Operation,
    progress: Progress,
) -> OrganizeOutcome {
    let mut outcome = OrganizeOutcome {
        planned: dated.len(),
        moved: 0,
        skipped_no_date: 0,
        rejected_duplicates: Vec::new(),
        failures: Vec::new(),
    };

    // 1. Resolve each file's target folder; group by folder.
    let mut target_of: Vec<Option<PathBuf>> = vec![None; dated.len()];
    let mut by_dir: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (i, (path, date)) in dated.iter().enumerate() {
        match destination_for(path, date, destination_root, format) {
            Some(target) => {
                let dir = target
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| destination_root.to_path_buf());
                by_dir.entry(dir).or_default().push(i);
                target_of[i] = Some(target);
            }
            None => outcome.skipped_no_date += 1,
        }
    }

    // 2. Shallow hash every source file (one pass).
    let sources: Vec<PathBuf> = dated.iter().map(|(p, _)| p.clone()).collect();
    let source_hashes = hash_image_data_parallel(&sources, 64 * 1024, progress);

    // 3. For each target folder that exists, shallow-hash its files and form
    // source <-> existing candidate pairs on equal 64kb hashes.
    let mut pairs: Vec<DupPair> = Vec::new();
    let mut pair_match: HashMap<PathBuf, PathBuf> = HashMap::new(); // source -> existing
    for (dir, idxs) in &by_dir {
        if !dir.is_dir() {
            continue;
        }
        let existing = match collect_image_files(dir) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if existing.is_empty() {
            continue;
        }
        let existing_hashes = hash_image_data_parallel(&existing, 64 * 1024, progress);
        let mut by_hash: HashMap<u64, &PathBuf> = HashMap::new();
        for (hash, path) in existing_hashes.iter().zip(&existing) {
            by_hash.entry(hash.hash).or_insert(path);
        }
        for &i in idxs {
            if let Some(existing_path) = by_hash.get(&source_hashes[i].hash) {
                let source = dated[i].0.clone();
                pairs.push(DupPair {
                    a: (*existing_path).clone(),
                    b: source.clone(),
                });
                pair_match.entry(source).or_insert((*existing_path).clone());
            }
        }
    }

    // 4. Deep-scan the candidates automatically: only confirmed duplicates
    // are rejected; shallow-only matches still move.
    let mut rejected: HashSet<PathBuf> = HashSet::new();
    if !pairs.is_empty() {
        let scan = deep_scan_pairs_with_progress(&pairs, progress);
        for source in &scan.confirmed_b {
            if rejected.insert(source.clone()) {
                if let Some(existing) = pair_match.get(source) {
                    outcome
                        .rejected_duplicates
                        .push((source.clone(), existing.clone()));
                }
            }
        }
    }

    // 5. Move the rest, collision-safe, collecting failures per file.
    let mut names = NameRegistry::new();
    let total = dated.len();
    for (i, (path, _)) in dated.iter().enumerate() {
        progress(i + 1, total);
        let Some(target) = &target_of[i] else {
            continue;
        };
        if rejected.contains(path) {
            continue;
        }
        let dir = target
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| destination_root.to_path_buf());
        let final_target = dir.join(names.claim(&dir, &file_name(path)));

        if let Err(e) = relocate_file(path, &final_target, op) {
            outcome.failures.push((path.clone(), e.to_string()));
        } else {
            outcome.moved += 1;
        }
    }

    outcome
}
