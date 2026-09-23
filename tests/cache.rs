use std::path::Path;

use dedupe2::Scanner::cache::{creation_dates_cached, lock, ScanCache};
use dedupe2::Scanner::compare::{compare_folders_cached, deep_scan_pairs_cached, DupPair};

fn sample(rel: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/TestImages")
            .join(rel),
    )
    .expect("sample image")
}

fn stats_delta(
    cache: &std::sync::Mutex<ScanCache>,
    before: dedupe2::Scanner::cache::CacheStats,
) -> (usize, usize) {
    let after = lock(cache).stats();
    (
        after.reused.saturating_sub(before.reused),
        after.computed.saturating_sub(before.computed),
    )
}

#[test]
fn cache_reuses_shallow_and_then_adds_deep_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photo.jpg");
    std::fs::write(&path, sample("jpg-exif-mod/image1.JPG")).unwrap();

    let mut cache = ScanCache::new();

    // Shallow pass computes once...
    cache.shallow_hash(&path, 64 * 1024);
    assert_eq!(cache.stats().computed, 1);
    // ...and is reused afterwards.
    cache.shallow_hash(&path, 64 * 1024);
    assert_eq!(cache.stats().reused, 1);

    // A later deep pass adds data to the same entry (nothing recomputed).
    cache.full_content_hash(&path).unwrap();
    assert_eq!(cache.stats().reused, 1, "shallow data must survive");
    assert_eq!(cache.stats().computed, 2);
    cache.full_bytes_hash(&path).unwrap();
    cache.pixel_key(&path).expect("jpg decodes");
    assert_eq!(cache.stats().computed, 4);

    // All layers now reuse.
    cache.shallow_hash(&path, 64 * 1024);
    cache.full_content_hash(&path).unwrap();
    cache.full_bytes_hash(&path).unwrap();
    cache.pixel_key(&path);
    assert_eq!(cache.stats().reused, 5);
}

#[test]
fn cache_invalidates_only_changed_files() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.jpg");
    let b = dir.path().join("b.jpg");
    std::fs::write(&a, sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(&b, sample("jpg-exif-mod/image1-exif.JPG")).unwrap();

    let mut cache = ScanCache::new();
    cache.shallow_hash(&a, 64 * 1024);
    cache.shallow_hash(&b, 64 * 1024);
    let computed_before = cache.stats().computed;

    // Change a's content (different size guarantees a detected change).
    std::fs::write(&a, b"a completely different and longer payload").unwrap();
    cache.shallow_hash(&a, 64 * 1024);
    cache.shallow_hash(&b, 64 * 1024);

    let stats = cache.stats();
    assert_eq!(
        stats.computed,
        computed_before + 1,
        "only the changed file is recomputed"
    );
    assert_eq!(stats.reused, 1, "the untouched file reuses its hash");
}

#[test]
fn cache_invalidates_branches_hierarchically() {
    let root = tempfile::tempdir().unwrap();
    let one = root.path().join("one");
    let two = root.path().join("two");
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    std::fs::write(one.join("x.jpg"), sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(two.join("y.jpg"), sample("jpg-exif-mod/image1.JPG")).unwrap();

    let mut cache = ScanCache::new();
    cache.shallow_hash(&one.join("x.jpg"), 64 * 1024);
    cache.shallow_hash(&two.join("y.jpg"), 64 * 1024);
    assert_eq!(cache.stats().files, 2);

    cache.invalidate_branch(&one);
    assert_eq!(cache.stats().files, 1, "branch entries dropped");

    cache.shallow_hash(&one.join("x.jpg"), 64 * 1024); // recomputed
    cache.shallow_hash(&two.join("y.jpg"), 64 * 1024); // reused
    let stats = cache.stats();
    assert_eq!(stats.computed, 3);
    assert_eq!(stats.reused, 1);
}

#[test]
fn repeated_compare_reuses_the_whole_tree() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("one.jpg"), sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(a.join("two.jpg"), sample("jpg-exif-mod/image1-exif.JPG")).unwrap();
    std::fs::write(b.join("one.jpg"), sample("jpg-exif-mod/image1.JPG")).unwrap();

    let cache = std::sync::Mutex::new(ScanCache::new());
    let first = compare_folders_cached(&a, &b, &cache, &|_, _| {}).unwrap();
    assert_eq!(first.duplicates.len(), 1);

    let before = lock(&cache).stats();
    let second = compare_folders_cached(&a, &b, &cache, &|_, _| {}).unwrap();
    let (reused, computed) = stats_delta(&cache, before);

    assert_eq!(second.duplicates.len(), 1);
    assert_eq!(computed, 0, "second scan must not recompute any hash");
    assert!(reused >= 3, "second scan reuses every file ({reused})");
}

