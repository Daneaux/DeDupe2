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
use crate::image_reader::{self, ImageData, ImageReaderError, Phash, PixelData};

/// What the cache did on behalf of the caller — surfaced in the UI so a
/// reused tree is visible at a glance.
#[derive(Debug, Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PixelKey {
    pub width: usize,
    pub height: usize,
    pub cpp: usize,
    pub hash: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct FileEntry {
    size: u64,
    mtime: Option<SystemTime>,
    /// Device id of the volume the file lives on (unix `st_dev`), 0 when
    /// unavailable. Persisted so same-volume decisions survive restarts.
    dev: u64,
    /// 64kb encoded-image-data hash + decoded flag.
    shallow: Option<(u64, bool)>,
    /// Whole-file byte hash.
    full_bytes: Option<u64>,
    /// Full encoded-image-data hash (deep scan step 2).
    full_content: Option<u64>,
    /// Decoded pixel key (deep scan step 3).
    pixel: Option<PixelKey>,
    /// Perceptual hash + source dimensions (near-duplicate detection for
    /// lossy same-kind files).
    phash: Option<Phash>,
    /// Cached creation date (may be a cached `Unknown`).
    date: Option<CreationDate>,
}

impl FileEntry {
    fn matches(&self, size: u64, mtime: Option<SystemTime>) -> bool {
        self.size == size && self.mtime == mtime
    }
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
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
            if entry.phash.is_some() {
                coverage.phash += 1;
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
    pub phash: usize,
    pub exif: usize,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanCache {
    root: DirNode,
    stats: CacheStats,
    /// Something changed since the last save. Session state, not persisted.
    #[serde(skip)]
    dirty: bool,
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
        self.dirty = true;
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
                let entry = &node.files[comp.as_os_str()];
                return BranchCoverage {
                    files: 1,
                    shallow: usize::from(entry.shallow.is_some()),
                    deep: usize::from(entry.full_bytes.is_some() || entry.full_content.is_some()),
                    phash: usize::from(entry.phash.is_some()),
                    exif: usize::from(entry.date.is_some()),
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
                self.dirty = true;
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
        let dev = device_of(&meta);
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
                        dev,
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
                            dev,
                            ..Default::default()
                        };
                        self.dirty = true;
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

    /// Perceptual hash, cached on success.
    pub fn phash(&mut self, path: &Path) -> Option<Phash> {
        if let Some(v) = self.phash_lookup(path) {
            return Some(v);
        }
        let v = image_reader::phash(path)?;
        self.phash_store(path, v);
        Some(v)
    }

    pub fn phash_lookup(&mut self, path: &Path) -> Option<Phash> {
        self.with_entry(path, |e| e.phash, |_e, _v| {}, || None)
    }

    pub fn phash_store(&mut self, path: &Path, value: Phash) {
        self.store_layer(path, value, |entry| &mut entry.phash, |entry, v| entry.phash = Some(v));
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
        self.dirty = true;
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
                self.dirty = true;
            }
        }
    }
}

#[cfg(unix)]
fn device_of(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.dev()
}

#[cfg(not(unix))]
fn device_of(_meta: &std::fs::Metadata) -> u64 {
    0
}

fn file_key(path: &Path) -> Option<(OsString, &Path)> {
    Some((path.file_name()?.to_os_string(), path.parent()?))
}

/// On-disk snapshot version. Bump when the entry shape changes; older files
/// are migrated when the difference is lossless, ignored otherwise.
const CACHE_FORMAT_VERSION: u32 = 2;

