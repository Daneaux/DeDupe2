use std::path::{Path, PathBuf};
use std::time::SystemTime;

use walkdir::WalkDir;

use crate::exif::{creation_date, CreationDate};
use crate::image_reader::{hash_all_bytes, hash_first_n_bytes};
use crate::volumes::{FileType, Volume};

#[derive(Debug, Clone, Copy)]
enum HashKind {
    Header,
    Full,
}

#[derive(Debug, Clone)]
pub struct ScanTarget {
    pub name: String,
    pub paths: Vec<PathBuf>,
    pub volume: Volume,
    pub extensions: Vec<String>,
    pub prefix_bytes: usize,
}

impl ScanTarget {
    pub fn includes_file(&self, path: &Path) -> bool {
        if self.extensions.is_empty() {
            return true;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        self.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext))
    }
}

#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    pub file_type: FileType,
    pub hash: u64,
    pub creation_date: CreationDate,
}

#[derive(Debug, Clone)]
pub struct ScannedTree {
    pub root: PathBuf,
    pub files: Vec<ScannedFile>,
    pub scan_time: SystemTime,
    pub target: ScanTarget,
}

pub fn shallow_scan(target: &ScanTarget) -> ScannedTree {
    scan(target, HashKind::Header)
}

pub fn deep_scan(target: &ScanTarget) -> ScannedTree {
    scan(target, HashKind::Full)
}

fn scan(target: &ScanTarget, kind: HashKind) -> ScannedTree {
    let mut files = Vec::new();
    for path in &target.paths {
        scan_path(path, kind, target, &mut files);
    }

    ScannedTree {
        root: target.volume.path.clone(),
        files,
        scan_time: SystemTime::now(),
        target: target.clone(),
    }
}

fn scan_path(root: &Path, kind: HashKind, target: &ScanTarget, files: &mut Vec<ScannedFile>) {
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
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
        let path = entry.into_path();
        let hash = match kind {
            HashKind::Header => hash_first_n_bytes(&path, target.prefix_bytes).unwrap_or(0),
            HashKind::Full => hash_all_bytes(&path).unwrap_or(0),
        };

        files.push(ScannedFile {
            path: path.clone(),
            size: meta.len(),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            file_type: file_type_of(&path),
            hash,
            creation_date: creation_date(&path),
        });
    }
}

pub(crate) fn file_type_of(path: &Path) -> FileType {
    FileType {
        ext: path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase(),
    }
}