#[test]
fn deep_scan_after_shallow_adds_reuses_and_stays_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("one.jpg"), sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(b.join("one.jpg"), sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(b.join("two.jpg"), sample("jpg-exif-mod/image1-exif.JPG")).unwrap();

    let cache = std::sync::Mutex::new(ScanCache::new());
    compare_folders_cached(&a, &b, &cache, &|_, _| {}).unwrap();

    // Deep scan on the shallowly-found pairs: shallow data already sits in
    // the cache, and the full hashes/pixels computed now are added to it.
    let pairs = vec![
        DupPair {
            a: a.join("one.jpg"),
            b: b.join("one.jpg"),
        },
        DupPair {
            a: a.join("one.jpg"),
            b: b.join("two.jpg"),
        },
    ];
    let before = lock(&cache).stats();
    let first = deep_scan_pairs_cached(&pairs, &cache, &|_, _| {});
    let (_, computed_first) = stats_delta(&cache, before);
    assert_eq!(first.kept, 2, "{first:?}");
    assert!(computed_first > 0, "deep data is computed on the first pass");

    let before = lock(&cache).stats();
    let second = deep_scan_pairs_cached(&pairs, &cache, &|_, _| {});
    let (reused, computed) = stats_delta(&cache, before);
    assert_eq!(second.kept, first.kept);
    assert_eq!(computed, 0, "second deep scan recomputes nothing");
    assert!(reused > 0, "second deep scan reuses the stored data");
}

#[test]
fn cached_dates_are_reused_across_exif_passes() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.jpg");
    let b = dir.path().join("b.jpg");
    std::fs::write(&a, sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(&b, sample("jpg-exif-mod/image1-exif.JPG")).unwrap();
    let paths = vec![a, b];

    let cache = std::sync::Mutex::new(ScanCache::new());
    let first = creation_dates_cached(&cache, &paths, &|_, _| {});
    assert!(first.iter().all(|d| *d != dedupe2::exif::CreationDate::Unknown));

    let before = lock(&cache).stats();
    let second = creation_dates_cached(&cache, &paths, &|_, _| {});
    let (reused, computed) = stats_delta(&cache, before);
    assert_eq!(first, second);
    assert_eq!(computed, 0);
    assert_eq!(reused, 2, "both dates served from the cache");
}

