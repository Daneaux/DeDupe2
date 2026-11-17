//! On-disk snapshot formats: the current one and the older versions that
//! still migrate losslessly.

use std::collections::HashMap;

use crate::exif::CreationDate;
use crate::image_reader::Phash;

use super::*;

/// On-disk snapshot version. Bump when the entry shape changes; older files
/// are migrated when the difference is lossless, ignored otherwise.
pub(super) const CACHE_FORMAT_VERSION: u32 = 6;

// ---- version 1 compatibility -------------------------------------------
// v1 stored a `sharpness` estimate inside Phash (a heuristic that was
// removed). Everything else is identical, so a v1 snapshot converts by
// dropping that one field — no cache knowledge is lost.
// Remove after the next format bump.
pub(super) mod v1 {
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

    pub(super) fn convert_entry(entry: FileEntry) -> FileEntrySuper {
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
            dhash: None,
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
pub(super) struct PersistedCache {
    pub(super) version: u32,
    pub(super) root: DirNode,
    pub(super) stats: CacheStats,
}

/// v2/v3 snapshots: same tree, absolute-path keyed; v3 additionally carried a
/// dev -> UUID map that v4 does not need (the volume key is in the tree).
pub(super) mod v2 {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct PersistedCache {
        pub version: u32,
        pub root: DirNode,
        pub stats: CacheStats,
    }
}

pub(super) mod v3 {
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

/// v5: entries with chroma but without the gradient hash.
pub(super) mod v5 {
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
        pub chroma: Option<u8>,
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

    pub(super) fn convert_entry(entry: FileEntry) -> FileEntrySuper {
        FileEntrySuper {
            size: entry.size,
            mtime: entry.mtime,
            dev: entry.dev,
            shallow: entry.shallow,
            full_bytes: entry.full_bytes,
            full_content: entry.full_content,
            pixel: entry.pixel,
            phash: entry.phash,
            chroma: entry.chroma,
            dhash: None,
            date: entry.date,
        }
    }

    type DirNodeSuper = super::DirNode;
    type FileEntrySuper = super::FileEntry;
}

/// v4: volume-keyed tree, entries without the chroma layer.
pub(super) mod v4 {
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

    pub(super) fn convert_entry(entry: FileEntry) -> FileEntrySuper {
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
            dhash: None,
            date: entry.date,
        }
    }

    type DirNodeSuper = super::DirNode;
    type FileEntrySuper = super::FileEntry;
}
