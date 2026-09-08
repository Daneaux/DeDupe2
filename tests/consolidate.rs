use std::fs;
use std::path::{Path, PathBuf};

use dedupe2::filemover::consolidate_events;

fn sample(rel: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/TestImages").join(rel);
    fs::read(path).expect("sample image should exist")
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

fn tree(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let p = entry.path();
            if p.is_dir() {
                stack.push(p.clone());
            } else if p.is_file() {
                out.push(p.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn consolidate_merges_same_date_folders_and_dedupes() {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2008");
    let purgatory = root.path().join("2008-purgatory");

    let img = sample("jpg-exif-mod/image1.JPG");
    let img_exif = sample("jpg-exif-mod/image1-exif.JPG");
    let other = sample("jpg-dupe-diff-size/IMG_2571.jpg");
    let heic = sample("HEIC-exif-mod/heic1.HEIC");

    // 01-01 (plain) and 01-01 Hawaii (descriptive) overlap partially.
    write(&year, "01-01/a.jpg", &img); // duplicate of b.jpg (exif variant)
    write(&year, "01-01/c.jpg", &other); // unique
    write(&year, "01-01 Hawaii/b.jpg", &img_exif); // duplicate of a.jpg
    write(&year, "01-01 Hawaii/d.heic", &heic); // unique

    let out = consolidate_events(&year, Some(&purgatory)).unwrap();

    assert_eq!(out.folded_groups, 1);
    assert_eq!(out.purged_files, 1);

    // Keeper folder "01-01 Hawaii" holds a.jpg, c.jpg and d.heic.
    assert_eq!(
        tree(&year),
        vec![
            "01-01 Hawaii/a.jpg",
            "01-01 Hawaii/c.jpg",
            "01-01 Hawaii/d.heic",
        ]
    );

    // Removed duplicate mirrors the origin folder in purgatory.
    assert_eq!(tree(&purgatory), vec!["01-01 Hawaii/b.jpg"]);

    // Shorter name (a.jpg) survived; b.jpg was purged.
    assert_eq!(fs::read(year.join("01-01 Hawaii/a.jpg")).unwrap(), img);
}

#[test]
fn consolidate_defaults_purgatory_next_to_year() {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2008");

    let img = sample("jpg-exif-mod/image1.JPG");
    let img_exif = sample("jpg-exif-mod/image1-exif.JPG");

    write(&year, "01-01/a.jpg", &img);
    write(&year, "01-01 Hawaii/b.jpg", &img_exif);

    consolidate_events(&year, None).unwrap();

    // Default purgatory is a sibling named "<year>-purgatory".
    let default = root.path().join("2008-purgatory");
    assert!(default.join("01-01 Hawaii/b.jpg").exists());
}

#[test]
fn consolidate_leaves_single_folders_untouched() {
    let root = tempfile::tempdir().unwrap();
    let year = root.path().join("2008");

    let img = sample("jpg-exif-mod/image1.JPG");
    write(&year, "03-14 Birthday/only.jpg", &img);

    let out = consolidate_events(&year, None).unwrap();

    assert_eq!(out.folded_groups, 0);
    assert!(year.join("03-14 Birthday/only.jpg").exists());
}
