//! Turning directories into snapshots of their media files.
//!
//! This module owns discovery, cheap stat metadata, and shallow hashing.
//! Comparison consumes the resulting [`TreeSnapshot`]s and only groups
//! hashes; it never walks or hashes a tree itself. Date extraction is a
//! separate, slower pass, so a fast scan does not pay for EXIF.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use rayon::prelude::*;

use crate::image_reader::{collect_image_files, hash_image_data_status, ImageReaderError};
use crate::Scanner::cache::ScanCache;

/// Hash result per file: `decoded` is `true` when the hash came from the
/// decoder, `false` when the file was unreadable and the raw-byte fallback
/// was used.
#[derive(Debug, Clone, Copy)]
pub struct FileHash {
    pub hash: u64,
    pub decoded: bool,
}

/// Hash the first `prefix_bytes` of each file's encoded image data, in
/// parallel. `progress(done, total)` is invoked once per completed file.
pub fn hash_image_data_parallel(
    paths: &[PathBuf],
    prefix_bytes: usize,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<FileHash> {
    let total = paths.len();
    let done = AtomicUsize::new(0);

    paths
        .par_iter()
        .map(|path| {
            let (hash, decoded) = hash_image_data_status(path, prefix_bytes);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            FileHash { hash, decoded }
        })
        .collect()
}

/// One media file as discovered by a fast scan.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// First `prefix_bytes` of encoded image data.
    pub hash: u64,
    /// Whether `hash` came from the decoder (as opposed to the unreadable
    /// raw-byte fallback).
    pub decoded: bool,
}

/// What a scan did: how many files it found, how many shallow hashes came
/// from the shared cache, and how many could not be decoded.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanStats {
    pub files: usize,
    pub reused: usize,
    pub computed: usize,
    pub unreadable: usize,
}

/// The scan of one tree, self-contained: the root, every discovered media
/// file with its metadata and shallow hash, when it was scanned, and what
/// the scan did. Comparison and other passes take this as input.
#[derive(Debug, Clone)]
pub struct TreeSnapshot {
    pub root: PathBuf,
    pub files: Vec<ScannedFile>,
    pub scanned_at: SystemTime,
    pub stats: ScanStats,
}

impl TreeSnapshot {
    pub fn decoded(&self) -> impl Iterator<Item = &ScannedFile> {
        self.files.iter().filter(|file| file.decoded)
    }
}

/// Scan one tree through the optional cache.
pub fn scan_tree(
    root: &Path,
    prefix_bytes: usize,
    cache: Option<&Mutex<ScanCache>>,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<TreeSnapshot, ImageReaderError> {
    let mut trees = scan_trees(&[root.to_path_buf()], prefix_bytes, cache, progress)?;
    Ok(trees.remove(0))
}

/// Scan several trees in one pass so progress covers all of them and each
/// file is hashed exactly once. Snapshots come back in root order.
pub fn scan_trees(
    roots: &[PathBuf],
    prefix_bytes: usize,
    cache: Option<&Mutex<ScanCache>>,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<Vec<TreeSnapshot>, ImageReaderError> {
    let mut entries: Vec<(usize, PathBuf)> = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        for path in collect_image_files(root)? {
            entries.push((index, path));
        }
    }

    let total = entries.len();
    let done = AtomicUsize::new(0);

    let scanned: Vec<(usize, ScannedFile, bool)> = entries
        .par_iter()
        .map(|(index, path)| {
            let meta = std::fs::metadata(path).ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta.as_ref().and_then(|m| m.modified().ok());

            let ((hash, decoded), reused) = match cache {
                Some(cache) => {
                    crate::Scanner::cache::shallow_hash_cached(cache, path, prefix_bytes)
                }
                None => (hash_image_data_status(path, prefix_bytes), false),
            };

            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            (
                *index,
                ScannedFile {
                    path: path.clone(),
                    size,
                    modified,
                    hash,
                    decoded,
                },
                reused,
            )
        })
        .collect();

    let now = SystemTime::now();
    let mut trees: Vec<TreeSnapshot> = roots
        .iter()
        .map(|root| TreeSnapshot {
            root: root.clone(),
            files: Vec::new(),
            scanned_at: now,
            stats: ScanStats::default(),
        })
        .collect();

    // `par_iter().collect()` preserves order, so files stay sorted per root
    // (`collect_image_files` sorts) and snapshots keep the caller's root order.
    for (index, file, reused) in scanned {
        let tree = &mut trees[index];
        tree.stats.files += 1;
        if reused {
            tree.stats.reused += 1;
        } else {
            tree.stats.computed += 1;
        }
        if !file.decoded {
            tree.stats.unreadable += 1;
        }
        tree.files.push(file);
    }

    Ok(trees)
}