#[test]
fn lookups_after_parent_invalidation_rebuild_the_branch_lazily() {
    // a/b/c is invalidated, which drops the deeper a/b/c/d/e subtree with it.
    // A later compare of a/b/c/d/e must rebuild only what it looks up.
    let root = tempfile::tempdir().unwrap();
    let deep_dir = root.path().join("a/b/c/d/e");
    let shallow_dir = root.path().join("a/b/c");
    std::fs::create_dir_all(&deep_dir).unwrap();
    let deep_file = deep_dir.join("x.jpg");
    let shallow_file = shallow_dir.join("y.jpg");
    std::fs::write(&deep_file, sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(&shallow_file, sample("jpg-exif-mod/image1-exif.JPG")).unwrap();

    let mut cache = ScanCache::new();
    cache.shallow_hash(&deep_file, 64 * 1024);
    cache.full_content_hash(&deep_file).unwrap();
    cache.shallow_hash(&shallow_file, 64 * 1024);
    assert_eq!(cache.stats().files, 2);

    cache.invalidate_branch(&root.path().join("a/b/c"));
    assert_eq!(cache.stats().files, 0, "whole subtree dropped with the parent");

    // Rebuild: the deep file is recomputed (its data was dropped)...
    let before = cache.stats().computed;
    cache.shallow_hash(&deep_file, 64 * 1024);
    assert_eq!(cache.stats().computed, before + 1);
    cache.full_content_hash(&deep_file).unwrap();
    // ...and everything reuses from then on, with correct values.
    let hash_before = cache.shallow_hash(&deep_file, 64 * 1024).0;
    let before = cache.stats();
    let hash_again = cache.shallow_hash(&deep_file, 64 * 1024).0;
    assert_eq!(hash_before, hash_again);
    assert_eq!(cache.stats().reused, before.reused + 1);
    assert_eq!(cache.stats().computed, before.computed);
}

#[test]
fn deleted_files_are_pruned_on_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let keep = dir.path().join("keep.jpg");
    let gone = dir.path().join("gone.jpg");
    std::fs::write(&keep, sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(&gone, sample("jpg-exif-mod/image1.JPG")).unwrap();

    let mut cache = ScanCache::new();
    cache.shallow_hash(&keep, 64 * 1024);
    cache.shallow_hash(&gone, 64 * 1024);
    assert_eq!(cache.stats().files, 2);

    std::fs::remove_file(&gone).unwrap();
    // A lookup of the vanished file is a miss AND prunes its entry...
    assert_eq!(cache.shallow_hash(&gone, 64 * 1024), (0, false));
    assert_eq!(cache.stats().files, 1, "deleted file's entry is gone");

    // ...while the surviving file reuses its data.
    let before = cache.stats();
    cache.shallow_hash(&keep, 64 * 1024);
    assert_eq!(cache.stats().reused, before.reused + 1);
}

#[test]
fn invalidation_of_a_file_keeps_its_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.jpg");
    let b = dir.path().join("b.jpg");
    std::fs::write(&a, sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(&b, sample("jpg-exif-mod/image1.JPG")).unwrap();

    let mut cache = ScanCache::new();
    cache.shallow_hash(&a, 64 * 1024);
    cache.shallow_hash(&b, 64 * 1024);

    cache.invalidate_file(&a);
    assert_eq!(cache.stats().files, 1);

    let before = cache.stats().computed;
    cache.shallow_hash(&a, 64 * 1024); // recomputed
    cache.shallow_hash(&b, 64 * 1024); // reused
    let stats = cache.stats();
    assert_eq!(stats.computed, before + 1);
    assert_eq!(stats.reused, 1);
}

#[test]
fn missing_files_do_not_materialize_directory_nodes() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = ScanCache::new();

    // A lookup of a path that does not exist creates nothing at all.
    let ghost = root.path().join("no/such/tree/ghost.jpg");
    assert_eq!(cache.shallow_hash(&ghost, 64 * 1024), (0, false));
    assert_eq!(cache.stats().files, 0);

    // Once the file exists, the lookup computes and stores it.
    let real_dir = root.path().join("no/such/tree");
    std::fs::create_dir_all(&real_dir).unwrap();
    std::fs::write(real_dir.join("ghost.jpg"), sample("jpg-exif-mod/image1.JPG")).unwrap();
    cache.shallow_hash(&ghost, 64 * 1024);
    assert_eq!(cache.stats().files, 1);
}

