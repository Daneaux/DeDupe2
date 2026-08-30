use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::Scanner::scanner::{file_type_of, ScanTarget, ScannedFile, ScannedTree};

pub const HEADER_READ_BYTES: usize = 64 * 1024;

pub fn fast_scan(target: &ScanTarget) -> ScannedTree {
    let files = collect_files(target);

    let scanned_files: Vec<ScannedFile> = files
        .par_iter()
        .filter_map(|(path, size, modified)| {
            read_header_hash(path).map(|hash| ScannedFile {
                path: path.clone(),
                size: *size,
                modified: *modified,
                file_type: file_type_of(path),
                hash: format!("{:016x}", hash),
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

pub fn read_header_hash(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let mut buffer = [0u8; HEADER_READ_BYTES];
    let bytes_read = file.read(&mut buffer).unwrap_or(0);
    Some(seahash::hash(&buffer[..bytes_read]))
}
