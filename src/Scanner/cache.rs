//! Hierarchical scan cache: mirrors the directory tree so scan results can be
//! reused across passes and invalidated per branch.
//!
//! Each file entry accumulates data as passes run — a shallow scan fills the
//! 64kb hash, a later EXIF pass adds the date, a deep scan adds the full byte
//! / full content / decoded-pixel data. Nothing ever rebuilds entries from
//! scratch: an entry is only reset when the file itself changed (size or
//! mtime), and directories are invalidated branch-wise on request.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::exif::CreationDate;
use crate::image_reader::{self, ImageData, ImageReaderError, PixelData};

/// What the cache did on behalf of the caller — surfaced in the UI so a
/// reused tree is visible at a glance.
#[derive(Debug, Default, Clone, Copy)]
pub struct CacheStats {
    /// Files currently held in the tree (tracked, never counted by walking).
    pub files: usize,
    /// Lookups served from the cache without touching the file again.
    pub reused: usize,
    /// Lookups that had to compute (and then store) a value.
    pub computed: usize,
}

/// Decoded pixels reduced to a comparable key (dims + content hash), so deep
/// scans can compare two files without decoding them twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelKey {
    pub width: usize,
    pub height: usize,
    pub cpp: usize,
    pub hash: u64,
}

#[derive(Debug, Clone, Default)]
struct FileEntry {
    size: u64,
    mtime: Option<SystemTime>,
    /// 64kb encoded-image-data hash + decoded flag.
    shallow: Option<(u64, bool)>,
    /// Whole-file byte hash.
    full_bytes: Option<u64>,
    /// Full encoded-image-data hash (deep scan step 2).
    full_content: Option<u64>,
    /// Decoded pixel key (deep scan step 3).
    pixel: Option<PixelKey>,
    /// Cached creation date (may be a cached `Unknown`).
    date: Option<CreationDate>,
}

impl FileEntry {
    fn matches(&self, size: u64, mtime: Option<SystemTime>) -> bool {
        self.size == size && self.mtime == mtime
    }
}

#[derive(Debug, Default)]
struct DirNode {
    children: HashMap<OsString, DirNode>,
    files: HashMap<OsString, FileEntry>,
}

impl DirNode {
    fn file_count(&self) -> usize {
        self.files.len() + self.children.values().map(DirNode::file_count).sum::<usize>()
    }

    fn accumulate_coverage(&self, coverage: &mut BranchCoverage) {
        for entry in self.files.values() {
            coverage.files += 1;
            if entry.shallow.is_some() {
                coverage.shallow += 1;
            }
            if entry.full_bytes.is_some() || entry.full_content.is_some() {
                coverage.deep += 1;
            }
            if entry.date.is_some() {
                coverage.exif += 1;
            }
        }
        for child in self.children.values() {
            child.accumulate_coverage(coverage);
        }
    }
}

/// What a subtree already has cached, layer by layer.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BranchCoverage {
    pub files: usize,
    pub shallow: usize,
    pub deep: usize,
    pub exif: usize,
}

#[derive(Debug, Default)]
pub struct ScanCache {
    root: DirNode,
    stats: CacheStats,
}

