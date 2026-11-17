use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;
use crate::exif::CreationDate;
use crate::image_reader::Phash;

fn test_entry(size: u64) -> FileEntry {
    FileEntry {
        size,
        mtime: None,
        dev: 1,
        shallow: Some((size, true)),
        full_bytes: None,
        full_content: None,
        pixel: None,
        phash: None,
        chroma: None,
        dhash: None,
        date: None,
    }
}

#[test]
fn merging_branches_keeps_existing_layers_and_adopts_missing_ones() {
    let mut target = DirNode {
        children: HashMap::new(),
        files: HashMap::new(),
    };
    let mut source = DirNode {
        children: HashMap::new(),
        files: HashMap::new(),
    };

    let mut target_entry = test_entry(100);
    target_entry.phash = Some(Phash {
        hash: 0x11,
        width: 8,
        height: 8,
    });
    let mut source_entry = test_entry(100);
    source_entry.phash = Some(Phash {
        hash: 0x22,
        width: 8,
        height: 8,
    });
    source_entry.dhash = Some(0x33);
    source_entry.chroma = Some(200);

    target.files.insert(OsString::from("a.jpg"), target_entry);
    source.files.insert(OsString::from("a.jpg"), source_entry);
    source.files.insert(OsString::from("b.jpg"), test_entry(50));
    source.children.insert(
        OsString::from("sub"),
        DirNode {
            children: HashMap::new(),
            files: HashMap::from([(OsString::from("c.jpg"), test_entry(10))]),
        },
    );

    merge_node(&mut target, source);

    let a = target.files.get(&OsString::from("a.jpg")).unwrap();
    assert_eq!(a.phash.unwrap().hash, 0x11, "existing layer kept");
    assert_eq!(a.dhash, Some(0x33), "missing layer adopted");
    assert_eq!(a.chroma, Some(200));
    assert!(target.files.contains_key(&OsString::from("b.jpg")));
    assert_eq!(
        target
            .children
            .get(&OsString::from("sub"))
            .unwrap()
            .files
            .get(&OsString::from("c.jpg"))
            .unwrap()
            .size,
        10
    );
}

#[test]
fn orphan_branches_mirroring_the_boot_root_are_adopted() {
    let mut cache = ScanCache::default();
    let boot: OsString = OsString::from("vol:NEW");
    let old: OsString = OsString::from("vol:OLD");
    let other: OsString = OsString::from("vol:OTHER");
    let unplugged: OsString = OsString::from("vol:GONE");

    // old boot branch: mirrors "/" (top-level "Users")
    cache.root.children.insert(
        old.clone(),
        DirNode {
            children: HashMap::from([(
                OsString::from("Users"),
                DirNode {
                    children: HashMap::new(),
                    files: HashMap::from([(OsString::from("a.jpg"), test_entry(3))]),
                },
            )]),
            files: HashMap::new(),
        },
    );
    // current boot branch with a newer entry
    cache.root.children.insert(
        boot.clone(),
        DirNode {
            children: HashMap::from([(
                OsString::from("Users"),
                DirNode {
                    children: HashMap::new(),
                    files: HashMap::from([(OsString::from("new.jpg"), test_entry(9))]),
                },
            )]),
            files: HashMap::new(),
        },
    );
    // a mounted other volume: never a candidate
    cache.root.children.insert(
        other.clone(),
        DirNode {
            children: HashMap::from([(
                OsString::from("Users"),
                DirNode {
                    children: HashMap::new(),
                    files: HashMap::from([(OsString::from("x.jpg"), test_entry(1))]),
                },
            )]),
            files: HashMap::new(),
        },
    );
    // an unplugged drive whose root names don't exist under "/"
    cache.root.children.insert(
        unplugged.clone(),
        DirNode {
            children: HashMap::from([(
                OsString::from("SomeExternalFolder"),
                DirNode {
                    children: HashMap::new(),
                    files: HashMap::from([(OsString::from("y.jpg"), test_entry(1))]),
                },
            )]),
            files: HashMap::new(),
        },
    );

    let mounted: HashSet<OsString> = HashSet::from([other.clone()]);
    let root_names: HashSet<OsString> = HashSet::from([OsString::from("Users")]);
    let adopted = cache.adopt_orphans_with(&boot, &mounted, &root_names);

    assert_eq!(adopted, 1);
    assert!(!cache.root.children.contains_key(&old));
    assert!(cache.root.children.contains_key(&other));
    assert!(cache.root.children.contains_key(&unplugged));
    let users = cache
        .root
        .children
        .get(&boot)
        .unwrap()
        .children
        .get(&OsString::from("Users"))
        .unwrap();
    assert!(users.files.contains_key(&OsString::from("a.jpg")));
    assert!(users.files.contains_key(&OsString::from("new.jpg")));
    // both boot entries plus the untouched other/unplugged branches
    assert_eq!(cache.stats.files, 4);
}

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
    let (name_a, dirs_a) =
        ScanCache::key_components_for_mount(&mount, Path::new("/Volumes/Disk/photos/2013/a.jpg"))
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
            node = node.children.entry(OsString::from(*comp)).or_default();
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
