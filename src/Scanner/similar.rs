//! Near-duplicate finder: same picture, different pixels — scaled-down or
//! recompressed copies of lossy images (jpg with jpg, heic with heic).
//!
//! Files are perceptual-hashed (through the scan cache, so the work is only
//! done once) and grouped by Hamming distance, with one extra gate: pHash
//! works on luminance structure, so a black-and-white copy hashes identically
//! to its color original. A mean-chroma comparison keeps achromatic and
//! chromatic versions of a picture apart. Each group is then reduced to a
//! keeper — the biggest file on disk — plus the smaller copies as removal
//! candidates.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rayon::prelude::*;

use crate::image_reader::{collect_image_files, distance, lossy_kind, Phash, SIMILAR_MAX_DISTANCE};
use crate::Scanner::cache::{
    full_content_cached, perceptual_cached, shallow_hashes_cached, ScanCache,
};

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

/// An exact duplicate found before the similarity pass: identical content
/// (shallow 64kb hash equal, confirmed with the full content hash). `keeper`
/// is the copy to keep, `path` the extra copy.
#[derive(Debug, Clone)]
pub struct ExactDuplicate {
    pub keeper: PathBuf,
    pub path: PathBuf,
}

#[derive(Debug, Default)]
pub struct SimilarOutcome {
    /// Supported media files found in the tree.
    pub scanned: usize,
    /// Files of a lossy kind that could be perceptual-hashed.
    pub lossy: usize,
    /// Exact duplicates, eliminated before similarity analysis.
    pub exact_duplicates: Vec<ExactDuplicate>,
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

    let entries: Vec<LossyEntry> = lossy
        .par_iter()
        .filter_map(|(_, path, kind)| {
            let perceptual = perceptual_cached(cache, path);
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            perceptual.map(|(phash, chroma)| LossyEntry {
                path: path.clone(),
                kind: *kind,
                phash,
                chroma,
                bytes,
            })
        })
        .collect();
    let lossy_hashed = entries.len();