#[test]
fn mtime_only_changes_invalidate_entries() {
    use std::time::{Duration, SystemTime};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photo.jpg");
    let original = sample("jpg-exif-mod/image1.JPG");
    std::fs::write(&path, &original).unwrap();

    let mut cache = ScanCache::new();
    cache.shallow_hash(&path, 64 * 1024);
    let full_before = cache.full_bytes_hash(&path).unwrap();
    let computed_before = cache.stats().computed;

    // Same size, new content, explicitly distinct mtime (flip a late byte so
    // the 64kb shallow window is unaffected — the point is invalidation by
    // mtime, verified through the whole-file hash).
    let mut tweaked = original.clone();
    let last = tweaked.len() - 1;
    tweaked[last] ^= 0x01;
    std::fs::write(&path, &tweaked).unwrap();
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        .unwrap();
    drop(f);

    cache.shallow_hash(&path, 64 * 1024);
    assert_eq!(
        cache.stats().computed,
        computed_before + 1,
        "mtime-only change must invalidate the entry"
    );

    // Sanity: the rebuilt data reflects the new bytes.
    let full_after = cache.full_bytes_hash(&path).unwrap();
    assert_ne!(full_before, full_after);
}

#[test]
fn cached_unknown_dates_are_hits_too() {
    // No readable metadata and a folder with no date pattern: the pipeline
    // resolves Unknown. That answer is cached and reused like any other.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("misc/mystery.jpg");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"\xFF\xD8\xFF\xE0 not a real jpeg").unwrap();

    let cache = std::sync::Mutex::new(ScanCache::new());
    let first = creation_dates_cached(&cache, std::slice::from_ref(&path), &|_, _| {});
    assert_eq!(first[0], dedupe2::exif::CreationDate::Unknown);

    let before = lock(&cache).stats();
    let second = creation_dates_cached(&cache, std::slice::from_ref(&path), &|_, _| {});
    let (reused, computed) = stats_delta(&cache, before);
    assert_eq!(second[0], dedupe2::exif::CreationDate::Unknown);
    assert_eq!(computed, 0, "a cached Unknown is not recomputed");
    assert_eq!(reused, 1);
}

#[test]
fn lock_recovers_from_poisoning() {
    use std::sync::{Arc, Mutex};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photo.jpg");
    std::fs::write(&path, sample("jpg-exif-mod/image1.JPG")).unwrap();

    let cache = Arc::new(Mutex::new(ScanCache::new()));
    let poisoner = cache.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock().unwrap();
        panic!("poison the cache mutex");
    })
    .join();
    assert!(cache.lock().is_err(), "precondition: mutex is poisoned");

    // The lock() helper keeps the cache usable despite the poison.
    let mut guard = lock(&cache);
    let (hash, decoded) = guard.shallow_hash(&path, 64 * 1024);
    assert!(decoded && hash != 0);
    assert_eq!(guard.stats().files, 1);
}

#[test]
fn branch_files_counts_only_the_requested_subtree() {
    let root = tempfile::tempdir().unwrap();
    let one = root.path().join("one/deep");
    let two = root.path().join("two");
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    let f1 = one.join("a.jpg");
    let f2 = one.join("b.jpg");
    let f3 = two.join("c.jpg");
    for f in [&f1, &f2, &f3] {
        std::fs::write(f, sample("jpg-exif-mod/image1.JPG")).unwrap();
    }

    let mut cache = ScanCache::new();
    for f in [&f1, &f2, &f3] {
        cache.shallow_hash(f, 64 * 1024);
    }

    assert_eq!(cache.branch_files(root.path()), 3, "whole tree");
    assert_eq!(cache.branch_files(&root.path().join("one")), 2, "subtree");
    assert_eq!(cache.branch_files(&root.path().join("one/deep")), 2);
    assert_eq!(cache.branch_files(&root.path().join("two")), 1);
    assert_eq!(cache.branch_files(&f1), 1, "single file");
    assert_eq!(cache.branch_files(&root.path().join("nope")), 0, "unknown tree");
    assert_eq!(cache.branch_files(&root.path().join("one/nope.jpg")), 0);

    // Invalidation shrinks the reported coverage.
    cache.invalidate_branch(&root.path().join("one"));
    assert_eq!(cache.branch_files(root.path()), 1);
}