// ---- version 1 compatibility -------------------------------------------
// v1 stored a `sharpness` estimate inside Phash (a heuristic that was
// removed). Everything else is identical, so a v1 snapshot converts by
// dropping that one field — no cache knowledge is lost.
// Remove after the next format bump.
mod v1 {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::time::SystemTime;

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct PersistedCache {
        pub version: u32,
        pub root: DirNode,
        pub stats: CacheStats,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct DirNode {
        pub children: HashMap<OsString, DirNode>,
        pub files: HashMap<OsString, FileEntry>,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct FileEntry {
        pub size: u64,
        pub mtime: Option<SystemTime>,
        pub dev: u64,
        pub shallow: Option<(u64, bool)>,
        pub full_bytes: Option<u64>,
        pub full_content: Option<u64>,
        pub pixel: Option<PixelKey>,
        pub phash: Option<PhashV1>,
        pub date: Option<CreationDate>,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct PhashV1 {
        pub hash: u64,
        pub width: u32,
        pub height: u32,
        #[allow(dead_code)]
        pub sharpness: f32,
    }

    pub fn convert_node(node: DirNode) -> DirNodeSuper {
        DirNodeSuper {
            children: node
                .children
                .into_iter()
                .map(|(name, child)| (name, convert_node(child)))
                .collect(),
            files: node
                .files
                .into_iter()
                .map(|(name, entry)| (name, convert_entry(entry)))
                .collect(),
        }
    }

    fn convert_entry(entry: FileEntry) -> FileEntrySuper {
        FileEntrySuper {
            size: entry.size,
            mtime: entry.mtime,
            dev: entry.dev,
            shallow: entry.shallow,
            full_bytes: entry.full_bytes,
            full_content: entry.full_content,
            pixel: entry.pixel,
            phash: entry.phash.map(|p| Phash {
                hash: p.hash,
                width: p.width,
                height: p.height,
            }),
            date: entry.date,
        }
    }

    type DirNodeSuper = super::DirNode;
    type FileEntrySuper = super::FileEntry;
}

/// The persisted form is the cache structure itself, serialized with serde
/// through a binary codec — no hand-translated shape, so non-UTF8 path
/// components and every learned layer (shallow hash, full hashes, pixel key,
/// phash, dates, device id) round-trip losslessly.
#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedCache {
    version: u32,
    root: DirNode,
    stats: CacheStats,
}

/// Where the cache persists by default: `$DEDUPE2_CACHE`, else the platform
/// cache directory (`~/Library/Caches/dedupe2/…` on macOS, `~/.cache/…`
/// elsewhere), else the temp directory.
pub fn default_cache_path() -> PathBuf {
    if let Some(p) = std::env::var_os("DEDUPE2_CACHE") {
        return PathBuf::from(p);
    }
    let Some(home) = std::env::var_os("HOME") else {
        return std::env::temp_dir().join("dedupe2-scan-cache.bin");
    };
    let base = PathBuf::from(home);
    if cfg!(target_os = "macos") {
        base.join("Library/Caches/dedupe2/scan-cache.bin")
    } else {
        base.join(".cache/dedupe2/scan-cache.bin")
    }
}

impl ScanCache {
    /// True when something changed since the last `save`.
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Load a previously saved snapshot. Missing, unreadable, corrupt or
    /// version-mismatched files yield an empty cache — never an error.
    /// Validation stays lazy: loaded entries are checked against the
    /// filesystem the first time they are used.
    pub fn load(path: &Path) -> ScanCache {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(_) => return ScanCache::new(),
        };
        // Peek at the version first: bincode isn't self-describing, so each
        // version needs its own parse.
        let version: u32 = match bincode::deserialize::<(u32,)>(&data) {
            Ok((version,)) => version,
            Err(e) => {
                tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                return ScanCache::new();
            }
        };

        match version {
            CACHE_FORMAT_VERSION => {
                let snapshot: PersistedCache = match bincode::deserialize(&data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return ScanCache::new();
                    }
                };
                let cache = ScanCache {
                    root: snapshot.root,
                    stats: snapshot.stats,
                    dirty: false,
                };
                tracing::info!("scan cache loaded: {} files", cache.stats.files);
                cache
            }
            1 => {
                let snapshot: v1::PersistedCache = match bincode::deserialize(&data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return ScanCache::new();
                    }
                };
                let cache = ScanCache {
                    root: v1::convert_node(snapshot.root),
                    stats: snapshot.stats,
                    // Rewrite in the current format on the next autosave.
                    dirty: true,
                };
                tracing::info!(
                    "scan cache loaded from version 1 and migrated: {} files",
                    cache.stats.files
                );
                cache
            }
            other => {
                tracing::info!(
                    "scan cache {} has version {other} (expected {CACHE_FORMAT_VERSION}) — starting fresh",
                    path.display()
                );
                ScanCache::new()
            }
        }
    }



    /// Write the full snapshot (atomically: temp file + rename). Clears the
    /// dirty flag on success.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        // Serialize by value without cloning the tree: take it, wrap it,
        // and always put it back.
        let snapshot = PersistedCache {
            version: CACHE_FORMAT_VERSION,
            root: std::mem::take(&mut self.root),
            stats: self.stats,
        };
        let data = bincode::serialize(&snapshot)
            .map_err(std::io::Error::other)
            .and_then(|data| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let tmp = path.with_extension("bin.tmp");
                std::fs::write(&tmp, &data)?;
                std::fs::rename(&tmp, path)
            });
        self.root = snapshot.root;
        data?;
        self.dirty = false;
        Ok(())
    }
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

