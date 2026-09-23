//! Hierarchical scan cache: mirrors the directory tree so scan results can be
//! reused across passes and invalidated per branch.
//!
//! Each file entry accumulates data as passes run — a shallow scan fills the
//! 64kb hash, a later EXIF pass adds the date, a deep scan adds the full byte
//! / full content / decoded-pixel data. Nothing ever rebuilds entries from
//! scratch: an entry is only reset when the file itself changed (size or
//! mtime), and directories are invalidated branch-wise on request.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::exif::CreationDate;
use crate::image_reader::{self, ImageData, ImageReaderError, Phash, PixelData};

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
        self.files.len() + self.children.values().map(DirNode::file_count).sum::<usize>()
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

    /// Volume key + mount point for a device id, resolved once per volume
    /// per process (diskutil on first sight).
    fn volume_mount(&mut self, path: &Path, dev: u64) -> Option<VolumeMount> {
        if let Some(mount) = self.mounts.get(&dev) {
            if path.starts_with(&mount.mount_point) {
                return Some(mount.clone());
            }
        }
        let mount_point = mount_point_of(path, dev);
        let key = self
            .mounts
            .get(&dev)
            .map(|mount| mount.key.clone())
            .unwrap_or_else(|| volume_key_for(&mount_point));
        let mount = VolumeMount { mount_point, key };
        self.mounts.insert(dev, mount.clone());
        Some(mount)
    }

    /// The persistent volume key for a path's volume (resolving if needed):
    /// `vol:<uuid>` when the volume reports one, else a mount-point fallback.
    pub fn volume_key(&mut self, path: &Path) -> Option<String> {
        let meta = std::fs::metadata(path).ok()?;
        let dev = device_of(&meta);
        self.volume_mount(path, dev)
            .map(|mount| mount.key.to_string_lossy().into_owned())
    }

    /// Tree components for an absolute path: `[volume-key, relative, path]`.
    fn key_components(&mut self, path: &Path, dev: u64) -> Option<(OsString, Vec<OsString>)> {
        let mount = self.volume_mount(path, dev)?;
        let rel = path.strip_prefix(&mount.mount_point).ok()?;
        let name = rel.file_name()?.to_os_string();
        let mut dirs = vec![mount.key];
        let mut parent = rel.parent();
        let mut stack = Vec::new();
        while let Some(p) = parent {
            if let Some(component) = p.file_name() {
                stack.push(component.to_os_string());
            }
            parent = p.parent();
        }
        stack.reverse();
        dirs.extend(stack);
        Some((name, dirs))
    }

    /// Tree components for the legacy (absolute-path) branch.
    fn legacy_components(path: &Path) -> (OsString, Vec<OsString>) {
        let name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| OsString::from("file"));
        let mut dirs = vec![OsString::from(LEGACY_ROOT)];
        dirs.extend(
            path.parent()
                .map(|p| {
                    p.components()
                        .map(|c| c.as_os_str().to_os_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        );
        (name, dirs)
    }

    /// Drop everything cached under `dir` (its own entries and all children).
    pub fn invalidate_branch(&mut self, dir: &Path) {
        // Volume-keyed branch.
        if let Some(mount) = self.mount_for_path(dir) {
            if let Some(dirs) = Self::dir_components_for_mount(&mount, dir) {
                if let Some(node) = Self::node_comps_existing_mut(&mut self.root, &dirs) {
                    let dropped = node.file_count();
                    self.stats.files = self.stats.files.saturating_sub(dropped);
                    *node = DirNode::default();
                    self.dirty = true;
                }
            }
        }
        // Legacy branch, keyed by the absolute path.
        let mut legacy_dirs = vec![OsString::from(LEGACY_ROOT)];
        legacy_dirs.extend(
            dir.components()
                .map(|c| c.as_os_str().to_os_string())
                .collect::<Vec<_>>(),
        );
        if let Some(node) = Self::node_comps_existing_mut(&mut self.root, &legacy_dirs) {
            let dropped = node.file_count();
            self.stats.files = self.stats.files.saturating_sub(dropped);
            *node = DirNode::default();
            self.dirty = true;
        }
    }

    /// How many cached file entries live under `path` — the subtree count for
    /// a directory, 1 for a cached file, 0 for anything never scanned.
    pub fn branch_files(&mut self, path: &Path) -> usize {
        self.branch_coverage(path).files
    }

    /// Per-layer coverage of the subtree at `path`: how many of its files
    /// have shallow hashes, deep data, phash, and dates cached. Counts both
    /// the volume-keyed branch and any not-yet-promoted legacy entries.
    pub fn branch_coverage(&mut self, path: &Path) -> BranchCoverage {
        let mut coverage = BranchCoverage::default();

        if let Some(mount) = self.mount_for_path(path) {
            if let Some((name, dirs)) = Self::key_components_for_mount(&mount, path) {
                accumulate_at(&self.root, &dirs, &name, &mut coverage);
            }
        }
        let (legacy_name, legacy_dirs) = Self::legacy_components(path);
        accumulate_at(&self.root, &legacy_dirs, &legacy_name, &mut coverage);

        coverage
    }

    /// Drop a single file's entry.
    pub fn invalidate_file(&mut self, path: &Path) {
        self.remove_entry(path);
    }

    /// Remove an entry without materializing any directory nodes. Handles
    /// both key spaces: the volume-keyed branch (when the path's volume can
    /// still be determined from known mounts) and the legacy branch.
    fn remove_entry(&mut self, path: &Path) {
        let mut removed = false;
        if let Some(mount) = self.mount_for_path(path) {
            if let Some((name, dirs)) = Self::key_components_for_mount(&mount, path) {
                if let Some(node) = Self::node_comps_existing_mut(&mut self.root, &dirs) {
                    removed |= node.files.remove(&name).is_some();
                }
            }
        }
        let (legacy_name, legacy_dirs) = Self::legacy_components(path);
        if let Some(node) = Self::node_comps_existing_mut(&mut self.root, &legacy_dirs) {
            removed |= node.files.remove(&legacy_name).is_some();
        }
        if removed {
            self.stats.files = self.stats.files.saturating_sub(1);
            self.dirty = true;
        }
    }

    /// Walk to an existing node; never creates nodes as a side effect.
    fn node_comps_existing_mut<'t>(
        node: &'t mut DirNode,
        comps: &[OsString],
    ) -> Option<&'t mut DirNode> {
        let mut node = node;
        for name in comps {
            node = node.children.get_mut(name)?;
        }
        Some(node)
    }

    fn node_comps_mut<'t>(node: &'t mut DirNode, comps: &[OsString]) -> Option<&'t mut DirNode> {
        let mut node = node;
        for name in comps {
            node = node.children.entry(name.clone()).or_default();
        }
        Some(node)
    }

    fn node_comps_ref<'t>(node: &'t DirNode, comps: &[OsString]) -> Option<&'t DirNode> {
        let mut node = node;
        for name in comps {
            node = node.children.get(name)?;
        }
        Some(node)
    }

    /// Directory components (no file split) for a path under a known mount.
    fn dir_components_for_mount(mount: &VolumeMount, dir: &Path) -> Option<Vec<OsString>> {
        let rel = dir.strip_prefix(&mount.mount_point).ok()?;
        let mut dirs = vec![mount.key.clone()];
        dirs.extend(
            rel.components()
                .map(|c| c.as_os_str().to_os_string())
                .collect::<Vec<_>>(),
        );
        Some(dirs)
    }

    /// Components for a path that is known to belong to a mounted volume.
    fn key_components_for_mount(mount: &VolumeMount, path: &Path) -> Option<(OsString, Vec<OsString>)> {
        let rel = path.strip_prefix(&mount.mount_point).ok()?;
        let name = rel.file_name()?.to_os_string();
        let mut dirs = vec![mount.key.clone()];
        let mut stack = Vec::new();
        let mut parent = rel.parent();
        while let Some(p) = parent {
            if let Some(component) = p.file_name() {
                stack.push(component.to_os_string());
            }
            parent = p.parent();
        }
        stack.reverse();
        dirs.extend(stack);
        Some((name, dirs))
    }

    /// The known volume whose mount point prefixes `path`, if any.
    fn mount_for_path(&self, path: &Path) -> Option<VolumeMount> {
        self.mounts
            .values()
            .filter(|mount| path.starts_with(&mount.mount_point))
            .max_by_key(|mount| mount.mount_point.as_os_str().len())
            .cloned()
    }

    fn with_entry<T: Clone>(
        &mut self,
        path: &Path,
        mut get: impl FnMut(&FileEntry) -> Option<T>,
        set: impl FnOnce(&mut FileEntry, T),
        compute: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(_) => {
                // The file no longer exists: drop its entry so the cache
                // never holds data for paths that are gone.
                self.remove_entry(path);
                return None;
            }
        };
        let size = meta.len();
        let mtime = meta.modified().ok();
        let dev = device_of(&meta);
        // The volume key separates drives structurally: a different drive
        // simply has a different branch, so validation stays size+mtime.
        let (name, dirs) = self.key_components(path, dev)?;

        // Validate (and reset when the file changed) at the volume-keyed
        // location; then serve the requested layer if present.
        {
            let node = Self::node_comps_mut(&mut self.root, &dirs)?;
            let entry = match node.files.entry(name.clone()) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(FileEntry {
                        size,
                        mtime,
                        dev,
                        ..Default::default()
                    });
                    self.stats.files += 1;
                    node.files.get_mut(&name).expect("just inserted")
                }
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    if !slot.get().matches(size, mtime) {
                        *slot.get_mut() = FileEntry {
                            size,
                            mtime,
                            dev,
                            ..Default::default()
                        };
                        self.dirty = true;
                    }
                    slot.into_mut()
                }
            };
            if let Some(value) = get(entry) {
                self.stats.reused += 1;
                return Some(value);
            }
        }

        // Miss: an entry migrated into the `legacy` branch (its volume was
        // unmounted at migration time) is promoted here, no recomputation.
        self.promote_legacy(path, size, mtime, dev, &dirs, &name);
        {
            let node = Self::node_comps_mut(&mut self.root, &dirs)?;
            if let Some(entry) = node.files.get(&name) {
                if let Some(value) = get(entry) {
                    self.stats.reused += 1;
                    return Some(value);
                }
            }
        }

        let value = compute()?;
        self.stats.computed += 1;
        let node = Self::node_comps_mut(&mut self.root, &dirs)?;
        let entry = node.files.entry(name).or_default();
        set(entry, value.clone());
        Some(value)
    }

    /// Move a legacy (absolute-path) entry to its volume-keyed location when
    /// one exists and still validates.
    fn promote_legacy(
        &mut self,
        path: &Path,
        size: u64,
        mtime: Option<SystemTime>,
        _dev: u64,
        dirs: &[OsString],
        name: &OsString,
    ) {
        let (legacy_name, legacy_dirs) = Self::legacy_components(path);
        let taken = {
            let Some(node) = Self::node_comps_existing_mut(&mut self.root, &legacy_dirs) else {
                return;
            };
            match node.files.get(&legacy_name) {
                Some(entry) if entry.matches(size, mtime) => {
                    let entry = node.files.remove(&legacy_name).expect("checked");
                    self.stats.files = self.stats.files.saturating_sub(1);
                    Some(entry)
                }
                _ => None,
            }
        };
        let Some(entry) = taken else { return };
        self.dirty = true;
        let Some(node) = Self::node_comps_mut(&mut self.root, &dirs) else {
            return;
        };
        match node.files.entry(name.clone()) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                self.stats.files += 1;
                slot.insert(entry);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                *slot.get_mut() = entry;
            }
        }
    }

    /// 64kb encoded-image-data hash (with decoded flag), cached.
    pub fn shallow_hash(&mut self, path: &Path, n: usize) -> (u64, bool) {
        if let Some(v) = self.shallow_hash_lookup(path) {
            return v;
        }
        let v = image_reader::hash_image_data_status(path, n);
        self.shallow_hash_store(path, v);
        v
    }

    /// Non-computing shallow lookup (still applies the stat-based reset).
    pub fn shallow_hash_lookup(&mut self, path: &Path) -> Option<(u64, bool)> {
        self.with_entry(path, |e| e.shallow, |_e, _v| {}, || None)
    }

    pub fn shallow_hash_store(&mut self, path: &Path, value: (u64, bool)) {
        self.store_layer(path, value, |entry| &mut entry.shallow, |entry, v| entry.shallow = Some(v));
    }

    /// Whole-file byte hash, cached on success.
    pub fn full_bytes_hash(&mut self, path: &Path) -> Result<u64, ImageReaderError> {
        if let Some(v) = self.full_bytes_lookup(path) {
            return Ok(v);
        }
        let v = image_reader::hash_all_bytes(path)?;
        self.full_bytes_store(path, v);
        Ok(v)
    }

    pub fn full_bytes_lookup(&mut self, path: &Path) -> Option<u64> {
        self.with_entry(path, |e| e.full_bytes, |_e, _v| {}, || None)
    }

    pub fn full_bytes_store(&mut self, path: &Path, value: u64) {
        self.store_layer(path, value, |entry| &mut entry.full_bytes, |entry, v| entry.full_bytes = Some(v));
    }

    /// Full encoded-image-data hash (deep scan step 2), cached on success.
    pub fn full_content_hash(&mut self, path: &Path) -> Result<u64, ImageReaderError> {
        if let Some(v) = self.full_content_lookup(path) {
            return Ok(v);
        }
        let v = image_reader::hash_image_data_all(path)?;
        self.full_content_store(path, v);
        Ok(v)
    }

    pub fn full_content_lookup(&mut self, path: &Path) -> Option<u64> {
        self.with_entry(path, |e| e.full_content, |_e, _v| {}, || None)
    }

    pub fn full_content_store(&mut self, path: &Path, value: u64) {
        self.store_layer(path, value, |entry| &mut entry.full_content, |entry, v| entry.full_content = Some(v));
    }

    /// Decoded-pixel key (deep scan step 3). Decode failures are not cached.
    pub fn pixel_key(&mut self, path: &Path) -> Option<PixelKey> {
        if let Some(v) = self.pixel_lookup(path) {
            return Some(v);
        }
        let v = pixel_key_of(path)?;
        self.pixel_store(path, v);
        Some(v)
    }

    pub fn pixel_lookup(&mut self, path: &Path) -> Option<PixelKey> {
        self.with_entry(path, |e| e.pixel, |_e, _v| {}, || None)
    }

    pub fn pixel_store(&mut self, path: &Path, value: PixelKey) {
        self.store_layer(path, value, |entry| &mut entry.pixel, |entry, v| entry.pixel = Some(v));
    }

    /// Perceptual hash, cached on success.
    pub fn phash(&mut self, path: &Path) -> Option<Phash> {
        if let Some(v) = self.phash_lookup(path) {
            return Some(v);
        }
        let v = image_reader::phash(path)?;
        self.phash_store(path, v);
        Some(v)
    }

    pub fn phash_lookup(&mut self, path: &Path) -> Option<Phash> {
        self.with_entry(path, |e| e.phash, |_e, _v| {}, || None)
    }

    pub fn phash_store(&mut self, path: &Path, value: Phash) {
        self.store_layer(path, value, |entry| &mut entry.phash, |entry, v| entry.phash = Some(v));
    }

    /// Mean chroma, cached on success.
    pub fn chroma(&mut self, path: &Path) -> Option<u8> {
        if let Some(v) = self.chroma_lookup(path) {
            return Some(v);
        }
        let image = image_reader::read_image(path).ok()?;
        let v = image_reader::mean_chroma(&image)?;
        self.chroma_store(path, v);
        Some(v)
    }

    pub fn chroma_lookup(&mut self, path: &Path) -> Option<u8> {
        self.with_entry(path, |e| e.chroma, |_e, _v| {}, || None)
    }

    pub fn chroma_store(&mut self, path: &Path, value: u8) {
        self.store_layer(path, value, |entry| &mut entry.chroma, |entry, v| entry.chroma = Some(v));
    }

    /// Cached creation date (a cached `Unknown` is a real answer, so date
    /// presence itself is the cache hit).
    pub fn creation_date(&mut self, path: &Path) -> Option<CreationDate> {
        self.with_entry(path, |e| e.date.clone(), |_e, _v| {}, || None)
    }

    /// Store a value into one layer, counting a first-time store as computed.
    fn store_layer<T: Clone>(
        &mut self,
        path: &Path,
        value: T,
        layer: impl FnOnce(&mut FileEntry) -> &mut Option<T>,
        set: impl FnOnce(&mut FileEntry, T),
    ) {
        // Never (re-)create an entry for a file that no longer exists.
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(_) => return,
        };
        let size = meta.len();
        let mtime = meta.modified().ok();
        let dev = device_of(&meta);
        let Some((name, dirs)) = self.key_components(path, dev) else {
            return;
        };
        self.dirty = true;
        let node = match Self::node_comps_mut(&mut self.root, &dirs) {
            Some(node) => node,
            None => return,
        };
        let entry = match node.files.entry(name) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                self.stats.files += 1;
                self.stats.computed += 1;
                slot.insert(FileEntry {
                    size,
                    mtime,
                    dev,
                    ..Default::default()
                })
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                if layer(slot.get_mut()).is_none() {
                    self.stats.computed += 1;
                }
                slot.into_mut()
            }
        };
        set(entry, value);
    }

    pub fn store_creation_date(&mut self, path: &Path, date: CreationDate) {
        self.store_layer(
            path,
            date,
            |entry| &mut entry.date,
            |entry, date| entry.date = Some(date),
        );
    }
}