#[test]
fn branch_coverage_reports_each_layer() {
    use dedupe2::Scanner::cache::BranchCoverage;

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a.jpg");
    let b = root.path().join("b.jpg");
    std::fs::write(&a, sample("jpg-exif-mod/image1.JPG")).unwrap();
    std::fs::write(&b, sample("jpg-exif-mod/image1-exif.JPG")).unwrap();

    let mut cache = ScanCache::new();

    // Nothing cached yet.
    assert_eq!(cache.branch_coverage(root.path()), BranchCoverage::default());

    // Shallow pass only.
    cache.shallow_hash(&a, 64 * 1024);
    cache.shallow_hash(&b, 64 * 1024);
    let cov = cache.branch_coverage(root.path());
    assert_eq!(cov.files, 2);
    assert_eq!(cov.shallow, 2);
    assert_eq!(cov.deep, 0);
    assert_eq!(cov.exif, 0);

    // Deep data on one file, phash on one, exif dates on both.
    cache.full_content_hash(&a).unwrap();
    cache.phash(&a);
    cache.store_creation_date(&a, dedupe2::exif::CreationDate::DateCreated("2020-01-01".into()));
    cache.store_creation_date(&b, dedupe2::exif::CreationDate::Unknown);
    let cov = cache.branch_coverage(root.path());
    assert_eq!(cov.shallow, 2);
    assert_eq!(cov.deep, 1);
    assert_eq!(cov.phash, 1);
    assert_eq!(cov.exif, 2, "a cached Unknown still counts as resolved");

    // phash is reused like every other layer.
    let computed_before = cache.stats().computed;
    cache.phash(&a);
    assert_eq!(cache.stats().computed, computed_before, "phash served from cache");
    assert_eq!(cache.stats().reused, 1);
}

#[test]
fn cache_round_trips_through_the_persisted_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.jpg");
    std::fs::write(&file, sample("jpg-exif-mod/image1.JPG")).unwrap();
    let store = dir.path().join("cache.bin");

    let mut cache = ScanCache::new();
    let shallow = cache.shallow_hash(&file, 64 * 1024);
    let full = cache.full_content_hash(&file).unwrap();
    let phash = cache.phash(&file).unwrap();
    cache.store_creation_date(
        &file,
        dedupe2::exif::CreationDate::DateCreated("2020-05-04 03:02:01".into()),
    );
    assert!(cache.dirty());
    cache.save(&store).unwrap();
    assert!(!cache.dirty(), "save clears the dirty flag");

    // A fresh process loads the snapshot and gets everything back.
    let mut loaded = ScanCache::load(&store);
    assert_eq!(loaded.stats().files, 1);
    let before = loaded.stats();
    assert_eq!(loaded.shallow_hash(&file, 64 * 1024), shallow);
    assert_eq!(loaded.full_content_hash(&file).unwrap(), full);
    assert_eq!(loaded.phash(&file).unwrap(), phash);
    assert_eq!(
        loaded.creation_date(&file),
        Some(dedupe2::exif::CreationDate::DateCreated("2020-05-04 03:02:01".into()))
    );
    let after = loaded.stats();
    assert_eq!(after.computed, before.computed, "nothing recomputed after load");
    assert_eq!(after.reused, before.reused + 4, "every layer served from the snapshot");
}

#[test]
fn loaded_entries_still_validate_against_the_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.jpg");
    std::fs::write(&file, sample("jpg-exif-mod/image1.JPG")).unwrap();
    let store = dir.path().join("cache.bin");

    let mut cache = ScanCache::new();
    cache.shallow_hash(&file, 64 * 1024);
    cache.save(&store).unwrap();

    // The file changes while the server is down.
    std::fs::write(&file, b"a different and longer payload").unwrap();

    let mut loaded = ScanCache::load(&store);
    let before = loaded.stats();
    let (hash, _) = loaded.shallow_hash(&file, 64 * 1024);
    let after = loaded.stats();
    assert_eq!(after.computed, before.computed + 1, "stale entry recomputed");
    assert_ne!(hash, 0);
}

