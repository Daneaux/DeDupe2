use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::Scanner::fastScan::hash_image_data_parallel;
use crate::exif::{creation_dates_batch, CreationDate};
use crate::image_reader::collect_image_files;

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
    // Dates are resolved in one pass: in-process readers per file, a single
    // batched exiftool subprocess for the undatable remainder, then folder
    // proxies (see creation_dates_batch).
    let dates = creation_dates_batch(paths, progress);
    paths
        .iter()
        .zip(dates)
        .map(|(path, creation_date)| DatedOriginal {
            path: path.clone(),
            creation_date,
        })
        .collect()
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

/// A pair that deep scanning rejected as NOT duplicates, with the layer of
/// evidence that disagreed (movies: full payload hashes, printed verbatim;
/// stills: decoded pixels).
#[derive(Debug)]
pub struct RemovedPair {
    pub a: PathBuf,
    pub b: PathBuf,
    pub reason: String,
}

/// Outcome of a deep scan: pairs confirmed as duplicates, and false positives
/// (the 64kb head matched but the files are not actually the same image).
/// A false-positive report is only produced for a B file that did NOT match
/// any A file: pairs whose B was confirmed against a *different* A file are
/// shallow-only links, counted in `covered` and suppressed — including from
/// the originals promotion the web layer derives from `removed`.
#[derive(Debug)]
pub struct DeepScanOutcome {
    /// Adjudicated pairs (kept + removed rows actually reported).
    pub checked: usize,
    pub kept: usize,
    /// Shallow pairs that disagreed but whose B is a confirmed duplicate of
    /// some other A file (cross-product leftovers).
    pub covered: usize,
    pub removed: Vec<RemovedPair>,
}

pub fn deep_scan_pairs(pairs: &[DupPair]) -> DeepScanOutcome {
    deep_scan_pairs_with_progress(pairs, &|_, _| {})
}

/// Re-verify each duplicate pair against its full content: exact file bytes,
/// a complete encoded-image-data hash (all scan data for stills, the whole
/// `mdat` payload for movies), and finally fully decoded pixels for stills.
/// Files whose content cannot be read at all stay "kept" as duplicates
/// (they cannot be proven false positives).
pub fn deep_scan_pairs_with_progress(
    pairs: &[DupPair],
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> DeepScanOutcome {
    let total = pairs.len();
    let done = AtomicUsize::new(0);

    let verdicts: Vec<Option<String>> = pairs
        .par_iter()
        .map(|pair| {
            let verdict = catch_unwind(AssertUnwindSafe(|| is_true_duplicate(&pair.a, &pair.b)))
                .unwrap_or(None);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            verdict
        })
        .collect();

    let checked = pairs.len();

    // The pairs form the A x B cross product of each 64kb hash bucket, so a
    // single B can sit in several pairs. B files confirmed against some A are
    // fully accounted for; any other pair mentioning that B is a shallow-only
    // link — it must not surface as a false positive (which would demote a
    // true duplicate back into the originals list downstream).
    let confirmed_b: HashSet<PathBuf> = pairs
        .iter()
        .zip(&verdicts)
        .filter(|(_, reason)| reason.is_none())
        .map(|(pair, _)| pair.b.clone())
        .collect();

    let mut kept = 0usize;
    let mut covered = 0usize;
    let mut removed = Vec::new();
    for (pair, reason) in pairs.iter().zip(verdicts) {
        match reason {
            None => kept += 1,
            Some(reason) => {
                if confirmed_b.contains(&pair.b) {
                    // B already proved to exist in A via a different pair.
                    covered += 1;
                    continue;
                }
                tracing::warn!(
                    "deep scan removed pair:\n  A: {}\n  B: {}\n  reason: {}",
                    pair.a.display(),
                    pair.b.display(),
                    reason
                );
                removed.push(RemovedPair {
                    a: pair.a.clone(),
                    b: pair.b.clone(),
                    reason,
                });
            }
        }
    }

    DeepScanOutcome {
        checked,
        kept,
        covered,
        removed,
    }
}
fn is_true_duplicate(a: &Path, b: &Path) -> Option<String> {
    use crate::image_reader::{hash_all_bytes, hash_image_data_all, is_video, read_image};

    // Exact bytes: trivially duplicates.
    if let (Ok(ha), Ok(hb)) = (hash_all_bytes(a), hash_all_bytes(b)) {
        if ha == hb {
            return None;
        }
    }

    // Full image-content hash: all encoded scan data for stills, the complete
    // `mdat` payload for movies. Movies have no pixel-decode stage, so for
    // them this hash is authoritative either way.
    match (hash_image_data_all(a), hash_image_data_all(b)) {
        (Ok(ha), Ok(hb)) => {
            if ha == hb {
                return None;
            }
            if is_video(a) || is_video(b) {
                return Some(format!(
                    "full video payloads differ (mdat hash {ha:016x} vs {hb:016x})"
                ));
            }
        }
        // Undecodable: cannot prove a false positive.
        _ => return None,
    }

    // Stills with different encoded data can still be the same image in a
    // different format; the final say is identical decoded pixels.
    match (
        catch_unwind(AssertUnwindSafe(|| read_image(a))),
        catch_unwind(AssertUnwindSafe(|| read_image(b))),
    ) {
        (Ok(Ok(ia)), Ok(Ok(ib))) => {
            if images_equal(&ia, &ib) {
                None
            } else {
                Some("decoded pixels differ".to_string())
            }
        }
        _ => None, // undecodable: cannot prove a false positive
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

/// The date a library folder layout implies for a file: `YYYY/MM-DD <desc>`
/// (event folder under a year folder) or a `YYYY-MM-DD` folder directly.
fn folder_implied_date(path: &Path) -> Option<(u32, u32, u32)> {
    use crate::exif::{parse_md_folder, parse_ymd_folder};

    let parent = path.parent()?.file_name()?.to_str()?.to_string();
    if let Some((y, m, d)) = parse_ymd_folder(&parent) {
        return Some((y, m, d));
    }
    let (m, d) = parse_md_folder(&parent)?;
    let grand = path.parent()?.parent()?.file_name()?.to_str()?.to_string();
    let y = grand.parse::<u32>().ok()?;
    Some((y, m, d))
}

/// Pull (year, month, day) out of any creation-date rendering.
fn ymd_from_string(s: &str) -> Option<(u32, u32, u32)> {
    let groups: Vec<u32> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .filter_map(|g| g.parse().ok())
        .collect();
    if groups.len() >= 3 {
        Some((groups[0], groups[1], groups[2]))
    } else {
        None
    }
}

#[derive(Debug)]
pub struct VerifyMismatch {
    pub path: PathBuf,
    /// The file's EXIF capture date (the truth).
    pub exif_date: String,
    /// The date the folder layout implies (where it currently sits).
    pub folder_date: String,
    /// Absolute difference in days between the two dates.
    pub days_off: i64,
}

/// Status of one file in the verification pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyStatus {
    Ok,
    Mismatch,
    OutsideStructure,
    Undated,
}

impl VerifyStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            VerifyStatus::Ok => "ok",
            VerifyStatus::Mismatch => "mismatch",
            VerifyStatus::OutsideStructure => "outside",
            VerifyStatus::Undated => "undated",
        }
    }
}

