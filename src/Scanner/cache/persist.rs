//! Loading, saving and migrating snapshots, and merging branches whose
//! volume key changed.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;

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
        let Ok(data) = std::fs::read(path) else {
            return ScanCache::new();
        };
        let Some(mut cache) = Self::parse_snapshot(&data, path) else {
            return ScanCache::new();
        };
        cache.refresh_mounts();
        if cache.adopt_boot_orphans() > 0 {
            cache = cache.with_dirty();
        }
        tracing::info!("scan cache loaded: {} files", cache.stats.files);
        cache
    }

    /// Decode (and, for older versions, migrate) a snapshot. `None` for
    /// anything unreadable, corrupt or of an unknown version. `path` is only
    /// used in log messages.
    pub(super) fn parse_snapshot(data: &[u8], path: &Path) -> Option<ScanCache> {
        // Peek at the version first: bincode isn't self-describing, so each
        // version needs its own parse.
        let version: u32 = match bincode::deserialize::<(u32,)>(data) {
            Ok((version,)) => version,
            Err(e) => {
                tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                return None;
            }
        };

        // After a successful load, pre-resolve currently mounted volumes so
        // coverage queries and validations take the fast path.
        let cache = match version {
            CACHE_FORMAT_VERSION => {
                let snapshot: PersistedCache = match bincode::deserialize(data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return None;
                    }
                };
                ScanCache {
                    root: snapshot.root,
                    stats: snapshot.stats,
                    ..ScanCache::default()
                }
            }
            5 => {
                let snapshot: v5::PersistedCache = match bincode::deserialize(data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return None;
                    }
                };
                let root = v5::convert_node(snapshot.root);
                let files = root.file_count();
                tracing::info!(
                    "scan cache {} migrated from version 5: {files} files",
                    path.display()
                );
                ScanCache {
                    root,
                    stats: CacheStats {
                        files,
                        ..snapshot.stats
                    },
                    ..ScanCache::default()
                }
                .with_dirty()
            }
            4 => {
                let snapshot: v4::PersistedCache = match bincode::deserialize(data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return None;
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
                    stats: CacheStats {
                        files,
                        ..snapshot.stats
                    },
                    ..ScanCache::default()
                }
                .with_dirty()
            }
            3 => {
                let snapshot: v3::PersistedCache = match bincode::deserialize(data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return None;
                    }
                };
                migrate_absolute_tree(snapshot.root, snapshot.stats, path)
            }
            2 => {
                let snapshot: v2::PersistedCache = match bincode::deserialize(data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return None;
                    }
                };
                migrate_absolute_tree(snapshot.root, snapshot.stats, path)
            }
            1 => {
                let snapshot: v1::PersistedCache = match bincode::deserialize(data) {
                    Ok(snapshot) => snapshot,
                    Err(e) => {
                        tracing::warn!("ignoring unreadable scan cache {}: {e}", path.display());
                        return None;
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
                return None;
            }
        };
        Some(cache)
    }

    /// Re-key an old absolute-path tree against the currently mounted volumes
    /// and rewrite the counters. Nothing is discarded: unmounted volumes'
    /// entries land under `legacy` for lazy promotion.
    pub(super) fn refresh_mounts(&mut self) {
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

/// Merge `source` into `target`, keeping existing entries and filling in
/// missing layers from the source (the source is typically an old-key branch
/// whose layers are as good as, or older than, the target's).
pub(super) fn merge_node(target: &mut DirNode, source: DirNode) {
    for (name, child) in source.children {
        match target.children.entry(name) {
            std::collections::hash_map::Entry::Occupied(mut occupied) => {
                merge_node(occupied.get_mut(), child);
            }
            std::collections::hash_map::Entry::Vacant(vacant) => {
                vacant.insert(child);
            }
        }
    }
    for (name, entry) in source.files {
        match target.files.entry(name) {
            std::collections::hash_map::Entry::Occupied(mut occupied) => {
                merge_entry_layers(occupied.get_mut(), entry);
            }
            std::collections::hash_map::Entry::Vacant(vacant) => {
                vacant.insert(entry);
            }
        }
    }
}

pub(super) fn merge_entry_layers(target: &mut FileEntry, source: FileEntry) {
    target.shallow = target.shallow.or(source.shallow);
    target.full_bytes = target.full_bytes.or(source.full_bytes);
    target.full_content = target.full_content.or(source.full_content);
    target.pixel = target.pixel.or(source.pixel);
    target.phash = target.phash.or(source.phash);
    target.chroma = target.chroma.or(source.chroma);
    target.dhash = target.dhash.or(source.dhash);
    if target.date.is_none() {
        target.date = source.date;
    }
}
/// Add the coverage of one location (a file, a directory, or nothing) to
/// the running total.
pub(super) fn accumulate_at(
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
pub(super) fn migrate_absolute_tree(
    old_root: DirNode,
    stats: CacheStats,
    path: &Path,
) -> ScanCache {
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
        stats: CacheStats { files, ..stats },
        ..ScanCache::default()
    }
    .with_dirty()
}

impl ScanCache {
    pub(super) fn with_dirty(mut self) -> Self {
        self.dirty = true;
        self
    }
}

/// Re-key an absolute-path tree into the volume-keyed tree. Entries whose
/// volume is currently mounted move to `[volume-key]/relative/path`; the rest
/// are preserved under the `legacy` branch and promoted lazily on lookup.
pub(super) fn rekey_tree(old: DirNode, mounts: &[(PathBuf, OsString)]) -> DirNode {
    // Volume keys are memoized per mount point: one diskutil call per volume,
    // not per file.
    let mut key_cache: HashMap<PathBuf, OsString> = mounts.iter().cloned().collect();

    pub(super) fn resolve(
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
        mounts
            .iter()
            .find_map(|(mount, key)| dirs_for(file_path, mount, key))
    }

    pub(super) fn dirs_for(
        file_path: &Path,
        mount: &Path,
        key: &OsString,
    ) -> Option<(OsString, Vec<OsString>)> {
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

    pub(super) fn walk(
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

#[cfg(test)]
mod corruption_tests {
    use super::*;

    /// A saved cache with real entries, and its bytes.
    fn saved_cache(dir: &Path) -> (PathBuf, Vec<u8>) {
        let mut cache = ScanCache::new();
        for i in 0..20 {
            let file = dir.join(format!("photo{i}.jpg"));
            std::fs::write(&file, format!("bytes {i}")).unwrap();
            cache.shallow_hash_store(&file, (i as u64, true));
            cache.full_bytes_store(&file, i as u64 * 7);
            cache.store_creation_date(&file, CreationDate::DateCreated("2020:01:02".into()));
        }
        let path = dir.join("cache.bin");
        cache.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    }

    /// The decode step of `load`, without the mount refresh (a diskutil
    /// call) that follows it: corrupt input is handled here or not at all.
    fn load_bytes(dir: &Path, bytes: &[u8]) -> ScanCache {
        ScanCache::parse_snapshot(bytes, &dir.join("corrupt.bin")).unwrap_or_default()
    }

    #[test]
    fn a_valid_snapshot_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _) = saved_cache(dir.path());
        let mut cache = ScanCache::load(&path);
        assert_eq!(cache.root.file_count(), 20);
        assert_eq!(
            cache.full_bytes_lookup(&dir.path().join("photo3.jpg")),
            Some(21)
        );
    }

    #[test]
    fn every_truncation_loads_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let (_, bytes) = saved_cache(dir.path());
        for len in 0..bytes.len() {
            let cache = load_bytes(dir.path(), &bytes[..len]);
            assert_eq!(
                cache.root.file_count(),
                0,
                "truncated at {len}: starts fresh"
            );
        }
    }

    #[test]
    fn flipped_bytes_never_panic() {
        let dir = tempfile::tempdir().unwrap();
        let (_, bytes) = saved_cache(dir.path());
        for at in 0..bytes.len() {
            for mask in [0x01u8, 0x80, 0xFF] {
                let mut corrupt = bytes.clone();
                corrupt[at] ^= mask;
                // Either a fresh cache or a (differently) valid one; never a
                // crash or a runaway allocation.
                let _ = load_bytes(dir.path(), &corrupt);
            }
        }
    }

    #[test]
    fn absurd_length_prefixes_are_rejected_not_allocated() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = CACHE_FORMAT_VERSION.to_le_bytes().to_vec();
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 32]);
        assert_eq!(load_bytes(dir.path(), &bytes).root.file_count(), 0);

        let garbage: Vec<u8> = (0..4096u32)
            .map(|i| i.wrapping_mul(2654435761) as u8)
            .collect();
        assert_eq!(load_bytes(dir.path(), &garbage).root.file_count(), 0);
        assert_eq!(load_bytes(dir.path(), &[]).root.file_count(), 0);
    }

    #[test]
    fn unknown_versions_start_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut bytes) = saved_cache(dir.path());
        bytes[..4].copy_from_slice(&999u32.to_le_bytes());
        assert_eq!(load_bytes(dir.path(), &bytes).root.file_count(), 0);
    }
}