#[test]
fn corrupt_cache_files_load_as_empty_and_usable() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("cache.bin");
    std::fs::write(&store, b"definitely not a bincode snapshot").unwrap();

    let mut cache = ScanCache::load(&store);
    assert_eq!(cache.stats().files, 0);

    let file = dir.path().join("photo.jpg");
    std::fs::write(&file, sample("jpg-exif-mod/image1.JPG")).unwrap();
    cache.shallow_hash(&file, 64 * 1024);
    assert_eq!(cache.stats().files, 1, "cache still works after a corrupt load");
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_survive_persistence() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().unwrap();
    let name = OsStr::from_bytes(b"photo-\xff\xfe.jpg");
    let file = dir.path().join(name);
    // APFS rejects non-UTF8 names outright (EILSEQ), so on such volumes the
    // scenario cannot exist; the serde derive keeps it lossless where it can.
    if std::fs::write(&file, sample("jpg-exif-mod/image1.JPG")).is_err() {
        eprintln!("filesystem rejects non-UTF8 names — skipping");
        return;
    }
    let store = dir.path().join("cache.bin");

    let mut cache = ScanCache::new();
    let expected = cache.shallow_hash(&file, 64 * 1024);
    cache.save(&store).unwrap();

    let mut loaded = ScanCache::load(&store);
    assert_eq!(loaded.stats().files, 1, "non-UTF8 path persisted");
    let before = loaded.stats();
    assert_eq!(loaded.shallow_hash(&file, 64 * 1024), expected);
    assert_eq!(loaded.stats().reused, before.reused + 1, "served from the snapshot");
}

#[test]
fn settings_round_trip_and_tolerate_corruption() {
    use dedupe2::web::settings::Settings;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");

    // Missing file: defaults.
    assert_eq!(Settings::load_from(&path).cache_path, None);

    let mut settings = Settings::default();
    settings.cache_path = Some(dir.path().join("cache.bin"));
    settings.save_to(&path).unwrap();
    assert_eq!(
        Settings::load_from(&path).cache_path,
        Some(dir.path().join("cache.bin"))
    );

    // Corrupt file: defaults, never an error.
    std::fs::write(&path, b"not json").unwrap();
    assert_eq!(Settings::load_from(&path).cache_path, None);
}

