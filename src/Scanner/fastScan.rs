use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::Scanner::scanner::{file_type_of, ScanTarget, ScannedFile, ScannedTree};
use crate::exif::creation_date;
use crate::image_reader::hash_image_data_n;

/// Hash the first `prefix_bytes` of each file's encoded image data, in
/// parallel. `progress(done, total)` is invoked once per completed file.
pub fn hash_image_data_parallel(
    paths: &[PathBuf],
    prefix_bytes: usize,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<u64> {
    let total = paths.len();
    let done = AtomicUsize::new(0);

    paths
        .par_iter()
        .map(|path| {
            let hash = hash_image_data_n(path, prefix_bytes);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            hash
        })
        .collect()
}

pub fn fast_scan(target: &ScanTarget) -> ScannedTree {
    fast_scan_with_progress(target, &|_, _| {})
}

pub fn fast_scan_with_progress(
    target: &ScanTarget,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> ScannedTree {
    let files = collect_files(target);
    let prefix_bytes = target.prefix_bytes;

    let paths: Vec<PathBuf> = files.iter().map(|(p, _, _)| p.clone()).collect();
    let hashes = hash_image_data_parallel(&paths, prefix_bytes, progress);

    let scanned_files: Vec<ScannedFile> = files
        .into_iter()
        .zip(hashes)
        .map(|((path, size, modified), hash)| {
            let file_type = file_type_of(&path);
            let creation_date = creation_date(&path);
            ScannedFile {
                path,
                size,
                modified,
                file_type,
                hash,
                creation_date,
            }
        })
        .collect();

    ScannedTree {
        root: target.volume.path.clone(),
        files: scanned_files,
        scan_time: SystemTime::now(),
        target: target.clone(),
    }
}

pub fn collect_files(target: &ScanTarget) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut files = Vec::new();

    for root in &target.paths {
        for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            if !target.includes_file(entry.path()) {
                continue;
            }

            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let modified = meta.modified().unwrap_or(SystemTime::now());

            files.push((entry.into_path(), meta.len(), modified));
        }
    }

    files
}
