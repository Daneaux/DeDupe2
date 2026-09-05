use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

use dedupe2::Scanner::scanner::{ScannedFile, ScannedTree, ScanTarget};
use dedupe2::dedupe::fileCompare::{
    difference, find_in_b_also_in_a, intersect, intersection, intersection_of_multiple_sets,
    things_in_B_also_in_A, union,
};
use dedupe2::exif::CreationDate;
use dedupe2::volumes::{FileType, Volume};

fn scanned_file(hash: u64) -> ScannedFile {
    ScannedFile {
        path: PathBuf::from(format!("file-{hash}.jpg")),
        size: 0,
        modified: SystemTime::now(),
        file_type: FileType { ext: "jpg".into() },
        hash,
        creation_date: CreationDate::Unknown,
    }
}

fn scanned_tree(hashes: &[u64]) -> ScannedTree {
    ScannedTree {
        root: PathBuf::from("/tmp"),
        files: hashes.iter().map(|&h| scanned_file(h)).collect(),
        scan_time: SystemTime::now(),
        target: ScanTarget {
            name: "test".into(),
            paths: vec![PathBuf::from("/tmp")],
            volume: Volume::new(PathBuf::from("/tmp")),
            extensions: vec![],
            prefix_bytes: 64 * 1024,
        },
    }
}

fn paths(items: &[&str]) -> HashSet<PathBuf> {
    items.iter().map(PathBuf::from).collect()
}

#[test]
fn intersection_returns_common_elements() {
    let a = paths(&["a", "b", "c"]);
    let b = paths(&["b", "c", "d"]);

    assert_eq!(intersection(&a, &b), paths(&["b", "c"]));
}

#[test]
fn intersect_returns_common_hashes() {
    let t1 = scanned_tree(&[1, 2, 3]);
    let t2 = scanned_tree(&[2, 3, 4]);

    let result = intersect(&t1, &t2);

    assert_eq!(result, [2, 3].iter().cloned().collect());
}

#[test]
fn find_in_b_also_in_a_returns_common_files() {
    let a = vec![scanned_file(1), scanned_file(2)];
    let b = vec![scanned_file(2), scanned_file(3), scanned_file(1)];

    let result = find_in_b_also_in_a(&a, &b);
    let hashes: HashSet<u64> = result.iter().map(|f| f.hash).collect();

    assert_eq!(hashes, [1, 2].iter().cloned().collect());
}

#[test]
fn things_in_b_also_in_a_matches_hash_set_approach() {
    let mut a = vec![scanned_file(5), scanned_file(1), scanned_file(3)];
    let mut b = vec![scanned_file(4), scanned_file(3), scanned_file(5), scanned_file(6)];

    let result = things_in_B_also_in_A(&mut a, &mut b);
    let hashes: HashSet<u64> = result.iter().map(|f| f.hash).collect();

    assert_eq!(hashes, [3, 5].iter().cloned().collect());
}

#[test]
#[should_panic]
fn things_in_b_also_in_a_panics_on_duplicate_hashes() {
    let mut a = vec![scanned_file(1), scanned_file(1)];
    let mut b = vec![scanned_file(2)];

    let _ = things_in_B_also_in_A(&mut a, &mut b);
}

#[test]
fn difference_returns_only_in_first() {
    let a = paths(&["a", "b", "c"]);
    let b = paths(&["b", "c", "d"]);

    assert_eq!(difference(&a, &b), paths(&["a"]));
}

#[test]
fn union_returns_all_elements() {
    let a = paths(&["a", "b"]);
    let b = paths(&["b", "c"]);

    assert_eq!(union(&a, &b), paths(&["a", "b", "c"]));
}

#[test]
fn intersection_of_multiple_sets_finds_common_across_all() {
    let a = paths(&["a", "b", "c"]);
    let b = paths(&["b", "c", "d"]);
    let c = paths(&["b", "c", "e"]);

    assert_eq!(intersection_of_multiple_sets(&[a, b, c]), paths(&["b", "c"]));
}

#[test]
fn intersection_of_multiple_sets_empty_input() {
    assert!(intersection_of_multiple_sets(&[]).is_empty());
}
