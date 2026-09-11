use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::Scanner::fastScan::hash_image_data_parallel;
use crate::exif::{creation_date, CreationDate};
use crate::image_reader::is_supported_image;

#[derive(Debug)]
pub struct Comparison {
    pub a: PathBuf,
    pub b: PathBuf,
    pub duplicates: Vec<DuplicateGroup>,
    pub a_only: Vec<PathBuf>,
    pub b_only: Vec<PathBuf>,
    /// Candidate-tree (B) files whose decoder failed or panicked. They are
    /// not classified as duplicates or originals; they are listed for
    /// review. Unreadable files in the destination tree are ignored.
    pub unreadable: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct DuplicateGroup {
    pub a: Vec<PathBuf>,
    pub b: Vec<PathBuf>,
}

/// An original from tree B after an EXIF scan.
#[derive(Debug)]
pub struct DatedOriginal {
    pub path: PathBuf,
    pub creation_date: CreationDate,
}

pub fn compare_folders(a: &Path, b: &Path) -> Result<Comparison, Box<dyn std::error::Error>> {
    compare_folders_with_progress(a, b, &|_, _| {})
}

/// Fast scan: 64kb encoded-image-data hash only. EXIF is a separate,
/// explicit pass (see `scan_exif_with_progress`).
pub fn compare_folders_with_progress(
    a: &Path,
    b: &Path,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<Comparison, Box<dyn std::error::Error>> {
    let a_files = collect_image_files(a)?;
    let b_files = collect_image_files(b)?;

    let a_len = a_files.len();
    let mut all = a_files.clone();
    all.extend(b_files.clone());

    let hashes = hash_image_data_parallel(&all, 64 * 1024, progress);

    let mut map: HashMap<u64, (Vec<PathBuf>, Vec<PathBuf>)> = HashMap::new();
    let mut unreadable = Vec::new();
    for (i, path) in all.iter().enumerate() {
        if !hashes[i].decoded {
            // Only candidate-tree (B) files are surfaced as unreadable; an
            // unreadable file in the destination library is simply ignored.
            if i >= a_len {
                unreadable.push(path.clone());
            }
            continue;
        }
        let entry = map.entry(hashes[i].hash).or_default();
        if i < a_len {
            entry.0.push(path.clone());
        } else {
            entry.1.push(path.clone());
        }
    }

    let mut duplicates = Vec::new();
    let mut a_only = Vec::new();
    let mut b_only = Vec::new();

    let mut groups: Vec<(Vec<PathBuf>, Vec<PathBuf>)> = map.into_values().collect();
    groups.sort_by_key(|(a, b)| {
        a.first()
            .cloned()
            .or_else(|| b.first().cloned())
            .unwrap_or_default()
    });

    for (a_list, b_list) in groups {
        if !a_list.is_empty() && !b_list.is_empty() {
            duplicates.push(DuplicateGroup {
                a: a_list,
                b: b_list,
            });
        } else if !b_list.is_empty() {
            b_only.extend(b_list);
        } else {
            a_only.extend(a_list);
        }
    }

    a_only.sort();
    b_only.sort();
    unreadable.sort();

    Ok(Comparison {
        a: a.to_path_buf(),
        b: b.to_path_buf(),
        duplicates,
        a_only,
        b_only,
        unreadable,
    })
}

/// Resolve EXIF capture dates (DateTimeOriginal, then CreateDate, else
/// Unknown) for the given paths, in parallel.
pub fn scan_exif(paths: &[PathBuf]) -> Vec<DatedOriginal> {
    scan_exif_with_progress(paths, &|_, _| {})
}

pub fn scan_exif_with_progress(
    paths: &[PathBuf],
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<DatedOriginal> {
    let total = paths.len();
    let done = AtomicUsize::new(0);

    paths
        .par_iter()
        .map(|path| {
            let creation_date = creation_date(path);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            DatedOriginal {
                path: path.clone(),
                creation_date,
            }
        })
        .collect()
}

fn collect_image_files(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry?;
        if entry.file_type().is_file() && is_supported_image(entry.path()) {
            out.push(entry.into_path());
        }
    }
    out.sort();
    Ok(out)
}

use std::collections::HashSet;

/// Per-candidate-tree counts against the destination set A and the other
/// candidate trees.
#[derive(Debug)]
pub struct TreeCounts {
    pub root: PathBuf,
    /// All supported media files found in the tree.
    pub total: usize,
    /// Files whose 64kb image-data hash exists in A.
    pub duplicates: usize,
    /// Files not in A and not in any *other* candidate tree either.
    pub unique_to_tree: usize,
    /// Files not in A but present in another candidate tree as well.
    pub shared_with_candidates: usize,
    /// Files that could not be decoded (excluded from the other counts).
    pub unreadable: usize,
}

/// Set differences between one destination tree and N candidate trees,
/// reported as counts only.
#[derive(Debug)]
pub struct SetComparison {
    pub a: PathBuf,
    /// All supported media files found in A.
    pub a_total: usize,
    /// A files not present in any candidate tree.
    pub a_unique: usize,
    /// A files that could not be decoded.
    pub a_unreadable: usize,
    /// Candidate files whose hash exists in A.
    pub duplicates: usize,
    /// Candidate files not present in A (the B+C-only set).
    pub candidates_only: usize,
    /// Per-candidate counts, in the order given.
    pub candidates: Vec<TreeCounts>,
}

pub fn compare_sets(
    a: &Path,
    candidates: &[PathBuf],
) -> Result<SetComparison, Box<dyn std::error::Error>> {
    compare_sets_with_progress(a, candidates, &|_, _| {})
}

pub fn compare_sets_with_progress(
    a: &Path,
    candidates: &[PathBuf],
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<SetComparison, Box<dyn std::error::Error>> {
    let a_files = collect_image_files(a)?;
    let a_hashes = hash_image_data_parallel(&a_files, 64 * 1024, progress);

    let mut a_set: HashSet<u64> = HashSet::new();
    let mut a_unreadable = 0usize;
    for fh in &a_hashes {
        if fh.decoded {
            a_set.insert(fh.hash);
        } else {
            a_unreadable += 1;
        }
    }

    // Per tree: decoded file hashes (files counted individually, so internal
    // duplicates count once per file).
    let mut tree_hashes: Vec<HashSet<u64>> = Vec::new();
    let mut tree_counts = Vec::new();

    let mut duplicates = 0usize;
    for root in candidates {
        let files = collect_image_files(root)?;
        let hashes = hash_image_data_parallel(&files, 64 * 1024, progress);

        let mut set: HashSet<u64> = HashSet::new();
        let mut dup = 0usize;
        let mut unreadable = 0usize;
        for fh in &hashes {
            if !fh.decoded {
                unreadable += 1;
                continue;
            }
            set.insert(fh.hash);
            if a_set.contains(&fh.hash) {
                dup += 1;
            }
        }
        duplicates += dup;
        let total = files.len();
        tree_hashes.push(set);
        tree_counts.push(TreeCounts {
            root: root.clone(),
            total,
            duplicates: dup,
            unique_to_tree: 0,
            shared_with_candidates: 0,
            unreadable,
        });
    }

    // Exclusivity: a candidate file is unique to its tree when its hash is in
    // no other candidate tree (it is already known not to be in A).
    for (i, counts) in tree_counts.iter_mut().enumerate() {
        let others: HashSet<u64> = tree_hashes
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .flat_map(|(_, s)| s.iter().copied())
            .collect();

        let files = collect_image_files(&counts.root)?;
        let hashes = hash_image_data_parallel(&files, 64 * 1024, progress);
        let mut unique = 0usize;
        let mut shared = 0usize;
        for fh in &hashes {
            if !fh.decoded || a_set.contains(&fh.hash) {
                continue;
            }
            if others.contains(&fh.hash) {
                shared += 1;
            } else {
                unique += 1;
            }
        }
        counts.unique_to_tree = unique;
        counts.shared_with_candidates = shared;
    }

    let candidates_only: usize = tree_counts
        .iter()
        .map(|c| c.total - c.duplicates - c.unreadable)
        .sum();

    Ok(SetComparison {
        a: a.to_path_buf(),
        a_total: a_files.len(),
        a_unique: a_set
            .iter()
            .filter(|h| !candidate_union_contains(&tree_hashes, **h))
            .count(),
        a_unreadable,
        duplicates,
        candidates_only,
        candidates: tree_counts,
    })
}

fn candidate_union_contains(tree_hashes: &[HashSet<u64>], hash: u64) -> bool {
    tree_hashes.iter().any(|s| s.contains(&hash))
}

/// One destination/candidate pair whose duplicate status should be verified
/// with a deep (full) comparison.
#[derive(Debug, Clone)]
pub struct DupPair {
    pub a: PathBuf,
    pub b: PathBuf,
}

/// Outcome of a deep scan: pairs confirmed as duplicates, and false positives
/// (the 64kb head matched but the files are not actually the same image).
#[derive(Debug)]
pub struct DeepScanOutcome {
    pub checked: usize,
    pub kept: usize,
    pub removed: Vec<DupPair>,
}

pub fn deep_scan_pairs(pairs: &[DupPair]) -> DeepScanOutcome {
    deep_scan_pairs_with_progress(pairs, &|_, _| {})
}

/// Re-verify each duplicate pair: exact bytes or identical decoded pixels mean
/// a true duplicate; anything else is a false positive. Files that cannot be
/// decoded are kept as duplicates (they cannot be proven otherwise).
pub fn deep_scan_pairs_with_progress(
    pairs: &[DupPair],
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> DeepScanOutcome {
    let total = pairs.len();
    let done = AtomicUsize::new(0);

    let kept_flags: Vec<bool> = pairs
        .par_iter()
        .map(|pair| {
            let kept = catch_unwind(AssertUnwindSafe(|| is_true_duplicate(&pair.a, &pair.b)))
                .unwrap_or(true);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            kept
        })
        .collect();

    let checked = pairs.len();
    let mut kept = 0usize;
    let mut removed = Vec::new();
    for (pair, is_kept) in pairs.iter().zip(kept_flags) {
        if is_kept {
            kept += 1;
        } else {
            removed.push(pair.clone());
        }
    }

    DeepScanOutcome {
        checked,
        kept,
        removed,
    }
}

fn is_true_duplicate(a: &Path, b: &Path) -> bool {
    use crate::image_reader::{hash_all_bytes, read_image};

    // Exact bytes: trivially duplicates.
    if let (Ok(ha), Ok(hb)) = (hash_all_bytes(a), hash_all_bytes(b)) {
        if ha == hb {
            return true;
        }
    }

    // Otherwise compare the fully decoded image data.
    match (
        catch_unwind(AssertUnwindSafe(|| read_image(a))),
        catch_unwind(AssertUnwindSafe(|| read_image(b))),
    ) {
        (Ok(Ok(ia)), Ok(Ok(ib))) => images_equal(&ia, &ib),
        _ => true, // undecodable: cannot prove a false positive
    }
}

fn images_equal(
    a: &crate::image_reader::ImageData,
    b: &crate::image_reader::ImageData,
) -> bool {
    use crate::image_reader::PixelData;

    if a.width != b.width || a.height != b.height || a.cpp != b.cpp {
        return false;
    }
    match (&a.data, &b.data) {
        (PixelData::U8(x), PixelData::U8(y)) => x == y,
        (PixelData::U16(x), PixelData::U16(y)) => x == y,
        _ => false,
    }
}
