//! Near-duplicate finder: same picture, different pixels — scaled-down or
//! recompressed copies of lossy images (jpg with jpg, heic with heic).
//!
//! Files are perceptual-hashed (through the scan cache, so the work is only
//! done once), grouped by Hamming distance, and each group is reduced to a
//! keeper — the biggest file on disk — plus the smaller copies as removal
//! candidates.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rayon::prelude::*;

use crate::image_reader::{collect_image_files, distance, lossy_kind, Phash, SIMILAR_MAX_DISTANCE};
use crate::Scanner::cache::{cached_phash, ScanCache};

/// One other copy of the same picture, with the evidence for its verdict.
#[derive(Debug, Clone)]
pub struct SimilarCandidate {
    pub path: PathBuf,
    /// Hamming distance between the candidate and its keeper.
    pub distance: u32,
    pub pixels: u64,
    pub bytes: u64,
    /// Eligible for the purgatory move (everything that is not the keeper).
    pub removable: bool,
}

/// A group of near-duplicates: one keeper (the best copy) and everything
/// that looks like the same picture at lower resolution/quality.
#[derive(Debug, Clone)]
pub struct SimilarGroup {
    pub keeper: PathBuf,
    pub keeper_pixels: u64,
    pub keeper_bytes: u64,
    pub candidates: Vec<SimilarCandidate>,
}

#[derive(Debug, Default)]
pub struct SimilarOutcome {
    /// Supported media files found in the tree.
    pub scanned: usize,
    /// Files of a lossy kind that could be perceptual-hashed.
    pub lossy: usize,
    pub groups: Vec<SimilarGroup>,
    /// Total members across all groups (keepers + candidates).
    pub candidates: usize,
    /// Candidates eligible for the purgatory move (deduplicated, sorted).
    pub removable: Vec<PathBuf>,
}

/// Find near-duplicate groups under `root`, phashes served from the cache.
pub fn find_similar_with_progress(
    root: &Path,
    cache: &Mutex<ScanCache>,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<SimilarOutcome, Box<dyn std::error::Error>> {
    let files = collect_image_files(root)?;
    let scanned = files.len();

    // Only lossy same-kind families get compared, with their kind kept so
    // jpg never unions with heic.
    let lossy: Vec<(usize, PathBuf, u8)> = files
        .iter()
        .enumerate()
        .filter_map(|(i, path)| lossy_kind(path).map(|kind| (i, path.clone(), kind)))
        .collect();
    let total = lossy.len();
    let done = AtomicUsize::new(0);

    struct Entry {
        path: PathBuf,
        kind: u8,
        phash: Phash,
        bytes: u64,
    }

    let entries: Vec<Entry> = lossy
        .par_iter()
        .filter_map(|(_, path, kind)| {
            let phash = cached_phash(cache, path);
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            phash.map(|phash| Entry {
                path: path.clone(),
                kind: *kind,
                phash,
                bytes,
            })
        })
        .collect();
    let lossy_hashed = entries.len();

    // Union-find over similar pairs (O(n²) popcounts; the file set here is
    // already narrowed to lossy images).
    let n = entries.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut Vec<usize>, mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut unions = 0usize;
    for i in 0..n {
        progress(unions, n.max(1));
        for j in (i + 1)..n {
            if entries[i].kind != entries[j].kind {
                continue;
            }
            if distance(entries[i].phash.hash, entries[j].phash.hash) <= SIMILAR_MAX_DISTANCE {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                    unions += 1;
                }
            }
        }
    }

    let mut members: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        members.entry(root).or_default().push(i);
    }

    let mut groups = Vec::new();
    let mut candidates_total = 0usize;
    let mut removable_total: Vec<PathBuf> = Vec::new();
    for mut ids in members.into_values() {
        if ids.len() < 2 {
            continue;
        }
        ids.sort();
        // Keeper: the biggest file on disk (ties: most pixels, then path).
        let keeper_idx = ids
            .iter()
            .copied()
            .max_by_key(|&i| {
                (
                    entries[i].bytes,
                    entries[i].phash.pixels(),
                    std::cmp::Reverse(entries[i].path.clone()),
                )
            })
            .expect("non-empty group");

        let keeper = &entries[keeper_idx];
        let mut candidates: Vec<SimilarCandidate> = ids
            .iter()
            .filter(|&&i| i != keeper_idx)
            .map(|&i| SimilarCandidate {
                path: entries[i].path.clone(),
                distance: distance(keeper.phash.hash, entries[i].phash.hash),
                pixels: entries[i].phash.pixels(),
                bytes: entries[i].bytes,
                removable: true,
            })
            .collect();
        candidates.sort_by(|a, b| a.path.cmp(&b.path));
        candidates_total += candidates.len();
        for candidate in &candidates {
            if candidate.removable {
                removable_total.push(candidate.path.clone());
            }
        }
        groups.push(SimilarGroup {
            keeper: keeper.path.clone(),
            keeper_pixels: keeper.phash.pixels(),
            keeper_bytes: keeper.bytes,
            candidates,
        });
    }
    groups.sort_by(|a, b| a.keeper.cmp(&b.keeper));

    removable_total.sort();
    removable_total.dedup();

    Ok(SimilarOutcome {
        scanned,
        lossy: lossy_hashed,
        groups,
        candidates: candidates_total,
        removable: removable_total,
    })
}