/// The mount point of `path`: walk up while the device stays the same.
fn mount_point_of(path: &Path, dev: u64) -> PathBuf {
    let mut mount = path;
    for ancestor in path.ancestors().skip(1) {
        match std::fs::metadata(ancestor) {
            Ok(meta) if device_of(&meta) == dev => mount = ancestor,
            _ => break,
        }
    }
    mount.to_path_buf()
}

/// Persistent volume UUID for a mount point. macOS: `diskutil info`
/// ("Volume UUID" survives remounts, unlike st_dev). Other platforms return
/// None and the mount path itself is used as the key.
///
/// TODO: SMB/mounted shares — derive the key from server + share name so
/// those volumes are stable across mount points too.
#[cfg(target_os = "macos")]
fn volume_uuid_for_mount(mount: &Path) -> Option<String> {
    let output = std::process::Command::new("diskutil")
        .args(["info"])
        .arg(mount)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("Volume UUID:") {
            let uuid = rest.trim();
            if !uuid.is_empty() {
                return Some(uuid.to_string());
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn volume_uuid_for_mount(_mount: &Path) -> Option<String> {
    None
}

/// All currently mounted volumes (mount point + key), for migration: entries
/// are re-keyed by matching their old absolute paths against these prefixes.
fn mounted_volumes() -> Vec<(PathBuf, OsString)> {
    let mut mounts = vec![(PathBuf::from("/"), volume_key_for(Path::new("/")))];
    if let Ok(entries) = std::fs::read_dir("/Volumes") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                mounts.push((path.clone(), volume_key_for(&path)));
            }
        }
    }
    // Longest mount point first so nested mounts win the prefix match.
    mounts.sort_by_key(|(mount, _)| std::cmp::Reverse(mount.as_os_str().len()));
    mounts
}