impl ScanCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    /// Drop everything cached under `dir` (its own entries and all children).
    pub fn invalidate_branch(&mut self, dir: &Path) {
        let mut node = &mut self.root;
        for comp in dir.components() {
            let name = comp.as_os_str().to_os_string();
            match node.children.get_mut(&name) {
                Some(child) => node = child,
                None => return,
            }
        }
        let dropped = node.file_count();
        self.stats.files = self.stats.files.saturating_sub(dropped);
        *node = DirNode::default();
    }

    /// How many cached file entries live under `path` — the subtree count for
    /// a directory, 1 for a cached file, 0 for anything never scanned. Backs
    /// the UI's "cached" indication; walks only the cached tree, never the
    /// filesystem.
    pub fn branch_files(&self, path: &Path) -> usize {
        self.branch_coverage(path).files
    }

    /// Per-layer coverage of the subtree at `path`: how many of its files
    /// have shallow hashes, deep data, and dates cached.
    pub fn branch_coverage(&self, path: &Path) -> BranchCoverage {
        let mut node = &self.root;
        for comp in path.components() {
            let name = comp.as_os_str().to_os_string();
            if let Some(child) = node.children.get(&name) {
                node = child;
                continue;
            }
            // The final component may be a file tracked in this node.
            if node.files.contains_key(comp.as_os_str()) {
                return BranchCoverage {
                    files: 1,
                    shallow: usize::from(node.files[comp.as_os_str()].shallow.is_some()),
                    deep: usize::from(
                        node.files[comp.as_os_str()].full_bytes.is_some()
                            || node.files[comp.as_os_str()].full_content.is_some(),
                    ),
                    exif: usize::from(node.files[comp.as_os_str()].date.is_some()),
                };
            }
            return BranchCoverage::default();
        }
        let mut coverage = BranchCoverage::default();
        node.accumulate_coverage(&mut coverage);
        coverage
    }

    /// Drop a single file's entry.
    pub fn invalidate_file(&mut self, path: &Path) {
        self.remove_entry(path);
    }

    /// Remove an entry without materializing any directory nodes.
    fn remove_entry(&mut self, path: &Path) {
        let Some((name, parent)) = file_key(path) else {
            return;
        };
        if let Some(node) = Self::node_mut_existing(&mut self.root, parent) {
            if node.files.remove(&name).is_some() {
                self.stats.files = self.stats.files.saturating_sub(1);
            }
        }
    }

    /// Walk to an existing node; never creates nodes as a side effect.
    fn node_mut_existing<'t>(node: &'t mut DirNode, dir: &Path) -> Option<&'t mut DirNode> {
        let mut node = node;
        for comp in dir.components() {
            let name = comp.as_os_str().to_os_string();
            node = node.children.get_mut(&name)?;
        }
        Some(node)
    }

    fn node_mut<'t>(node: &'t mut DirNode, dir: &Path) -> Option<&'t mut DirNode> {
        let mut node = node;
        for comp in dir.components() {
            let name = comp.as_os_str().to_os_string();
            node = node.children.entry(name).or_default();
        }
        Some(node)
    }

    fn with_entry<T: Clone>(
        &mut self,
        path: &Path,
        get: impl FnOnce(&FileEntry) -> Option<T>,
        set: impl FnOnce(&mut FileEntry, T),
        compute: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(_) => {
                // The file no longer exists: drop its entry so the cache
                // never holds data for paths that are gone.
                self.remove_entry(path);
                return None;
            }
        };
        let size = meta.len();
        let mtime = meta.modified().ok();
        let name = path.file_name()?.to_os_string();
        let parent = path.parent()?;

        // Reset the entry when the file changed; the whole point of the
        // hierarchy is that only affected entries lose their data.
        {
            let node = Self::node_mut(&mut self.root, parent)?;
            let entry = match node.files.entry(name.clone()) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(FileEntry {
                        size,
                        mtime,
                        ..Default::default()
                    });
                    self.stats.files += 1;
                    node.files.get_mut(&name).expect("just inserted")
                }
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    if !slot.get().matches(size, mtime) {
                        *slot.get_mut() = FileEntry {
                            size,
                            mtime,
                            ..Default::default()
                        };
                    }
                    slot.into_mut()
                }
            };
            if let Some(value) = get(entry) {
                self.stats.reused += 1;
                return Some(value);
            }
        }

        let value = compute()?;
        self.stats.computed += 1;
        let node = Self::node_mut(&mut self.root, parent)?;
        let entry = node.files.entry(name).or_default();
        set(entry, value.clone());
        Some(value)
    }

    /// 64kb encoded-image-data hash (with decoded flag), cached.
    pub fn shallow_hash(&mut self, path: &Path, n: usize) -> (u64, bool) {
        if let Some(v) = self.shallow_hash_lookup(path) {
            return v;
        }
        let v = image_reader::hash_image_data_status(path, n);
        self.shallow_hash_store(path, v);
        v
    }

    /// Non-computing shallow lookup (still applies the stat-based reset).
    pub fn shallow_hash_lookup(&mut self, path: &Path) -> Option<(u64, bool)> {
        self.with_entry(path, |e| e.shallow, |_e, _v| {}, || None)
    }

    pub fn shallow_hash_store(&mut self, path: &Path, value: (u64, bool)) {
        self.store_layer(path, value, |entry| &mut entry.shallow, |entry, v| entry.shallow = Some(v));
    }

    /// Whole-file byte hash, cached on success.
    pub fn full_bytes_hash(&mut self, path: &Path) -> Result<u64, ImageReaderError> {
        if let Some(v) = self.full_bytes_lookup(path) {
            return Ok(v);
        }
        let v = image_reader::hash_all_bytes(path)?;
        self.full_bytes_store(path, v);
        Ok(v)
    }

    pub fn full_bytes_lookup(&mut self, path: &Path) -> Option<u64> {
        self.with_entry(path, |e| e.full_bytes, |_e, _v| {}, || None)
    }

    pub fn full_bytes_store(&mut self, path: &Path, value: u64) {
        self.store_layer(path, value, |entry| &mut entry.full_bytes, |entry, v| entry.full_bytes = Some(v));
    }

    /// Full encoded-image-data hash (deep scan step 2), cached on success.
    pub fn full_content_hash(&mut self, path: &Path) -> Result<u64, ImageReaderError> {
        if let Some(v) = self.full_content_lookup(path) {
            return Ok(v);
        }
        let v = image_reader::hash_image_data_all(path)?;
        self.full_content_store(path, v);
        Ok(v)
    }

    pub fn full_content_lookup(&mut self, path: &Path) -> Option<u64> {
        self.with_entry(path, |e| e.full_content, |_e, _v| {}, || None)
    }

    pub fn full_content_store(&mut self, path: &Path, value: u64) {
        self.store_layer(path, value, |entry| &mut entry.full_content, |entry, v| entry.full_content = Some(v));
    }

    /// Decoded-pixel key (deep scan step 3). Decode failures are not cached.
    pub fn pixel_key(&mut self, path: &Path) -> Option<PixelKey> {
        if let Some(v) = self.pixel_lookup(path) {
            return Some(v);
        }
        let v = pixel_key_of(path)?;
        self.pixel_store(path, v);
        Some(v)
    }

    pub fn pixel_lookup(&mut self, path: &Path) -> Option<PixelKey> {
        self.with_entry(path, |e| e.pixel, |_e, _v| {}, || None)
    }

    pub fn pixel_store(&mut self, path: &Path, value: PixelKey) {
        self.store_layer(path, value, |entry| &mut entry.pixel, |entry, v| entry.pixel = Some(v));
    }

    /// Cached creation date (a cached `Unknown` is a real answer, so date
    /// presence itself is the cache hit).
    pub fn creation_date(&mut self, path: &Path) -> Option<CreationDate> {
        self.with_entry(path, |e| e.date.clone(), |_e, _v| {}, || None)
    }

    /// Store a value into one layer, counting a first-time store as computed.
    fn store_layer<T: Clone>(
        &mut self,
        path: &Path,
        value: T,
        layer: impl FnOnce(&mut FileEntry) -> &mut Option<T>,
        set: impl FnOnce(&mut FileEntry, T),
    ) {
        // Never (re-)create an entry for a file that no longer exists — the
        // compute may have raced a deletion, and a missing file must not
        // resurrect a ghost entry.
        if std::fs::metadata(path).is_err() {
            return;
        }
        let Some((name, parent)) = file_key(path) else {
            return;
        };
        let Some(node) = Self::node_mut(&mut self.root, parent) else {
            return;
        };
        let entry = match node.files.entry(name) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                self.stats.files += 1;
                self.stats.computed += 1;
                slot.insert(FileEntry::default())
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                if layer(slot.get_mut()).is_none() {
                    self.stats.computed += 1;
                }
                slot.into_mut()
            }
        };
        set(entry, value);
    }

    pub fn store_creation_date(&mut self, path: &Path, date: CreationDate) {
        if std::fs::metadata(path).is_err() {
            return;
        }
        if let Some((name, parent)) = file_key(path) {
            if let Some(node) = Self::node_mut(&mut self.root, parent) {
                let entry = node.files.entry(name).or_default();
                if entry.date.is_none() {
                    self.stats.computed += 1;
                }
                entry.date = Some(date);
            }
        }
    }
}

