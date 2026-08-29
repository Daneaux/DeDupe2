use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use walkdir::WalkDir;

use crate::volumes::{FileType, Volume};

pub const HEADER_HASH_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ScanTarget {
    pub name: String,
    pub paths: Vec<PathBuf>,
    pub volume: Volume,
}

#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    pub file_type: FileType,
    pub hash64kb: String,
}

#[derive(Debug, Clone)]
pub struct ScannedTree {
    pub root: PathBuf,
    pub files: Vec<ScannedFile>,
    pub scan_time: SystemTime,
    pub target: ScanTarget,
}

pub fn shallow_scan(target: &ScanTarget) -> ScannedTree {
    let mut files = Vec::new();
    for path in &target.paths {
        scan_path(path, &mut files);
    }

    ScannedTree {
        root: target.volume.path.clone(),
        files,
        scan_time: SystemTime::now(),
        target: target.clone(),
    }
}

fn scan_path(root: &Path, files: &mut Vec<ScannedFile>) {
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let path = entry.into_path();

        files.push(ScannedFile {
            path: path.clone(),
            size: meta.len(),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            file_type: file_type_of(&path),
            hash64kb: hash_header(&path),
        });
    }
}

fn file_type_of(path: &Path) -> FileType {
    FileType {
        ext: path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase(),
    }
}

fn hash_header(path: &Path) -> String {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return String::new(),
    };

    let mut buf = [0u8; HEADER_HASH_BYTES];
    let read = file.read(&mut buf).unwrap_or(0);
    format!("{:016x}", seahash::hash(&buf[..read]))
}
