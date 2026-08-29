use std::path::{Path, PathBuf};

use super::{FileType, ScanDepth, Volume, VolumeError};

#[derive(Debug, Default)]
pub struct VolumeManager {
    volumes: Vec<Volume>,
    file_types: Vec<FileType>,
}

impl VolumeManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn demo() -> Self {
        Self {
            volumes: vec![
                Volume {
                    name: "C:".into(),
                    label: "Windows".into(),
                    path: "C:\\".into(),
                    fs: "NTFS".into(),
                    total_bytes: 476_000_000_000,
                    used_bytes: 312_000_000_000,
                    scanned: true,
                    dirty: false,
                    depth: ScanDepth::Shallow,
                    subdirs: vec![
                        "Users".into(),
                        "Program Files".into(),
                        "Windows".into(),
                        "ProgramData".into(),
                    ],
                },
                Volume {
                    name: "D:".into(),
                    label: "Data".into(),
                    path: "D:\\".into(),
                    fs: "NTFS".into(),
                    total_bytes: 1_863_000_000_000,
                    used_bytes: 940_000_000_000,
                    scanned: true,
                    dirty: true,
                    depth: ScanDepth::Deep,
                    subdirs: vec![
                        "Documents".into(),
                        "Media".into(),
                        "Projects".into(),
                        "Downloads".into(),
                    ],
                },
                Volume {
                    name: "E:".into(),
                    label: "Backup".into(),
                    path: "E:\\".into(),
                    fs: "exFAT".into(),
                    total_bytes: 3_726_000_000_000,
                    used_bytes: 2_100_000_000_000,
                    scanned: false,
                    dirty: false,
                    depth: ScanDepth::Shallow,
                    subdirs: vec!["Backups".into(), "Archives".into()],
                },
            ],
            file_types: vec![
                FileType { ext: "jpg".into() },
                FileType { ext: "png".into() },
                FileType { ext: "mp4".into() },
                FileType { ext: "pdf".into() },
                FileType { ext: "docx".into() },
            ],
        }
    }

    pub fn volumes(&self) -> &[Volume] {
        &self.volumes
    }

    pub fn file_types(&self) -> &[FileType] {
        &self.file_types
    }

    pub fn is_scanned(&self, path: &Path) -> bool {
        self.find(path).map(|i| self.volumes[i].scanned).unwrap_or(false)
    }

    pub fn is_dirty(&self, path: &Path) -> bool {
        self.find(path).map(|i| self.volumes[i].dirty).unwrap_or(false)
    }

    pub fn add_volume(&mut self, path: impl Into<PathBuf>) -> Result<(), VolumeError> {
        let path = path.into();
        if self.find(&path).is_some() {
            return Err(VolumeError::AlreadyExists(path.display().to_string()));
        }
        self.volumes.push(Volume::new(path));
        Ok(())
    }

    pub fn remove_volume(&mut self, path: &Path) -> Result<(), VolumeError> {
        let idx = self
            .find(path)
            .ok_or_else(|| VolumeError::NotFound(path.display().to_string()))?;
        self.volumes.remove(idx);
        Ok(())
    }

    pub async fn detect_volumes(&mut self) -> Result<Vec<Volume>, VolumeError> {
        todo!("enumerate connected volumes")
    }

    pub async fn scan(&mut self, path: &Path, depth: ScanDepth) -> Result<(), VolumeError> {
        todo!("scan volume {} at {depth:?}", path.display())
    }

    pub async fn rescan(&mut self, path: &Path) -> Result<(), VolumeError> {
        todo!("rescan volume {}", path.display())
    }

    pub async fn scan_subtree(&mut self, path: &Path, subdir: &Path) -> Result<(), VolumeError> {
        todo!("scan subtree {} of {}", subdir.display(), path.display())
    }

    pub fn mark_dirty(&mut self, path: &Path) -> Result<(), VolumeError> {
        let idx = self
            .find(path)
            .ok_or_else(|| VolumeError::NotFound(path.display().to_string()))?;
        self.volumes[idx].dirty = true;
        Ok(())
    }

    pub fn clear_dirty(&mut self, path: &Path) -> Result<(), VolumeError> {
        let idx = self
            .find(path)
            .ok_or_else(|| VolumeError::NotFound(path.display().to_string()))?;
        self.volumes[idx].dirty = false;
        Ok(())
    }

    pub fn add_file_type(&mut self, ext: impl Into<String>) -> Result<(), VolumeError> {
        let ext = ext.into();
        if self.file_types.iter().any(|f| f.ext == ext) {
            return Err(VolumeError::AlreadyExists(ext));
        }
        self.file_types.push(FileType { ext });
        Ok(())
    }

    pub fn remove_file_type(&mut self, ext: &str) -> Result<(), VolumeError> {
        let idx = self
            .file_types
            .iter()
            .position(|f| f.ext == ext)
            .ok_or_else(|| VolumeError::NotFound(ext.to_string()))?;
        self.file_types.remove(idx);
        Ok(())
    }

    fn find(&self, path: &Path) -> Option<usize> {
        self.volumes.iter().position(|v| v.path == path)
    }
}