    // Exact duplicates first (shallow 64kb hash, confirmed with the full
    // content hash): they are not "similar", they are the same file. The
    // extra copies are excluded from similarity analysis.
    let mut exact_duplicates: Vec<ExactDuplicate> = Vec::new();
    let mut excluded: HashSet<usize> = HashSet::new();
    {
        let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
        let shallow = shallow_hashes_cached(cache, &paths, 64 * 1024, &|_, _| {});
        let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, (hash, decoded)) in shallow.iter().enumerate() {
            if *decoded {
                buckets.entry(*hash).or_default().push(i);
            }
        }
        for members in buckets.into_values() {
            if members.len() < 2 {
                continue;
            }
            let mut by_content: HashMap<u64, Vec<usize>> = HashMap::new();
            for &i in &members {
                if let Ok(hash) = full_content_cached(cache, &entries[i].path) {
                    by_content.entry(hash).or_default().push(i);
                }
            }
            for mut ids in by_content.into_values() {
                if ids.len() < 2 {
                    continue;
                }
                ids.sort_by_key(|&i| {
                    (
                        std::cmp::Reverse(entries[i].bytes),
                        std::cmp::Reverse(entries[i].phash.pixels()),
                        entries[i].path.clone(),
                    )
                });
                let keeper = entries[ids[0]].path.clone();
                for &extra in &ids[1..] {
                    excluded.insert(extra);
                    exact_duplicates.push(ExactDuplicate {
                        keeper: keeper.clone(),
                        path: entries[extra].path.clone(),
                    });
                }
            }
        }
    }

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
        if excluded.contains(&i) {
            continue;
        }
        for j in (i + 1)..n {
            if excluded.contains(&j) {
                continue;
            }
            if entries[i].kind != entries[j].kind {
                continue;
            }
            if !chroma_compatible(entries[i].chroma, entries[j].chroma) {
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

    exact_duplicates.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(SimilarOutcome {
        scanned,
        lossy: lossy_hashed,
        exact_duplicates,
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
    /// Exact duplicates between the trees, eliminated before similarity.
    pub exact_duplicates: Vec<ExactDuplicate>,
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
    /// Mean chroma (colorfulness), when measurable.
    chroma: Option<u8>,
    bytes: u64,
}

/// A chroma this low means the picture is effectively black and white.
const ACHROMATIC_MAX: u8 = 6;
/// The other side must be at least this colorful for the pair to count as
/// "color vs black-and-white" rather than two versions of a gray picture.
const CHROMATIC_MIN: u8 = 10;
/// A bigger gap than this (with a big ratio) also counts as different
/// colorfulness, catching heavily desaturated edits.
const CHROMA_MAX_DIFF: i32 = 35;

/// Whether two pictures are color-compatible enough to be the same picture.
fn chroma_compatible(a: Option<u8>, b: Option<u8>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return true; // unknown: fall back to the phash verdict
    };
    let (lo, hi) = (a.min(b), a.max(b));
    if lo <= ACHROMATIC_MAX && hi >= CHROMATIC_MIN {
        return false; // one side is black and white, the other is not
    }
    if (hi as i32 - lo as i32) >= CHROMA_MAX_DIFF && hi >= lo.saturating_mul(2) {
        return false; // drastically different colorfulness
    }
    true
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
            let perceptual = perceptual_cached(cache, path);
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            perceptual.map(|(phash, chroma)| LossyEntry {
                path: path.clone(),
                kind: *kind,
                phash,
                chroma,
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

    // Exact duplicates first: shallow 64kb hash match between the trees,
    // confirmed with the full content hash (cache-backed). Those B files are
    // the same file as an A copy, not "similar", and skip the similarity
    // analysis entirely.
    let mut exact_duplicates: Vec<ExactDuplicate> = Vec::new();
    let mut duplicate_b: HashSet<usize> = HashSet::new();
    {
        let a_paths: Vec<PathBuf> = a_entries.iter().map(|e| e.path.clone()).collect();
        let b_paths: Vec<PathBuf> = b_entries.iter().map(|e| e.path.clone()).collect();
        let a_shallow = shallow_hashes_cached(cache, &a_paths, 64 * 1024, &|_, _| {});
        let b_shallow = shallow_hashes_cached(cache, &b_paths, 64 * 1024, &|_, _| {});
        let mut a_by_hash: HashMap<u64, usize> = HashMap::new();
        for (i, (hash, decoded)) in a_shallow.iter().enumerate() {
            if *decoded {
                a_by_hash.entry(*hash).or_insert(i);
            }
        }
        for (j, (hash, decoded)) in b_shallow.iter().enumerate() {
            if !*decoded {
                continue;
            }
            let Some(&i) = a_by_hash.get(hash) else {
                continue;
            };
            let content_a = full_content_cached(cache, &a_entries[i].path);
            let content_b = full_content_cached(cache, &b_entries[j].path);
            if let (Ok(content_a), Ok(content_b)) = (content_a, content_b) {
                if content_a == content_b {
                    duplicate_b.insert(j);
                    exact_duplicates.push(ExactDuplicate {
                        keeper: a_entries[i].path.clone(),
                        path: b_entries[j].path.clone(),
                    });
                }
            }
        }
    }
    exact_duplicates.sort_by(|a, b| a.path.cmp(&b.path));

    // For each remaining B file, the best A match: nearest phash, then
    // larger A copy.
    let mut matches = Vec::new();
    let mut removable_b = Vec::new();
    for (j, b) in b_entries.iter().enumerate() {
        if duplicate_b.contains(&j) {
            continue;
        }
        let mut best: Option<(&LossyEntry, u32)> = None;
        for a in &a_entries {
            if a.kind != b.kind {
                continue;
            }
            if !chroma_compatible(a.chroma, b.chroma) {
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
        exact_duplicates,
        matches,
        removable_b,
    })
}