fn file_key(path: &Path) -> Option<(OsString, &Path)> {
    Some((path.file_name()?.to_os_string(), path.parent()?))
}

/// Lock helper that survives a poisoned mutex (a panic while hashing one
/// file must not disable the cache for the rest of the scan).
pub fn lock(cache: &Mutex<ScanCache>) -> std::sync::MutexGuard<'_, ScanCache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

fn pixel_key_of(path: &Path) -> Option<PixelKey> {
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        image_reader::read_image(path)
    }));
    let image: ImageData = match decoded {
        Ok(Ok(image)) => image,
        _ => return None,
    };
    let hash = match &image.data {
        PixelData::U8(bytes) => seahash::hash(bytes),
        PixelData::U16(words) => {
            let mut hasher = seahash::SeaHasher::new();
            use std::hash::Hasher;
            for chunk in words.chunks(4096) {
                for &word in chunk {
                    hasher.write_u16(word);
                }
            }
            hasher.finish()
        }
    };
    Some(PixelKey {
        width: image.width,
        height: image.height,
        cpp: image.cpp,
        hash,
    })
}

/// Shallow-hash many files through one shared cache. The lock is only held
/// for the lookup and the store — the actual file hashing runs outside it, so
/// the pass stays parallel instead of serializing behind the cache.
pub fn shallow_hashes_cached(
    cache: &Mutex<ScanCache>,
    paths: &[PathBuf],
    n: usize,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<(u64, bool)> {
    let total = paths.len();
    let done = AtomicUsize::new(0);
    paths
        .par_iter()
        .map(|path| {
            // NOTE: the lookup guard must be dropped before the store lock —
            // a `match lock(...)` scrutinee guard would live through the
            // whole match and deadlock the second lock.
            let cached = { lock(cache).shallow_hash_lookup(path) };
            let result = match cached {
                Some(value) => value,
                None => {
                    let value = image_reader::hash_image_data_status(path, n);
                    lock(cache).shallow_hash_store(path, value);
                    value
                }
            };
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            result
        })
        .collect()
}

/// Whole-file byte hash through the cache, computed outside the lock.
pub fn full_bytes_cached(cache: &Mutex<ScanCache>, path: &Path) -> Result<u64, ImageReaderError> {
    if let Some(value) = lock(cache).full_bytes_lookup(path) {
        return Ok(value);
    }
    let value = image_reader::hash_all_bytes(path)?;
    lock(cache).full_bytes_store(path, value);
    Ok(value)
}

/// Full encoded-image-data hash through the cache, computed outside the lock.
pub fn full_content_cached(cache: &Mutex<ScanCache>, path: &Path) -> Result<u64, ImageReaderError> {
    if let Some(value) = lock(cache).full_content_lookup(path) {
        return Ok(value);
    }
    let value = image_reader::hash_image_data_all(path)?;
    lock(cache).full_content_store(path, value);
    Ok(value)
}

/// Decoded pixel key through the cache, computed outside the lock.
pub fn pixel_cached(cache: &Mutex<ScanCache>, path: &Path) -> Option<PixelKey> {
    if let Some(value) = lock(cache).pixel_lookup(path) {
        return Some(value);
    }
    let value = pixel_key_of(path)?;
    lock(cache).pixel_store(path, value);
    Some(value)
}

/// Date resolution through the cache: cached dates are served directly; the
/// misses go through the normal batched funnel (in-process readers → one
/// exiftool run → folder proxy) and are stored as they come back.
pub fn creation_dates_cached(
    cache: &Mutex<ScanCache>,
    paths: &[PathBuf],
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<CreationDate> {
    let total = paths.len();
    let mut out: Vec<Option<CreationDate>> = Vec::with_capacity(total);
    let mut misses: Vec<(usize, PathBuf)> = Vec::new();
    {
        let mut cache = lock(cache);
        for (i, path) in paths.iter().enumerate() {
            match cache.creation_date(path) {
                Some(date) => {
                    out.push(Some(date));
                    progress(i + 1, total);
                }
                None => {
                    out.push(None);
                    misses.push((i, path.clone()));
                }
            }
        }
    }

    if !misses.is_empty() {
        let miss_paths: Vec<PathBuf> = misses.iter().map(|(_, p)| p.clone()).collect();
        let hits = total - misses.len();
        let computed = crate::exif::creation_dates_batch(&miss_paths, &|done, _total| {
            progress(hits + done, total);
        });
        let mut cache = lock(cache);
        for ((i, path), date) in misses.iter().zip(computed) {
            out[*i] = Some(date.clone());
            cache.store_creation_date(path, date);
        }
    }

    out.into_iter()
        .map(|date| date.unwrap_or(CreationDate::Unknown))
        .collect()
}