/// Key for a mount point: its UUID, or the mount path as a fallback (see the
/// SMB TODO on `volume_uuid_for_mount`).
fn volume_key_for(mount: &Path) -> OsString {
    match volume_uuid_for_mount(mount) {
        Some(uuid) => OsString::from(format!("vol:{uuid}")),
        None => OsString::from(format!("mount:{}", mount.display())),
    }
}

/// First component reserved for entries that could not be re-keyed at
/// migration time (their volume was not mounted). Lookups promote them lazily.
const LEGACY_ROOT: &str = "legacy";

#[cfg(unix)]
fn device_of(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.dev()
}

#[cfg(not(unix))]
fn device_of(_meta: &std::fs::Metadata) -> u64 {
    0
}


/// On-disk snapshot version. Bump when the entry shape changes; older files
/// are migrated when the difference is lossless, ignored otherwise.
const CACHE_FORMAT_VERSION: u32 = 5;

// ---- version 1 compatibility -------------------------------------------
// v1 stored a `sharpness` estimate inside Phash (a heuristic that was
// removed). Everything else is identical, so a v1 snapshot converts by
// dropping that one field — no cache knowledge is lost.
// Remove after the next format bump.
mod v1 {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::time::SystemTime;

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct PersistedCache {
        pub version: u32,
        pub root: DirNode,
        pub stats: CacheStats,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct DirNode {
        pub children: HashMap<OsString, DirNode>,
        pub files: HashMap<OsString, FileEntry>,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct FileEntry {
        pub size: u64,
        pub mtime: Option<SystemTime>,
        pub dev: u64,
        pub shallow: Option<(u64, bool)>,
        pub full_bytes: Option<u64>,
        pub full_content: Option<u64>,
        pub pixel: Option<PixelKey>,
        pub phash: Option<PhashV1>,
        pub date: Option<CreationDate>,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct PhashV1 {
        pub hash: u64,
        pub width: u32,
        pub height: u32,
        #[allow(dead_code)]
        pub sharpness: f32,
    }

    pub fn convert_node(node: DirNode) -> DirNodeSuper {
        DirNodeSuper {
            children: node
                .children
                .into_iter()
                .map(|(name, child)| (name, convert_node(child)))
                .collect(),
            files: node
                .files
                .into_iter()
                .map(|(name, entry)| (name, convert_entry(entry)))
                .collect(),
        }
    }

    fn convert_entry(entry: FileEntry) -> FileEntrySuper {
        FileEntrySuper {
            size: entry.size,
            mtime: entry.mtime,
            dev: entry.dev,
            shallow: entry.shallow,
            full_bytes: entry.full_bytes,
            full_content: entry.full_content,
            pixel: entry.pixel,
            phash: entry.phash.map(|p| Phash {
                hash: p.hash,
                width: p.width,
                height: p.height,
            }),
            chroma: None,
            date: entry.date,
        }
    }

    type DirNodeSuper = super::DirNode;
    type FileEntrySuper = super::FileEntry;
}

/// The persisted form is the cache structure itself, serialized with serde
/// through a binary codec — no hand-translated shape, so non-UTF8 path
/// components and every learned layer (shallow hash, full hashes, pixel key,
/// phash, dates, device id) round-trip losslessly.
#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedCache {
    version: u32,
    root: DirNode,
    stats: CacheStats,
}

/// v2/v3 snapshots: same tree, absolute-path keyed; v3 additionally carried a
/// dev -> UUID map that v4 does not need (the volume key is in the tree).
mod v2 {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct PersistedCache {
        pub version: u32,
        pub root: DirNode,
        pub stats: CacheStats,
    }
}

mod v3 {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct PersistedCache {
        pub version: u32,
        pub root: DirNode,
        pub stats: CacheStats,
        #[allow(dead_code)]
        pub volume_ids: HashMap<u64, String>,
    }
}

/// v4: volume-keyed tree, entries without the chroma layer.
mod v4 {
    use super::*;
    use std::ffi::OsString;
    use std::time::SystemTime;

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct PersistedCache {
        pub version: u32,
        pub root: DirNode,
        pub stats: CacheStats,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct DirNode {
        pub children: HashMap<OsString, DirNode>,
        pub files: HashMap<OsString, FileEntry>,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct FileEntry {
        pub size: u64,
        pub mtime: Option<SystemTime>,
        pub dev: u64,
        pub shallow: Option<(u64, bool)>,
        pub full_bytes: Option<u64>,
        pub full_content: Option<u64>,
        pub pixel: Option<PixelKey>,
        pub phash: Option<Phash>,
        pub date: Option<CreationDate>,
    }

    pub fn convert_node(node: DirNode) -> DirNodeSuper {
        DirNodeSuper {
            children: node
                .children
                .into_iter()
                .map(|(name, child)| (name, convert_node(child)))
                .collect(),
            files: node
                .files
                .into_iter()
                .map(|(name, entry)| (name, convert_entry(entry)))
                .collect(),
        }
    }

    fn convert_entry(entry: FileEntry) -> FileEntrySuper {
        FileEntrySuper {
            size: entry.size,
            mtime: entry.mtime,
            dev: entry.dev,
            shallow: entry.shallow,
            full_bytes: entry.full_bytes,
            full_content: entry.full_content,
            pixel: entry.pixel,
            phash: entry.phash,
            chroma: None,
            date: entry.date,
        }
    }

    type DirNodeSuper = super::DirNode;
    type FileEntrySuper = super::FileEntry;
}

/// Where the cache persists by default: `$DEDUPE2_CACHE`, else the platform
/// cache directory (`~/Library/Caches/dedupe2/…` on macOS, `~/.cache/…`
/// elsewhere), else the temp directory.
pub fn default_cache_path() -> PathBuf {
    if let Some(p) = std::env::var_os("DEDUPE2_CACHE") {
        return PathBuf::from(p);
    }
    let Some(home) = std::env::var_os("HOME") else {
        return std::env::temp_dir().join("dedupe2-scan-cache.bin");
    };
    let base = PathBuf::from(home);
    if cfg!(target_os = "macos") {
        base.join("Library/Caches/dedupe2/scan-cache.bin")
    } else {
        base.join(".cache/dedupe2/scan-cache.bin")
    }
}

impl ScanCache {
    /// True when something changed since the last `save`.
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Load a previously saved snapshot. Missing, unreadable, corrupt or
    /// version-mismatched files yield an empty cache — never an error.
    /// Validation stays lazy: loaded entries are checked against the
    /// filesystem the first time they are used.
    pub fn load(path: &Path) -> ScanCache {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(_) => return ScanCache::new(),
        };
        // Peek at the version first: bincode isn't self-describing, so each
        // version needs its own parse.
        let version: u32 = match bincode::deserialize::<(u32,)>(&data) {
            Ok((version,)) => version,
            Err(e) => {
                tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                return ScanCache::new();
            }
        };

        // After a successful load, pre-resolve currently mounted volumes so
        // coverage queries and validations take the fast path.
        let mut cache = match version {
            CACHE_FORMAT_VERSION => {
                let snapshot: PersistedCache = match bincode::deserialize(&data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return ScanCache::new();
                    }
                };
                ScanCache {
                    root: snapshot.root,
                    stats: snapshot.stats,
                    ..ScanCache::default()
                }
            }
            4 => {
                let snapshot: v4::PersistedCache = match bincode::deserialize(&data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return ScanCache::new();
                    }
                };
                let root = v4::convert_node(snapshot.root);
                let files = root.file_count();
                tracing::info!(
                    "scan cache {} migrated from version 4: {files} files",
                    path.display()
                );
                ScanCache {
                    root,
                    stats: CacheStats { files, ..snapshot.stats },
                    ..ScanCache::default()
                }
                .with_dirty()
            }
            3 => {
                let snapshot: v3::PersistedCache = match bincode::deserialize(&data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return ScanCache::new();
                    }
                };
                migrate_absolute_tree(snapshot.root, snapshot.stats, path)
            }
            2 => {
                let snapshot: v2::PersistedCache = match bincode::deserialize(&data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return ScanCache::new();
                    }
                };
                migrate_absolute_tree(snapshot.root, snapshot.stats, path)
            }
            1 => {
                let snapshot: v1::PersistedCache = match bincode::deserialize(&data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return ScanCache::new();
                    }
                };
                let stats = snapshot.stats;
                migrate_absolute_tree(v1::convert_node(snapshot.root), stats, path)
            }
            other => {
                tracing::info!(
                    "scan cache {} has version {other} (expected {CACHE_FORMAT_VERSION}) — starting fresh",
                    path.display()
                );
                return ScanCache::new();
            }
        };

        cache.refresh_mounts();
        tracing::info!("scan cache loaded: {} files", cache.stats.files);
        cache
    }

    /// Re-key an old absolute-path tree against the currently mounted volumes
    /// and rewrite the counters. Nothing is discarded: unmounted volumes'
    /// entries land under `legacy` for lazy promotion.
    fn refresh_mounts(&mut self) {
        for (mount_point, key) in mounted_volumes() {
            if let Ok(meta) = std::fs::metadata(&mount_point) {
                let dev = device_of(&meta);
                self.mounts
                    .entry(dev)
                    .or_insert(VolumeMount { mount_point, key });
            }
        }
    }

    /// Write the full snapshot (atomically: temp file + rename). Clears the
    /// dirty flag on success.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        // Serialize by value without cloning the tree: take it, wrap it,
        // and always put it back.
        let snapshot = PersistedCache {
            version: CACHE_FORMAT_VERSION,
            root: std::mem::take(&mut self.root),
            stats: self.stats,
        };
        let data = bincode::serialize(&snapshot)
            .map_err(std::io::Error::other)
            .and_then(|data| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let tmp = path.with_extension("bin.tmp");
                std::fs::write(&tmp, &data)?;
                std::fs::rename(&tmp, path)
            });
        self.root = snapshot.root;
        data?;
        self.dirty = false;
        Ok(())
    }
}