/// One cross-tree near-duplicate: a B file that looks like an A file, with
/// the evidence deciding which side is the better copy to keep.
#[derive(Debug, Clone)]
pub struct SimilarMatch {
    pub a: PathBuf,
    pub b: PathBuf,
    pub distance: u32,
    pub a_pixels: u64,
    pub b_pixels: u64,
    pub a_bytes: u64,
    pub b_bytes: u64,
    /// True when the A copy is the better one (B may be removable).
    pub a_is_better: bool,
    /// True when B is the better copy (never offered for removal).
    pub b_is_better: bool,
    /// B eligible for the purgatory move: A is the bigger file.
    pub removable: bool,
}

#[derive(Debug, Default)]
pub struct SimilarCompareOutcome {
    pub scanned_a: usize,
    pub scanned_b: usize,
    pub lossy_a: usize,
    pub lossy_b: usize,
    pub matches: Vec<SimilarMatch>,
    /// B-side files eligible for the purgatory move (deduplicated, sorted).
    pub removable_b: Vec<PathBuf>,
}

/// Which side is the better copy: the bigger file on disk wins. Ties fall
/// back to more pixels, then to A (the library of record).
fn pick_better(a_pixels: u64, a_bytes: u64, b_pixels: u64, b_bytes: u64) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match a_bytes.cmp(&b_bytes) {
        Ordering::Equal => match a_pixels.cmp(&b_pixels) {
            Ordering::Equal => Ordering::Greater,
            other => other,
        },
        other => other,
    }
}

struct LossyEntry {
    path: PathBuf,
    kind: u8,
    phash: Phash,
    bytes: u64,
}

fn collect_lossy(
    root: &Path,
    cache: &Mutex<ScanCache>,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<(usize, Vec<LossyEntry>), Box<dyn std::error::Error>> {
    let files = collect_image_files(root)?;
    let scanned = files.len();
    let lossy: Vec<(PathBuf, u8)> = files
        .into_iter()
        .filter_map(|path| lossy_kind(&path).map(|kind| (path, kind)))
        .collect();
    let total = lossy.len();
    let done = AtomicUsize::new(0);

    let entries: Vec<LossyEntry> = lossy
        .par_iter()
        .filter_map(|(path, kind)| {
            let phash = cached_phash(cache, path);
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            phash.map(|phash| LossyEntry {
                path: path.clone(),
                kind: *kind,
                phash,
                bytes,
            })
        })
        .collect();
    Ok((scanned, entries))
}

/// Compare two trees: every B file that looks like an A file (same lossy
/// kind, phash distance within the threshold) is reported with the verdict
/// of which side is the better copy. `removable_b` holds the B files that
/// are the worse copy and safe to purge.
pub fn find_similar_between_with_progress(
    a_root: &Path,
    b_root: &Path,
    cache: &Mutex<ScanCache>,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<SimilarCompareOutcome, Box<dyn std::error::Error>> {
    let (scanned_a, a_entries) = collect_lossy(a_root, cache, progress)?;
    let (scanned_b, b_entries) = collect_lossy(b_root, cache, progress)?;

    // For each B file, the best A match: nearest phash, then larger A copy.
    let mut matches = Vec::new();
    let mut removable_b = Vec::new();
    for b in &b_entries {
        let mut best: Option<(&LossyEntry, u32)> = None;
        for a in &a_entries {
            if a.kind != b.kind {
                continue;
            }
            let distance = distance(a.phash.hash, b.phash.hash);
            if distance > SIMILAR_MAX_DISTANCE {
                continue;
            }
            let replace = match best {
                None => true,
                Some((current, current_distance)) => {
                    (distance, std::cmp::Reverse(a.phash.pixels())) < (current_distance, std::cmp::Reverse(current.phash.pixels()))
                }
            };
            if replace {
                best = Some((a, distance));
            }
        }
        let Some((a, distance)) = best else {
            continue;
        };

        let verdict = pick_better(a.phash.pixels(), a.bytes, b.phash.pixels(), b.bytes);
        let a_is_better = verdict == std::cmp::Ordering::Greater;
        let b_is_better = verdict == std::cmp::Ordering::Less;
        let removable = a_is_better;
        if removable {
            removable_b.push(b.path.clone());
        }
        matches.push(SimilarMatch {
            a: a.path.clone(),
            b: b.path.clone(),
            distance,
            a_pixels: a.phash.pixels(),
            b_pixels: b.phash.pixels(),
            a_bytes: a.bytes,
            b_bytes: b.bytes,
            a_is_better,
            b_is_better,
            removable,
        });
    }
    matches.sort_by(|x, y| x.b.cmp(&y.b));
    removable_b.sort();
    removable_b.dedup();

    Ok(SimilarCompareOutcome {
        scanned_a,
        scanned_b,
        lossy_a: a_entries.len(),
        lossy_b: b_entries.len(),
        matches,
        removable_b,
    })
}
