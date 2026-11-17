//! Addressing entries by path (volume key + components), branch-wide
//! invalidation and coverage, and lazy promotion of legacy entries.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::time::SystemTime;

use super::persist::{accumulate_at, merge_node};
use super::*;

impl ScanCache {
    /// Volume key + mount point for a device id, resolved once per volume
    /// per process (diskutil on first sight).
    pub(super) fn volume_mount(&mut self, path: &Path, dev: u64) -> Option<VolumeMount> {
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
    pub(super) fn key_components(
        &mut self,
        path: &Path,
        dev: u64,
    ) -> Option<(OsString, Vec<OsString>)> {
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
    pub(super) fn legacy_components(path: &Path) -> (OsString, Vec<OsString>) {
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
        let mount = std::fs::metadata(dir)
            .ok()
            .map(|meta| device_of(&meta))
            .and_then(|dev| self.volume_mount(dir, dev))
            .or_else(|| self.mount_for_path(dir));
        if let Some(mount) = mount {
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
    /// Merge root branches left behind by earlier boot-volume UUIDs (macOS
    /// re-creates the sealed system volume on updates) into the current boot
    /// branch. Only branches that mirror the boot filesystem's top level are
    /// adopted; branches of other mounted volumes and of unmounted drives
    /// (whose top-level names don't exist under "/") are left alone.
    pub(super) fn adopt_boot_orphans(&mut self) -> usize {
        let Ok(boot_meta) = std::fs::metadata("/") else {
            return 0;
        };
        let boot_dev = device_of(&boot_meta);
        let boot_key = self
            .volume_mount(Path::new("/"), boot_dev)
            .map(|mount| mount.key)
            .unwrap_or_else(|| volume_key_for(Path::new("/")));
        let mounted_keys: HashSet<OsString> = self
            .mounts
            .values()
            .map(|mount| mount.key.clone())
            .collect();
        let Ok(entries) = std::fs::read_dir("/") else {
            return 0;
        };
        let root_names: HashSet<OsString> = entries.flatten().map(|e| e.file_name()).collect();
        self.adopt_orphans_with(&boot_key, &mounted_keys, &root_names)
    }

    pub(super) fn adopt_orphans_with(
        &mut self,
        boot_key: &OsString,
        mounted_keys: &HashSet<OsString>,
        root_names: &HashSet<OsString>,
    ) -> usize {
        let legacy = OsString::from(LEGACY_ROOT);
        let candidates: Vec<OsString> = self
            .root
            .children
            .iter()
            .filter(|(key, branch)| {
                *key != boot_key
                    && **key != legacy
                    && !mounted_keys.contains(*key)
                    && branch.file_count() > 0
                    && !branch.children.is_empty()
                    && branch.children.keys().all(|name| root_names.contains(name))
            })
            .map(|(key, _)| key.clone())
            .collect();
        if candidates.is_empty() {
            return 0;
        }
        let sources: Vec<DirNode> = candidates
            .iter()
            .filter_map(|key| self.root.children.remove(key))
            .collect();
        let target = self.root.children.entry(boot_key.clone()).or_default();
        for node in sources {
            merge_node(target, node);
        }
        self.stats.files = self.root.file_count();
        tracing::info!(
            "scan cache: merged {} stale boot-volume branch(es) into {boot_key:?}",
            candidates.len()
        );
        candidates.len()
    }

    pub fn branch_coverage(&mut self, path: &Path) -> BranchCoverage {
        let mut coverage = BranchCoverage::default();

        // Resolve the volume exactly like lookups do (stat -> device ->
        // mount walk); mount-point prefix matching alone misses paths that
        // reach a volume through symlinks or the "/Users" side of the boot
        // disk, which made cached trees look empty in the UI.
        let mount = std::fs::metadata(path)
            .ok()
            .map(|meta| device_of(&meta))
            .and_then(|dev| self.volume_mount(path, dev))
            .or_else(|| self.mount_for_path(path));
        if let Some(mount) = mount {
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
    pub(super) fn remove_entry(&mut self, path: &Path) {
        let mut removed = false;
        // The file is usually gone here; its parent still gives us the
        // volume (stat the parent, mirroring lookup resolution).
        let mount = std::fs::metadata(path)
            .ok()
            .map(|meta| device_of(&meta))
            .and_then(|dev| self.volume_mount(path, dev))
            .or_else(|| {
                path.parent()
                    .and_then(|parent| std::fs::metadata(parent).ok())
                    .map(|meta| device_of(&meta))
                    .and_then(|dev| self.volume_mount(path, dev))
            })
            .or_else(|| self.mount_for_path(path));
        if let Some(mount) = mount {
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
    pub(super) fn node_comps_existing_mut<'t>(
        node: &'t mut DirNode,
        comps: &[OsString],
    ) -> Option<&'t mut DirNode> {
        let mut node = node;
        for name in comps {
            node = node.children.get_mut(name)?;
        }
        Some(node)
    }

    pub(super) fn node_comps_mut<'t>(
        node: &'t mut DirNode,
        comps: &[OsString],
    ) -> Option<&'t mut DirNode> {
        let mut node = node;
        for name in comps {
            node = node.children.entry(name.clone()).or_default();
        }
        Some(node)
    }

    pub(super) fn node_comps_ref<'t>(node: &'t DirNode, comps: &[OsString]) -> Option<&'t DirNode> {
        let mut node = node;
        for name in comps {
            node = node.children.get(name)?;
        }
        Some(node)
    }

    /// Directory components (no file split) for a path under a known mount.
    pub(super) fn dir_components_for_mount(
        mount: &VolumeMount,
        dir: &Path,
    ) -> Option<Vec<OsString>> {
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
    pub(super) fn key_components_for_mount(
        mount: &VolumeMount,
        path: &Path,
    ) -> Option<(OsString, Vec<OsString>)> {
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
    pub(super) fn mount_for_path(&self, path: &Path) -> Option<VolumeMount> {
        self.mounts
            .values()
            .filter(|mount| path.starts_with(&mount.mount_point))
            .max_by_key(|mount| mount.mount_point.as_os_str().len())
            .cloned()
    }

    pub(super) fn with_entry<T: Clone>(
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
    pub(super) fn promote_legacy(
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
        let Some(node) = Self::node_comps_mut(&mut self.root, dirs) else {
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
}