/// Environment-specific: verifies every mounted volume reports a persistent
/// UUID (vol:<uuid> keys), that distinct volumes get distinct keys/branches,
/// and that the same volume exposed at two mount points (macOS firmlinks:
/// "/" and "/Volumes/Macintosh HD") shares one key. Run with
/// `cargo test --test cache real_volumes -- --ignored --nocapture`.
#[test]
#[ignore]
fn real_volumes_have_uuids_and_unique_cache_branches() {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    // Enumerate mounted volumes: "/" plus every /Volumes entry.
    let mut mounts: Vec<PathBuf> = vec![PathBuf::from("/")];
    if let Ok(entries) = std::fs::read_dir("/Volumes") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                mounts.push(path);
            }
        }
    }

    let mut cache = ScanCache::new();

    // 1. Every mount resolves to a persistent UUID key.
    let mut by_key: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for mount in &mounts {
        let key = cache
            .volume_key(mount)
            .unwrap_or_else(|| panic!("{} did not resolve to a volume", mount.display()));
        println!("volume {:40} -> {key}", mount.display());
        assert!(
            key.starts_with("vol:"),
            "{} has no Volume UUID (key: {key})",
            mount.display()
        );
        by_key.entry(key).or_default().push(mount.clone());
    }
    println!("{} mount(s), {} distinct volume(s)", mounts.len(), by_key.len());

    // Multiple mount points of the SAME volume share a key (and therefore a
    // single cache branch); distinct volumes must not collide.
    let distinct = by_key.len();
    assert!(distinct >= 2, "expected at least two distinct volumes, got {distinct}");

    // 2. Cache something on every distinct volume. Writable volumes get a
    // probe with the SAME relative path (so a branch collision would be
    // visible); unwritable ones fall back to the first existing supported
    // file found by a bounded walk.
    fn first_supported_file(root: &std::path::Path) -> Option<PathBuf> {
        let mut stack = vec![root.to_path_buf()];
        let mut seen = 0usize;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                seen += 1;
                if seen > 20_000 {
                    return None;
                }
                let path = entry.path();
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    stack.push(path);
                } else if kind.is_file() && dedupe2::image_reader::is_supported_image(&path) {
                    return Some(path);
                }
            }
        }
        None
    }

    let boot_key = cache.volume_key(std::path::Path::new("/")).unwrap_or_default();

    let mut probes: Vec<(String, PathBuf, u64)> = Vec::new(); // same rel path
    let mut others: Vec<(String, PathBuf, u64)> = Vec::new(); // any existing file
    let mut probe_dirs: Vec<PathBuf> = Vec::new();
    for (key, mount_list) in &by_key {
        // Prefer the real /Volumes mount as the walking root ("/" also
        // contains the other volumes under /Volumes).
        let mount = mount_list
            .iter()
            .find(|m| m.starts_with("/Volumes"))
            .unwrap_or(&mount_list[0]);

        let probe_dir = mount.join(format!(".dedupe2-probe-{}", std::process::id()));
        match std::fs::create_dir(&probe_dir) {
            Ok(()) => {
                let probe_file = probe_dir.join("probe.bin");
                std::fs::write(&probe_file, format!("probe-for-{key}").as_bytes()).unwrap();
                let (hash, _) = cache.shallow_hash(&probe_file, 64 * 1024);
                probes.push((key.clone(), probe_file, hash));
                probe_dirs.push(probe_dir);
            }
            Err(e) => {
                if *key == boot_key || !mount.starts_with("/Volumes") {
                    println!("skipping {} ({e})", mount.display());
                    continue;
                }
                println!("{} is not writable ({e}) — using an existing file", mount.display());
                if let Some(existing) = first_supported_file(mount) {
                    let (hash, decoded) = cache.shallow_hash(&existing, 64 * 1024);
                    println!("  cached {} (hash {hash:016x}, decoded={decoded})", existing.display());
                    others.push((key.clone(), existing, hash));
                } else {
                    println!("  no supported file found on {}", mount.display());
                }
            }
        }
    }

    let cached_volumes = probes.len() + others.len();
    assert!(
        cached_volumes >= 2,
        "need at least two volumes with cache entries (got {cached_volumes})"
    );

    // Every cached file must be served from its own volume's branch.
    for (key, path, expected) in probes.iter().chain(others.iter()) {
        assert_eq!(
            cache.shallow_hash(path, 64 * 1024).0,
            *expected,
            "{key}: lookup returned a different volume's data ({})",
            path.display()
        );
        assert_eq!(cache.branch_coverage(path).files, 1, "{key}: coverage");
    }

    // Same relative path on different volumes: hashes stay distinct.
    if probes.len() >= 2 {
        let rel_paths: Vec<String> = probes
            .iter()
            .map(|(_, path, _)| {
                path.components()
                    .rev()
                    .take(2)
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect();
        assert!(
            rel_paths.windows(2).all(|w| w[0] == w[1]),
            "probes should share their relative path: {rel_paths:?}"
        );
        let mut hashes: Vec<u64> = probes.iter().map(|(_, _, hash)| *hash).collect();
        hashes.sort();
        hashes.dedup();
        assert_eq!(hashes.len(), probes.len(), "probe contents are distinct");
    }

    println!(
        "verified {cached_volumes} distinct volume branches ({} probes, {} existing files)",
        probes.len(),
        others.len()
    );

    for dir in probe_dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}