/// Every verified file, with its dates and classification.
#[derive(Debug)]
pub struct VerifiedFile {
    pub path: PathBuf,
    /// EXIF capture date, when known.
    pub exif_date: Option<String>,
    /// Folder-implied date, when the folder encodes one.
    pub folder_date: Option<String>,
    pub status: VerifyStatus,
    /// For mismatches: |exif - folder| in days.
    pub days_off: Option<i64>,
}

#[derive(Debug)]
pub struct VerifyOutcome {
    /// All supported media files found in the tree.
    pub total: usize,
    /// EXIF date agrees with the folder date.
    pub ok: usize,
    /// EXIF date disagrees with the folder date (wrong folder).
    pub mismatches: Vec<VerifyMismatch>,
    /// Has an EXIF date, but the folder does not encode one to verify against.
    pub outside_structure: usize,
    /// No EXIF date and no folder date.
    pub undated: usize,
    /// Every file with its classification, in folder order.
    pub files: Vec<VerifiedFile>,
}

pub fn verify_tree_dates(root: &Path) -> Result<VerifyOutcome, Box<dyn std::error::Error>> {
    verify_tree_dates_with_progress(root, &|_, _| {})
}

/// Verify that every file in the destination tree sits in the folder its EXIF
/// capture date implies. Catches files moved by stale/wrong dates.
pub fn verify_tree_dates_with_progress(
    root: &Path,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<VerifyOutcome, Box<dyn std::error::Error>> {
    use crate::exif::creation_dates_batch;

    let files = collect_image_files(root)?;
    let total = files.len();

    // Dates come from one pass over the tree: in-process readers per file,
    // a single batched exiftool subprocess for the undatable remainder, then
    // folder proxies. Each in-process read is still individually time-limited.
    let dates = creation_dates_batch(&files, progress);
    let classifications: Vec<(Option<(u32, u32, u32)>, Option<(u32, u32, u32)>)> = files
        .iter()
        .zip(dates)
        .map(|(path, date)| {
            (
                ymd_from_string(&date.to_string()),
                folder_implied_date(path),
            )
        })
        .collect();

    let mut outcome = VerifyOutcome {
        total,
        ok: 0,
        mismatches: Vec::new(),
        outside_structure: 0,
        undated: 0,
        files: Vec::with_capacity(files.len()),
    };

    let as_date = |(y, m, d): (u32, u32, u32)| chrono::NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32);
    let days_between = |a: (u32, u32, u32), b: (u32, u32, u32)| -> i64 {
        match (as_date(a), as_date(b)) {
            (Some(da), Some(db)) => da.signed_duration_since(db).num_days().abs(),
            _ => 0,
        }
    };

    for (path, (exif_ymd, folder_ymd)) in files.iter().zip(classifications) {
        let (status, days_off) = match (exif_ymd, folder_ymd) {
            (Some(e), Some(f)) => {
                if e == f {
                    outcome.ok += 1;
                    (VerifyStatus::Ok, Some(0))
                } else {
                    outcome.mismatches.push(VerifyMismatch {
                        path: path.clone(),
                        exif_date: format!("{:04}-{:02}-{:02}", e.0, e.1, e.2),
                        folder_date: format!("{:04}-{:02}-{:02}", f.0, f.1, f.2),
                        days_off: days_between(e, f),
                    });
                    (VerifyStatus::Mismatch, Some(days_between(e, f)))
                }
            }
            (Some(_), None) => {
                outcome.outside_structure += 1;
                (VerifyStatus::OutsideStructure, None)
            }
            (None, _) => {
                outcome.undated += 1;
                (VerifyStatus::Undated, None)
            }
        };

        outcome.files.push(VerifiedFile {
            path: path.clone(),
            exif_date: exif_ymd.map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}")),
            folder_date: folder_ymd.map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}")),
            status,
            days_off,
        });
    }

    outcome.mismatches.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(outcome)
}
