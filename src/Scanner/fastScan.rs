use std::path::PathBuf;
use std::time::SystemTime;

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::Scanner::scanner::{file_type_of, ScanTarget, ScannedFile, ScannedTree};
use crate::exif::creation_date;
use crate::image_reader::hash_first_n_bytes;

pub fn fast_scan(target: &ScanTarget) -> ScannedTree {
    let files = collect_files(target);
    let prefix_bytes = target.prefix_bytes;

    let scanned_files: Vec<ScannedFile> = files
        .par_iter()
        .filter_map(|(path, size, modified)| {
            hash_first_n_bytes(path, prefix_bytes)
                .ok()
                .map(|hash| ScannedFile {
                    path: path.clone(),
                    size: *size,
                    modified: *modified,
                    file_type: file_type_of(path),
                    hash,
                    creation_date: creation_date(path),
                })
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