/// Add the coverage of one location (a file, a directory, or nothing) to
/// the running total.
fn accumulate_at(
    root: &DirNode,
    dirs: &[OsString],
    name: &OsString,
    coverage: &mut BranchCoverage,
) {
    let Some(node) = ScanCache::node_comps_ref(root, dirs) else {
        return;
    };
    if let Some(entry) = node.files.get(name) {
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
        return;
    }
    if let Some(child) = node.children.get(name) {
        child.accumulate_coverage(coverage);
    }
}

/// Migrate an absolute-path-keyed snapshot into the v4 volume-keyed tree.
fn migrate_absolute_tree(old_root: DirNode, stats: CacheStats, path: &Path) -> ScanCache {
    let mounts = mounted_volumes();
    let root = rekey_tree(old_root, &mounts);
    let files = root.file_count();
    tracing::info!(
        "scan cache {} migrated to volume keys: {files} of {} entries re-keyed",
        path.display(),
        stats.files
    );
    ScanCache {
        root,
        stats: CacheStats {
            files,
            ..stats
        },
        ..ScanCache::default()
    }
    .with_dirty()
}

impl ScanCache {
    fn with_dirty(mut self) -> Self {
        self.dirty = true;
        self
    }
}

/// Re-key an absolute-path tree into the volume-keyed tree. Entries whose
/// volume is currently mounted move to `[volume-key]/relative/path`; the rest
/// are preserved under the `legacy` branch and promoted lazily on lookup.
fn rekey_tree(old: DirNode, mounts: &[(PathBuf, OsString)]) -> DirNode {
    // Volume keys are memoized per mount point: one diskutil call per volume,
    // not per file.
    let mut key_cache: HashMap<PathBuf, OsString> = mounts.iter().cloned().collect();

    fn resolve(
        file_path: &Path,
        mounts: &[(PathBuf, OsString)],
        key_cache: &mut HashMap<PathBuf, OsString>,
    ) -> Option<(OsString, Vec<OsString>)> {
        // Existing files resolve exactly like lookups do (stat, then walk up
        // to the real mount boundary — firmlinks on macOS make that deeper
        // than "/").
        let exact = std::fs::metadata(file_path).ok().and_then(|meta| {
            let dev = device_of(&meta);
            let mount = mount_point_of(file_path, dev);
            let key = key_cache
                .entry(mount.clone())
                .or_insert_with(|| volume_key_for(&mount))
                .clone();
            dirs_for(file_path, &mount, &key)
        });
        if exact.is_some() {
            return exact;
        }
        // Missing files (drive not mounted): best-effort prefix match so
        // volumes on the conventional mount points still re-key.
        mounts.iter().find_map(|(mount, key)| dirs_for(file_path, mount, key))
    }

    fn dirs_for(file_path: &Path, mount: &Path, key: &OsString) -> Option<(OsString, Vec<OsString>)> {
        let rel = file_path.strip_prefix(mount).ok()?;
        let name = rel.file_name()?.to_os_string();
        let mut dirs = vec![key.clone()];
        let mut stack = Vec::new();
        let mut parent = rel.parent();
        while let Some(p) = parent {
            if let Some(component) = p.file_name() {
                stack.push(component.to_os_string());
            }
            parent = p.parent();
        }
        stack.reverse();
        dirs.extend(stack);
        Some((name, dirs))
    }

    fn walk(
        node: DirNode,
        abs: &mut PathBuf,
        new_root: &mut DirNode,
        mounts: &[(PathBuf, OsString)],
        key_cache: &mut HashMap<PathBuf, OsString>,
    ) {
        for (name, entry) in node.files {
            let file_path = abs.join(&name);
            let target = resolve(&file_path, mounts, key_cache)
                .unwrap_or_else(|| ScanCache::legacy_components(&file_path));
            let mut target_node = &mut *new_root;
            for component in target.1 {
                target_node = target_node.children.entry(component).or_default();
            }
            target_node.files.insert(target.0, entry);
        }
        for (name, child) in node.children {
            abs.push(&name);
            walk(child, abs, new_root, mounts, key_cache);
            abs.pop();
        }
    }

    let mut new_root = DirNode::default();
    let mut abs = PathBuf::new();
    walk(old, &mut abs, &mut new_root, mounts, &mut key_cache);
    new_root
}

