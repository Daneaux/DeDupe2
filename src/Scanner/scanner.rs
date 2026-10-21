use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::exif::CreationDate;
use crate::volumes::{FileType, Volume};

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

pub(crate) fn file_type_of(path: &Path) -> FileType {
    FileType {
        ext: path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase(),
    }
}
