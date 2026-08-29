use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanDepth {
    Shallow,
    Deep,
}

impl ScanDepth {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScanDepth::Shallow => "shallow",
            ScanDepth::Deep => "deep",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileType {
    pub ext: String,
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub name: String,
    pub label: String,
    pub path: PathBuf,
    pub fs: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub scanned: bool,
    pub dirty: bool,
    pub depth: ScanDepth,
    pub subdirs: Vec<String>,
}

impl Volume {
    pub fn new(path: PathBuf) -> Self {
        Self {
            name: path.display().to_string(),
            label: String::new(),
            path,
            fs: String::new(),
            total_bytes: 0,
            used_bytes: 0,
            scanned: false,
            dirty: false,
            depth: ScanDepth::Shallow,
            subdirs: Vec::new(),
        }
    }

    pub fn total_gb(&self) -> u64 {
        self.total_bytes / 1_000_000_000
    }

    pub fn used_gb(&self) -> u64 {
        self.used_bytes / 1_000_000_000
    }

    pub fn used_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            0
        } else {
            (self.used_bytes as f64 / self.total_bytes as f64 * 100.0) as u8
        }
    }

    pub fn path_str(&self) -> String {
        self.path.display().to_string()
    }
}