/// Lock helper that survives a poisoned mutex (a panic while hashing one
/// file must not disable the cache for the rest of the scan).
pub fn lock(cache: &Mutex<ScanCache>) -> std::sync::MutexGuard<'_, ScanCache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

fn pixel_key_of(path: &Path) -> Option<PixelKey> {
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        image_reader::read_image(path)
    }));
    let image: ImageData = match decoded {
        Ok(Ok(image)) => image,
        _ => return None,
    };
    let hash = match &image.data {
        PixelData::U8(bytes) => seahash::hash(bytes),
        PixelData::U16(words) => {
            let mut hasher = seahash::SeaHasher::new();
            use std::hash::Hasher;
            for chunk in words.chunks(4096) {
                for &word in chunk {
                    hasher.write_u16(word);
                }
            }
            hasher.finish()
        }
    };
    Some(PixelKey {
        width: image.width,
        height: image.height,
        cpp: image.cpp,
        hash,
    })
}

/// Shallow-hash many files through one shared cache. The lock is only held
/// for the lookup and the store — the actual file hashing runs outside it, so
/// the pass stays parallel instead of serializing behind the cache.
pub fn shallow_hashes_cached(
    cache: &Mutex<ScanCache>,
    paths: &[PathBuf],
    n: usize,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<(u64, bool)> {
    let total = paths.len();
    let done = AtomicUsize::new(0);
    paths
        .par_iter()
        .map(|path| {
            // NOTE: the lookup guard must be dropped before the store lock —
            // a `match lock(...)` scrutinee guard would live through the
            // whole match and deadlock the second lock.
            let cached = { lock(cache).shallow_hash_lookup(path) };
            let result = match cached {
                Some(value) => value,
                None => {
                    let value = image_reader::hash_image_data_status(path, n);
                    lock(cache).shallow_hash_store(path, value);
                    value
                }
            };
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            result
        })
        .collect()
}

/// Whole-file byte hash through the cache, computed outside the lock.
pub fn full_bytes_cached(cache: &Mutex<ScanCache>, path: &Path) -> Result<u64, ImageReaderError> {
    if let Some(value) = lock(cache).full_bytes_lookup(path) {
        return Ok(value);
    }
    let value = image_reader::hash_all_bytes(path)?;
    lock(cache).full_bytes_store(path, value);
    Ok(value)
}

/// Full encoded-image-data hash through the cache, computed outside the lock.
pub fn full_content_cached(cache: &Mutex<ScanCache>, path: &Path) -> Result<u64, ImageReaderError> {
    if let Some(value) = lock(cache).full_content_lookup(path) {
        return Ok(value);
    }
    let value = image_reader::hash_image_data_all(path)?;
    lock(cache).full_content_store(path, value);
    Ok(value)
}

/// Perceptual hash + chroma through the cache. Phash and chroma both need a
/// decode, so missing pieces are filled with a single decode pass (the old
/// phash value survives if only the chroma is new).
pub fn perceptual_cached(cache: &Mutex<ScanCache>, path: &Path) -> Option<(Phash, Option<u8>)> {
    let (phash, chroma) = {
        let mut cache = lock(cache);
        (cache.phash_lookup(path), cache.chroma_lookup(path))
    };
    if let (Some(phash), Some(chroma)) = (phash, chroma) {
        return Some((phash, Some(chroma)));
    }

    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        image_reader::read_image(path)
    }));
    let image = match decoded {
        Ok(Ok(image)) => image,
        _ => return phash.map(|phash| (phash, None)),
    };
    let phash = match phash {
        Some(phash) => phash,
        None => {
            let phash = image_reader::phash_of(&image)?;
            lock(cache).phash_store(path, phash);
            phash
        }
    };
    let chroma = match chroma {
        Some(chroma) => Some(chroma),
        None => match image_reader::mean_chroma(&image) {
            Some(chroma) => {
                lock(cache).chroma_store(path, chroma);
                Some(chroma)
            }
            None => None,
        },
    };
    Some((phash, chroma))
}

