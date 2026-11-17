//! Hierarchical scan cache: mirrors the directory tree so scan results can be
//! reused across passes and invalidated per branch.
//!
//! Each file entry accumulates data as passes run — a shallow scan fills the
//! 64kb hash, a later EXIF pass adds the date, a deep scan adds the full byte
//! / full content / decoded-pixel data. Nothing ever rebuilds entries from
//! scratch: an entry is only reset when the file itself changed (size or
//! mtime), and directories are invalidated branch-wise on request.

mod format;
mod layers;
mod persist;
mod tree;
mod volume;

#[cfg(test)]
mod chroma_tests;
#[cfg(test)]
mod dhash_tests;
#[cfg(test)]
mod regression_tests;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use crate::exif::CreationDate;
use crate::image_reader::Phash;

use format::*;
use volume::*;

pub use layers::{
    cached_phash, creation_dates_cached, full_bytes_cached, full_content_cached, perceptual_cached,
    pixel_cached, shallow_hash_cached, shallow_hashes_cached,
};
pub use persist::default_cache_path;

#[cfg(test)]
use persist::*;

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
    /// Mean chroma 0-255 (colorfulness): separates a B&W copy from its color
    /// original, which pHash alone cannot do.
    chroma: Option<u8>,
    /// Gradient (dHash) signature: a second, independent structure hash that
    /// must agree before two pictures are called similar.
    dhash: Option<u64>,
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
        self.files.len()
            + self
                .children
                .values()
                .map(DirNode::file_count)
                .sum::<usize>()
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

/// How a volume is addressed in the cache tree: its persistent key plus the
/// mount point it currently lives at. Runtime-only — never persisted, so a
/// device id reused after a reboot can never mis-map.
#[derive(Debug, Clone)]
struct VolumeMount {
    mount_point: PathBuf,
    key: OsString,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanCache {
    root: DirNode,
    stats: CacheStats,
    /// dev -> volume key + mount point, resolved once per volume per process.
    #[serde(skip)]
    mounts: HashMap<u64, VolumeMount>,
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
}

/// Lock helper that survives a poisoned mutex (a panic while hashing one
/// file must not disable the cache for the rest of the scan).
pub fn lock(cache: &Mutex<ScanCache>) -> std::sync::MutexGuard<'_, ScanCache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}
