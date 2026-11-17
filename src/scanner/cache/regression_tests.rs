use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::Path;

use super::*;
use crate::exif::CreationDate;

fn file_entry(size: u64) -> FileEntry {
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

fn branch_with(paths: &[&str]) -> DirNode {
    let mut node = DirNode {
        children: HashMap::new(),
        files: HashMap::new(),
    };
    for path in paths {
        let mut cursor = &mut node;
        let mut comps: Vec<&str> = path.split('/').collect();
        let name = comps.pop().unwrap();
        for comp in comps {
            cursor = cursor
                .children
                .entry(OsString::from(comp))
                .or_insert_with(|| DirNode {
                    children: HashMap::new(),
                    files: HashMap::new(),
                });
        }
        cursor.files.insert(OsString::from(name), file_entry(1));
    }
    node
}

#[test]
fn adoption_requires_known_top_level_names_and_real_files() {
    let mut cache = ScanCache::default();
    let boot = OsString::from("vol:NEW");
    let mounted_key = OsString::from("vol:OTHER");
    let mounted: HashSet<OsString> = HashSet::from([mounted_key.clone()]);
    let root_names: HashSet<OsString> = HashSet::from([OsString::from("Users")]);

    // Mirrors the boot root but also has a directory that does not exist
    // under "/": must not be adopted.
    cache.root.children.insert(
        OsString::from("vol:UNKNOWN_TOPLEVEL"),
        branch_with(&["Users/a.jpg", "Elsewhere/b.jpg"]),
    );
    // Fully empty directory-only branch: no files to adopt.
    cache.root.children.insert(
        OsString::from("vol:EMPTY"),
        DirNode {
            children: HashMap::from([(
                OsString::from("Users"),
                DirNode {
                    children: HashMap::new(),
                    files: HashMap::new(),
                },
            )]),
            files: HashMap::new(),
        },
    );
    // Legacy and mounted branches are never candidates.
    cache.root.children.insert(
        OsString::from(LEGACY_ROOT),
        branch_with(&["Users/legacy.jpg"]),
    );
    cache
        .root
        .children
        .insert(mounted_key.clone(), branch_with(&["Users/mounted.jpg"]));

    let adopted = cache.adopt_orphans_with(&boot, &mounted, &root_names);

    assert_eq!(adopted, 0);
    assert!(cache
        .root
        .children
        .contains_key(&OsString::from("vol:UNKNOWN_TOPLEVEL")));
    assert!(cache
        .root
        .children
        .contains_key(&OsString::from("vol:EMPTY")));
    assert!(cache
        .root
        .children
        .contains_key(&OsString::from(LEGACY_ROOT)));
    assert!(cache.root.children.contains_key(&mounted_key));
    assert!(!cache.root.children.contains_key(&boot));
}

#[test]
fn adoption_is_idempotent() {
    let mut cache = ScanCache::default();
    let boot = OsString::from("vol:NEW");
    let old = OsString::from("vol:OLD");
    cache
        .root
        .children
        .insert(old.clone(), branch_with(&["Users/old.jpg"]));
    cache
        .root
        .children
        .insert(boot.clone(), branch_with(&["Users/new.jpg"]));

    let mounted: HashSet<OsString> = HashSet::new();
    let root_names: HashSet<OsString> = HashSet::from([OsString::from("Users")]);

    assert_eq!(cache.adopt_orphans_with(&boot, &mounted, &root_names), 1);
    let files = cache.stats.files;
    assert_eq!(files, 2);
    assert_eq!(cache.adopt_orphans_with(&boot, &mounted, &root_names), 0);
    assert_eq!(
        cache.stats.files, files,
        "second adoption must not duplicate counts"
    );
    assert_eq!(cache.root.file_count(), 2);
}

#[test]
fn healed_cache_survives_save_and_load() {
    let dir = std::env::temp_dir().join(format!(
        "dedupe2-heal-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("cache.bin");

    let mut cache = ScanCache::default();
    let boot = OsString::from("vol:NEW");
    let old = OsString::from("vol:OLD");
    cache
        .root
        .children
        .insert(old.clone(), branch_with(&["Users/a.jpg"]));
    cache
        .root
        .children
        .insert(boot.clone(), branch_with(&["Users/b.jpg"]));
    let mounted: HashSet<OsString> = HashSet::new();
    let root_names: HashSet<OsString> = HashSet::from([OsString::from("Users")]);
    assert_eq!(cache.adopt_orphans_with(&boot, &mounted, &root_names), 1);
    assert_eq!(cache.stats.files, 2);

    cache.save(&store).expect("save healed cache");
    let loaded = ScanCache::load(&store);

    assert_eq!(loaded.root.file_count(), 2);
    assert_eq!(loaded.stats.files, 2);
    assert!(!loaded.root.children.contains_key(&old));
    // Load may re-adopt the synthetic boot branch into the real boot key
    // (its top-level names mirror "/"); either way the knowledge is kept
    // exactly once, never duplicated across branches.
    let branches_with_files = loaded
        .root
        .children
        .values()
        .filter(|branch| branch.file_count() > 0)
        .count();
    assert_eq!(branches_with_files, 1);

    let _ = std::fs::remove_file(&store);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn merge_never_drops_existing_knowledge() {
    let mut target = DirNode {
        children: HashMap::new(),
        files: HashMap::new(),
    };
    let mut source = DirNode {
        children: HashMap::new(),
        files: HashMap::new(),
    };

    let mut t = file_entry(10);
    t.full_bytes = Some(1);
    t.pixel = Some(PixelKey {
        width: 2,
        height: 3,
        cpp: 3,
        hash: 4,
    });
    t.date = Some(CreationDate::DateCreated("2020-01-02".into()));
    let mut s = file_entry(10);
    s.full_bytes = Some(99);
    s.pixel = Some(PixelKey {
        width: 9,
        height: 9,
        cpp: 4,
        hash: 9,
    });
    s.date = Some(CreationDate::DateCreated("1999-09-09".into()));
    s.full_content = Some(7);
    s.dhash = Some(8);
    target.files.insert(OsString::from("a.jpg"), t);
    source.files.insert(OsString::from("a.jpg"), s);

    merge_node(&mut target, source);

    let merged = target.files.get(&OsString::from("a.jpg")).unwrap();
    assert_eq!(merged.full_bytes, Some(1), "existing full hash kept");
    assert_eq!(merged.pixel.unwrap().hash, 4, "existing pixel key kept");
    assert_eq!(
        merged.date,
        Some(CreationDate::DateCreated("2020-01-02".into())),
        "existing date kept"
    );
    assert_eq!(merged.full_content, Some(7), "missing layer adopted");
    assert_eq!(merged.dhash, Some(8), "missing layer adopted");
}

#[test]
fn volume_keys_prefer_the_boot_data_uuid() {
    let mount = Path::new("/Volumes/Example");
    assert_eq!(
        volume_key_from(
            Some("DATA".into()),
            || Some("SYSTEM".into()),
            Path::new("/")
        ),
        OsString::from("vol:DATA")
    );
    assert_eq!(
        volume_key_from(None, || Some("MOUNT-UUID".into()), mount),
        OsString::from("vol:MOUNT-UUID")
    );
    assert_eq!(
        volume_key_from(None, || None, mount),
        OsString::from("mount:/Volumes/Example")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn boot_mount_detection_matches_the_root_device() {
    assert!(is_boot_mount(Path::new("/")));
    assert!(!is_boot_mount(Path::new("/definitely/not/a/mount")));
}