/// Perceptual hash through the cache, computed outside the lock.
pub fn cached_phash(cache: &Mutex<ScanCache>, path: &Path) -> Option<Phash> {
    if let Some(value) = lock(cache).phash_lookup(path) {
        return Some(value);
    }
    let value = image_reader::phash(path)?;
    lock(cache).phash_store(path, value);
    Some(value)
}

/// Decoded pixel key through the cache, computed outside the lock.
pub fn pixel_cached(cache: &Mutex<ScanCache>, path: &Path) -> Option<PixelKey> {
    if let Some(value) = lock(cache).pixel_lookup(path) {
        return Some(value);
    }
    let value = pixel_key_of(path)?;
    lock(cache).pixel_store(path, value);
    Some(value)
}

/// Date resolution through the cache: cached dates are served directly; the
/// misses go through the normal batched funnel (in-process readers → one
/// exiftool run → folder proxy) and are stored as they come back.
pub fn creation_dates_cached(
    cache: &Mutex<ScanCache>,
    paths: &[PathBuf],
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<CreationDate> {
    let total = paths.len();
    let mut out: Vec<Option<CreationDate>> = Vec::with_capacity(total);
    let mut misses: Vec<(usize, PathBuf)> = Vec::new();
    {
        let mut cache = lock(cache);
        for (i, path) in paths.iter().enumerate() {
            match cache.creation_date(path) {
                Some(date) => {
                    out.push(Some(date));
                    progress(i + 1, total);
                }
                None => {
                    out.push(None);
                    misses.push((i, path.clone()));
                }
            }
        }
    }

    if !misses.is_empty() {
        let miss_paths: Vec<PathBuf> = misses.iter().map(|(_, p)| p.clone()).collect();
        let hits = total - misses.len();
        let computed = crate::exif::creation_dates_batch(&miss_paths, &|done, _total| {
            progress(hits + done, total);
        });
        let mut cache = lock(cache);
        for ((i, path), date) in misses.iter().zip(computed) {
            out[*i] = Some(date.clone());
            cache.store_creation_date(path, date);
        }
    }

    out.into_iter()
        .map(|date| date.unwrap_or(CreationDate::Unknown))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_v1(root: &mut v1::DirNode, path: &Path, entry: v1::FileEntry) {
        let mut node = root;
        for comp in path.parent().unwrap().components() {
            node = node
                .children
                .entry(comp.as_os_str().to_os_string())
                .or_insert_with(|| v1::DirNode {
                    children: HashMap::new(),
                    files: HashMap::new(),
                });
        }
        node.files
            .insert(path.file_name().unwrap().to_os_string(), entry);
    }

    /// A real version-1 snapshot (with the sharpness field) must migrate:
    /// loaded entries still validate against the file and serve every layer
    /// from the cache, with no recomputation.
    #[test]
    fn version_1_snapshots_migrate_without_losing_knowledge() {
        let dir = std::env::temp_dir().join(format!("dedupe2-v1-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("cache.bin");
        let file = dir.join("photo.jpg");
        std::fs::write(&file, vec![7u8; 2048]).unwrap();

        let meta = std::fs::metadata(&file).unwrap();
        let entry = v1::FileEntry {
            size: meta.len(),
            mtime: meta.modified().ok(),
            dev: 7,
            shallow: Some((0xABCD, true)),
            full_bytes: Some(11),
            full_content: Some(22),
            pixel: Some(PixelKey {
                width: 10,
                height: 20,
                cpp: 3,
                hash: 33,
            }),
            phash: Some(v1::PhashV1 {
                hash: 44,
                width: 10,
                height: 20,
                sharpness: 0.75,
            }),
            date: Some(CreationDate::DateCreated("2020-01-02".into())),
        };
        let mut root = v1::DirNode {
            children: HashMap::new(),
            files: HashMap::new(),
        };
        insert_v1(&mut root, &file, entry);
        let snapshot = v1::PersistedCache {
            version: 1,
            root,
            stats: CacheStats {
                files: 1,
                reused: 5,
                computed: 6,
            },
        };
        std::fs::write(&store, bincode::serialize(&snapshot).unwrap()).unwrap();

        let mut cache = ScanCache::load(&store);
        assert_eq!(cache.stats().files, 1, "entry survived the migration");
        assert!(cache.dirty(), "migration schedules a rewrite in v2");


        let before = cache.stats();
        assert_eq!(cache.shallow_hash(&file, 64 * 1024), (0xABCD, true));
        assert_eq!(cache.full_content_hash(&file).unwrap(), 22);
        assert_eq!(
            cache.phash(&file).unwrap(),
            Phash {
                hash: 44,
                width: 10,
                height: 20
            },
            "sharpness dropped, everything else intact"
        );
        assert_eq!(
            cache.creation_date(&file),
            Some(CreationDate::DateCreated("2020-01-02".into()))
        );
        assert_eq!(
            cache.stats().computed,
            before.computed,
            "nothing recomputed after migration"
        );

        let _ = std::fs::remove_file(&store);
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn volume_keys_separate_drives_structurally() {
        let mut cache = ScanCache::default();
        let mount = VolumeMount {
            mount_point: PathBuf::from("/Volumes/Disk"),
            key: OsString::from("vol:UUID-A"),
        };
        // Same relative layout under two different volume keys never shares
        // entries: the tree branches by volume.
        let (name_a, dirs_a) = ScanCache::key_components_for_mount(
            &mount,
            Path::new("/Volumes/Disk/photos/2013/a.jpg"),
        )
        .unwrap();
        assert_eq!(name_a, OsString::from("a.jpg"));
        assert_eq!(
            dirs_a,
            vec![
                OsString::from("vol:UUID-A"),
                OsString::from("photos"),
                OsString::from("2013")
            ]
        );

        let mount_b = VolumeMount {
            mount_point: PathBuf::from("/Volumes/Other"),
            key: OsString::from("vol:UUID-B"),
        };
        let (_, dirs_b) = ScanCache::key_components_for_mount(
            &mount_b,
            Path::new("/Volumes/Other/photos/2013/a.jpg"),
        )
        .unwrap();
        assert_ne!(dirs_a[0], dirs_b[0], "different drives, different branches");

        // Legacy components keep the absolute path under the reserved root.
        let (legacy_name, legacy_dirs) =
            ScanCache::legacy_components(Path::new("/Volumes/Gone/photos/a.jpg"));
        assert_eq!(legacy_name, OsString::from("a.jpg"));
        assert_eq!(legacy_dirs.last().unwrap(), &OsString::from("photos"));
        assert_eq!(legacy_dirs[0], OsString::from(LEGACY_ROOT));

        let _ = &mut cache;
    }

    #[test]
    fn version_2_snapshots_migrate_to_volume_ids() {
        let dir = std::env::temp_dir().join(format!("dedupe2-v2-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("cache.bin");
        let file = dir.join("photo.jpg");
        std::fs::write(&file, vec![9u8; 1024]).unwrap();
        let meta = std::fs::metadata(&file).unwrap();

        // v2 shares DirNode/FileEntry with v3 — only the snapshot wrapper
        // differed (no volume_ids map), so the fixture is built with the
        // real types.
        let mut root = DirNode {
            children: HashMap::new(),
            files: HashMap::new(),
        };
        let mut node = &mut root;
        for comp in file.parent().unwrap().components() {
            node = node
                .children
                .entry(comp.as_os_str().to_os_string())
                .or_default();
        }
        node.files.insert(
            file.file_name().unwrap().to_os_string(),
            FileEntry {
                size: meta.len(),
                mtime: meta.modified().ok(),
                dev: 7,
                shallow: Some((0xFEED, true)),
                ..Default::default()
            },
        );
        // DirNode/FileEntry derive Default + serde for exactly this.
        let snapshot = v2::PersistedCache {
            version: 2,
            root,
            stats: CacheStats {
                files: 1,
                reused: 0,
                computed: 1,
            },
        };
        std::fs::write(&store, bincode::serialize(&snapshot).unwrap()).unwrap();

        let mut cache = ScanCache::load(&store);
        assert_eq!(cache.stats().files, 1);
        assert!(cache.dirty(), "migration schedules a v3 rewrite");
        let before = cache.stats();
        assert_eq!(
            cache.shallow_hash(&file, 64 * 1024),
            (0xFEED, true),
            "v2 knowledge preserved without recomputation"
        );
        assert_eq!(cache.stats().computed, before.computed);
        // The mount resolution is the only thing that changed.
        assert!(!cache.mounts.is_empty(), "volume mount resolved on touch");

        let _ = std::fs::remove_file(&store);
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn rekey_puts_identical_layouts_under_each_volume() {
        // Two drives with the same relative layout become two branches.
        let mut old = DirNode::default();
        for (volume, entry_marker) in [("A", 1u64), ("B", 2u64)] {
            let mut node = &mut old;
            for comp in ["/", "Volumes", volume, "photos", "2013"].iter() {
                node = node
                    .children
                    .entry(OsString::from(*comp))
                    .or_default();
            }
            node.files.insert(
                OsString::from("a.jpg"),
                FileEntry {
                    size: entry_marker,
                    ..Default::default()
                },
            );
        }

        let mounts = vec![
            (PathBuf::from("/Volumes/A"), OsString::from("vol:A")),
            (PathBuf::from("/Volumes/B"), OsString::from("vol:B")),
        ];
        let root = rekey_tree(old, &mounts);

        let a = root
            .children
            .get(&OsString::from("vol:A"))
            .and_then(|n| n.children.get(&OsString::from("photos")))
            .and_then(|n| n.children.get(&OsString::from("2013")))
            .and_then(|n| n.files.get(&OsString::from("a.jpg")))
            .expect("A branch keyed by volume");
        let b = root
            .children
            .get(&OsString::from("vol:B"))
            .and_then(|n| n.children.get(&OsString::from("photos")))
            .and_then(|n| n.children.get(&OsString::from("2013")))
            .and_then(|n| n.files.get(&OsString::from("a.jpg")))
            .expect("B branch keyed by volume");
        assert_eq!(a.size, 1);
        assert_eq!(b.size, 2);
    }

    #[test]
    fn legacy_entries_are_promoted_on_lookup_without_recompute() {
        let dir = std::env::temp_dir().join(format!("dedupe2-legacy-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("photo.jpg");
        std::fs::write(&file, vec![5u8; 512]).unwrap();
        let meta = std::fs::metadata(&file).unwrap();

        let mut cache = ScanCache::default();
        // Simulate an entry that migration parked in the legacy branch.
        let (legacy_name, legacy_dirs) = ScanCache::legacy_components(&file);
        {
            let node = ScanCache::node_comps_mut(&mut cache.root, &legacy_dirs).unwrap();
            node.files.insert(
                legacy_name,
                FileEntry {
                    size: meta.len(),
                    mtime: meta.modified().ok(),
                    shallow: Some((0xBEEF, true)),
                    ..Default::default()
                },
            );
            cache.stats.files += 1;
        }

        let before = cache.stats();
        assert_eq!(
            cache.shallow_hash(&file, 64 * 1024),
            (0xBEEF, true),
            "legacy entry served without recomputation"
        );
        let after = cache.stats();
        assert_eq!(after.computed, before.computed);
        assert_eq!(after.reused, before.reused + 1);
        assert!(cache.dirty(), "promotion rewrites the cache");

        // The legacy branch is empty now; the entry lives under the volume key.
        assert_eq!(cache.branch_coverage(&file).files, 1);
        let legacy_files = ScanCache::node_comps_existing_mut(&mut cache.root, &legacy_dirs)
            .map(|node| node.file_count())
            .unwrap_or(0);
        assert_eq!(legacy_files, 0, "moved out of the legacy branch");

        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
    }
}

#[cfg(test)]
mod chroma_tests {
    use super::*;

    #[test]
    fn perceptual_cache_stores_hash_and_chroma_together() {
        let dir = std::env::temp_dir().join(format!("dedupe2-chroma-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("photo.jpg");
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages/jpg-exif-mod/image1.JPG");
        std::fs::copy(&src, &file).unwrap();

        let cache = Mutex::new(ScanCache::new());
        let (phash, chroma) = perceptual_cached(&cache, &file).expect("perceptual");
        assert!(chroma.is_some(), "color jpeg has a chroma value");
        assert!(lock(&cache).stats().computed >= 2, "hash and chroma stored");

        // Second pass: everything served from the cache.
        let before = lock(&cache).stats();
        let (phash2, chroma2) = perceptual_cached(&cache, &file).expect("perceptual");
        let after = lock(&cache).stats();
        assert_eq!(phash, phash2);
        assert_eq!(chroma, chroma2);
        assert_eq!(after.computed, before.computed, "no recompute");

        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn version_4_snapshots_migrate_dropping_only_chroma() {
        let dir = std::env::temp_dir().join(format!("dedupe2-v4-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("cache.bin");
        let file = dir.join("photo.jpg");
        std::fs::write(&file, vec![3u8; 1024]).unwrap();
        let meta = std::fs::metadata(&file).unwrap();

        // Volume-keyed tree (as v4 wrote it) with the cache's own component
        // layout, so lookups resolve to the exact same branch.
        let (name, dirs) = ScanCache::new()
            .key_components(&file, device_of(&meta))
            .expect("key components");
        let empty = || v4::DirNode {
            children: HashMap::new(),
            files: HashMap::new(),
        };
        let mut root = empty();
        let mut node = &mut root;
        for component in &dirs {
            node = node.children.entry(component.clone()).or_insert_with(empty);
        }
        node.files.insert(
            name,
            v4::FileEntry {
                size: meta.len(),
                mtime: meta.modified().ok(),
                dev: 7,
                shallow: Some((0xCAFE, true)),
                full_bytes: None,
                full_content: None,
                pixel: None,
                phash: Some(Phash {
                    hash: 99,
                    width: 10,
                    height: 20,
                }),
                date: None,
            },
        );

        let snapshot = v4::PersistedCache {
            version: 4,
            root,
            stats: CacheStats {
                files: 1,
                reused: 0,
                computed: 1,
            },
        };
        std::fs::write(&store, bincode::serialize(&snapshot).unwrap()).unwrap();

        let mut cache = ScanCache::load(&store);
        assert_eq!(cache.stats().files, 1);
        assert!(cache.dirty(), "migration schedules a v5 rewrite");

        let before = cache.stats();
        assert_eq!(cache.shallow_hash(&file, 64 * 1024), (0xCAFE, true));
        assert_eq!(cache.phash(&file).unwrap().hash, 99, "phash preserved");
        assert_eq!(
            cache.stats().computed,
            before.computed,
            "v4 knowledge served without recomputation"
        );

        let _ = std::fs::remove_file(&store);
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
    }
}