/// Perceptual hash through the cache, computed outside the lock.
pub fn cached_phash(cache: &Mutex<ScanCache>, path: &Path) -> Option<Phash> {
    if let Some(value) = lock(cache).phash_lookup(path) {
        return Some(value);
    }
    let value = image_reader::phash(path)?;
    lock(cache).phash_store(path, value);
    Some(value)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_v1(root: &mut v1::DirNode, path: &Path, entry: v1::FileEntry) {
        let mut node = root;
        for comp in path.parent().unwrap().components() {
            node = node
                .children
                .entry(comp.as_os_str().to_os_string())
                .or_insert_with(|| v1::DirNode {
                    children: HashMap::new(),
                    files: HashMap::new(),
                });
        }
        node.files
            .insert(path.file_name().unwrap().to_os_string(), entry);
    }

    /// A real version-1 snapshot (with the sharpness field) must migrate:
    /// loaded entries still validate against the file and serve every layer
    /// from the cache, with no recomputation.
    #[test]
    fn version_1_snapshots_migrate_without_losing_knowledge() {
        let dir = std::env::temp_dir().join(format!("dedupe2-v1-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("cache.bin");
        let file = dir.join("photo.jpg");
        std::fs::write(&file, vec![7u8; 2048]).unwrap();

        let meta = std::fs::metadata(&file).unwrap();
        let entry = v1::FileEntry {
            size: meta.len(),
            mtime: meta.modified().ok(),
            dev: 7,
            shallow: Some((0xABCD, true)),
            full_bytes: Some(11),
            full_content: Some(22),
            pixel: Some(PixelKey {
                width: 10,
                height: 20,
                cpp: 3,
                hash: 33,
            }),
            phash: Some(v1::PhashV1 {
                hash: 44,
                width: 10,
                height: 20,
                sharpness: 0.75,
            }),
            date: Some(CreationDate::DateCreated("2020-01-02".into())),
        };
        let mut root = v1::DirNode {
            children: HashMap::new(),
            files: HashMap::new(),
        };
        insert_v1(&mut root, &file, entry);
        let snapshot = v1::PersistedCache {
            version: 1,
            root,
            stats: CacheStats {
                files: 1,
                reused: 5,
                computed: 6,
            },
        };
        std::fs::write(&store, bincode::serialize(&snapshot).unwrap()).unwrap();

        let mut cache = ScanCache::load(&store);
        assert_eq!(cache.stats().files, 1, "entry survived the migration");
        assert!(cache.dirty(), "migration schedules a rewrite in v2");

        let before = cache.stats();
        assert_eq!(cache.shallow_hash(&file, 64 * 1024), (0xABCD, true));
        assert_eq!(cache.full_content_hash(&file).unwrap(), 22);
        assert_eq!(
            cache.phash(&file).unwrap(),
            Phash {
                hash: 44,
                width: 10,
                height: 20
            },
            "sharpness dropped, everything else intact"
        );
        assert_eq!(
            cache.creation_date(&file),
            Some(CreationDate::DateCreated("2020-01-02".into()))
        );
        assert_eq!(
            cache.stats().computed,
            before.computed,
            "nothing recomputed after migration"
        );

        let _ = std::fs::remove_file(&store);
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
    }
}
