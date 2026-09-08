use std::fs;
use std::path::{Path, PathBuf};

use dedupe2::filemover::{merge_libraries, MergeOutcome, Operation};

/// Read a real sample image from the checked-in TestImages set.
fn sample(rel: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages").join(rel);
    fs::read(path).expect("sample image should exist")
}

/// Write `contents` to `root/<rel>`, creating parent directories as needed.
fn write_file(root: &Path, rel: &str, contents: &[u8]) -> PathBuf {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
    path
}

/// Relative (source-root) path of `path` under `root`, as a `/`-joined string.
fn rel_str(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Sorted relative paths that exist under `root`.
fn tree_rel(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
            } else if path.is_file() {
                out.push(rel_str(root, &path));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn dedupe_with_64kb_encoded_image_data_and_merge() {
    let root = tempfile::tempdir().unwrap();
    let lib_a = root.path().join("lib-a");
    let lib_b = root.path().join("lib-b");
    let destination = root.path().join("library");
    let purgatory = root.path().join("purgatory");

    // Step 1: build a tree with subfolders in `yyyy/mm-dd <description>` form.
    // The duplicate groups mix files whose *bytes* differ (exif variants) so
    // that only the 64 KB encoded-image-data hash can detect them as dupes.
    let jpeg = sample("jpg-exif-mod/image1.JPG");
    let jpeg_exif = sample("jpg-exif-mod/image1-exif.JPG");
    let heic = sample("HEIC-exif-mod/heic1.HEIC");
    let heic_exif = sample("HEIC-exif-mod/heic1-copy.heic");

    // Step 2+3+4: duplicates within one folder (A) and across folders (A/B),
    // bare "2004/06-01" vs "2004/06-01 Party" folders, different filenames.

    // JPEG duplicate group:
    //   beach-copy.jpg is an exact byte copy, vacation-photo.jpg is the
    //   exif-modified image. Both share the same 64 KB encoded image data.
    write_file(&lib_a, "2004/05-12 Hawaii/beach.jpg", &jpeg);
    write_file(&lib_a, "2004/05-12 Hawaii/beach-copy.jpg", &jpeg);
    write_file(&lib_b, "2004/05-12/vacation-photo.jpg", &jpeg_exif);

    // HEIC duplicate group: exif-modified copy.
    write_file(&lib_a, "2004/06-01/dinner.heic", &heic);
    write_file(&lib_b, "2004/06-01 Party/dinner-copy.heic", &heic_exif);

    // Step 5: find duplicates via the 64 KB encoded-image-data hash, keep the
    // shortest filename, merge same-date folders under the more descriptive
    // name, and move the removed duplicates to a mirror tree at "purgatory".
    let outcome: MergeOutcome = merge_libraries(
        &[lib_a.clone(), lib_b.clone()],
        &destination,
        &purgatory,
        Operation::Move,
    )
    .unwrap();

    // Kept survivors, with `05-12` -> `05-12 Hawaii` and `06-01` -> `06-01 Party`.
    let mut kept = tree_rel(&destination);
    kept.sort();
    assert_eq!(
        kept,
        vec![
            "2004/05-12 Hawaii/beach.jpg",
            "2004/06-01 Party/dinner.heic",
        ]
    );

    // Removed duplicates mirror each source's own structure (no folder merging).
    let mut purged = tree_rel(&purgatory);
    purged.sort();
    assert_eq!(
        purged,
        vec![
            "2004/05-12 Hawaii/beach-copy.jpg",
            "2004/05-12/vacation-photo.jpg",
            "2004/06-01 Party/dinner-copy.heic",
        ]
    );

    // The exif-modified duplicates MUST be treated as duplicates of the kept
    // file, proving the 64 KB encoded-image-data hash was used, not exact bytes.
    assert_eq!(outcome.kept.len(), 2);
    assert_eq!(outcome.purged.len(), 3);

    // Kept file content is preserved.
    assert_eq!(
        fs::read(destination.join("2004/05-12 Hawaii/beach.jpg")).unwrap(),
        jpeg
    );

    // Move consolidates: nothing is left behind in the sources.
    for lib in [&lib_a, &lib_b] {
        assert!(tree_rel(lib).is_empty(), "source {:?} should be emptied", lib);
    }
}
