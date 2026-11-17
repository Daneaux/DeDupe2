use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use super::*;
use crate::image_reader::Phash;

#[test]
fn perceptual_cache_stores_hash_and_chroma_together() {
    let dir = std::env::temp_dir().join(format!("dedupe2-chroma-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("photo.jpg");
    let src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages/jpg-exif-mod/image1.JPG");
    std::fs::copy(&src, &file).unwrap();

    let cache = Mutex::new(ScanCache::new());
    let (phash, chroma, dhash) = perceptual_cached(&cache, &file).expect("perceptual");
    assert!(chroma.is_some(), "color jpeg has a chroma value");
    assert!(dhash.is_some(), "gradient hash stored too");
    assert!(
        lock(&cache).stats().computed >= 3,
        "hash, chroma and dhash stored"
    );

    // Second pass: everything served from the cache.
    let before = lock(&cache).stats();
    let (phash2, chroma2, dhash2) = perceptual_cached(&cache, &file).expect("perceptual");
    let after = lock(&cache).stats();
    assert_eq!(phash, phash2);
    assert_eq!(chroma, chroma2);
    assert_eq!(dhash, dhash2);
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
