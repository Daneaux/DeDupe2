//! Volume identity: mount points, volume UUIDs and the boot Data volume,
//! so cache keys survive remounts and macOS updates.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The mount point of `path`: walk up while the device stays the same.
pub(super) fn mount_point_of(path: &Path, dev: u64) -> PathBuf {
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
pub(super) fn volume_uuid_for_mount(mount: &Path) -> Option<String> {
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
pub(super) fn volume_uuid_for_mount(_mount: &Path) -> Option<String> {
    None
}

/// All currently mounted volumes (mount point + key), for migration: entries
/// are re-keyed by matching their old absolute paths against these prefixes.
pub(super) fn mounted_volumes() -> Vec<(PathBuf, OsString)> {
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
///
/// Boot-volume paths are keyed by the *data* volume's UUID: macOS re-creates
/// the sealed system volume on OS updates, which changes its UUID and would
/// otherwise strand every cached boot-volume entry under an old key.
pub(super) fn volume_key_for(mount: &Path) -> OsString {
    let data_uuid = if is_boot_mount(mount) {
        boot_data_volume_uuid()
    } else {
        None
    };
    volume_key_from(data_uuid, || volume_uuid_for_mount(mount), mount)
}

/// Decide a volume's persistent key from the (optional) boot Data-volume UUID,
/// lazily falling back to the mount's own UUID and finally to its path. Split
/// out so every branch — including the diskutil-unavailable fallback — is
/// unit-testable without touching the real system.
pub(super) fn volume_key_from(
    data_uuid: Option<String>,
    volume_uuid: impl FnOnce() -> Option<String>,
    mount: &Path,
) -> OsString {
    match data_uuid.or_else(volume_uuid) {
        Some(uuid) => OsString::from(format!("vol:{uuid}")),
        None => OsString::from(format!("mount:{}", mount.display())),
    }
}

/// True when `mount` lives on the boot device (e.g. "/" and, on macOS,
/// "/Volumes/Macintosh HD" both point at the sealed system volume).
#[cfg(target_os = "macos")]
pub(super) fn is_boot_mount(mount: &Path) -> bool {
    match (std::fs::metadata(mount), std::fs::metadata("/")) {
        (Ok(a), Ok(b)) => device_of(&a) == device_of(&b),
        _ => false,
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn is_boot_mount(_mount: &Path) -> bool {
    false
}

/// UUID of the APFS data volume backing the boot disk. Unlike the sealed
/// system volume, it survives OS updates.
#[cfg(target_os = "macos")]
pub(super) fn boot_data_volume_uuid() -> Option<String> {
    volume_uuid_for_mount(Path::new("/System/Volumes/Data"))
}

#[cfg(not(target_os = "macos"))]
pub(super) fn boot_data_volume_uuid() -> Option<String> {
    None
}

/// First component reserved for entries that could not be re-keyed at
/// migration time (their volume was not mounted). Lookups promote them lazily.
pub(super) const LEGACY_ROOT: &str = "legacy";

#[cfg(unix)]
pub(super) fn device_of(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.dev()
}

#[cfg(not(unix))]
pub(super) fn device_of(_meta: &std::fs::Metadata) -> u64 {
    0
}
