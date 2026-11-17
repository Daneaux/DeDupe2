use std::collections::HashMap;

use super::*;
use crate::image_reader::Phash;

#[test]
fn version_5_snapshots_migrate_keeping_chroma() {
    let dir = std::env::temp_dir().join(format!("dedupe2-v5-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("cache.bin");
    let file = dir.join("photo.jpg");
    std::fs::write(&file, vec![4u8; 1024]).unwrap();
    let meta = std::fs::metadata(&file).unwrap();

    let (name, dirs) = ScanCache::new()
        .key_components(&file, device_of(&meta))
        .expect("key components");
    let empty = || v5::DirNode {
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
        v5::FileEntry {
            size: meta.len(),
            mtime: meta.modified().ok(),
            dev: 7,
            shallow: Some((0xB0B, true)),
            full_bytes: None,
            full_content: None,
            pixel: None,
            phash: Some(Phash {
                hash: 55,
                width: 10,
                height: 20,
            }),
            chroma: Some(42),
            date: None,
        },
    );
    let snapshot = v5::PersistedCache {
        version: 5,
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
    assert!(cache.dirty(), "migration schedules a v6 rewrite");
    let before = cache.stats();
    assert_eq!(cache.shallow_hash(&file, 64 * 1024), (0xB0B, true));
    assert_eq!(cache.phash(&file).unwrap().hash, 55);
    assert_eq!(cache.chroma_lookup(&file), Some(42), "chroma preserved");
    assert_eq!(cache.dhash_lookup(&file), None, "dhash fills in lazily");
    assert_eq!(cache.stats().computed, before.computed, "no recompute");

    let _ = std::fs::remove_file(&store);
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir(&dir);
}
