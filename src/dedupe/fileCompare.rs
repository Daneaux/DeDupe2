use std::path::PathBuf;
use std::time::Instant;
use dedupe2::volumes::Volume;
use std::collections::{HashMap, HashSet};
use dedupe2::Scanner::fastScan::fast_scan;
use dedupe2::Scanner::scanner::{deep_scan, shallow_scan, ScanTarget};

pub fn intersection(set1: &HashSet<PathBuf>, set2: &HashSet<PathBuf>) -> HashSet<PathBuf> {
    set1.intersection(set2).cloned().collect()
}

pub fn intersect(tree1: &ScannedTree, tree2: &ScannedTree) -> HashSet<PathBuf> {
    let set1: HashSet<PathBuf> = tree1.files.iter().map(|f| f.hash).collect();
    let set2: HashSet<PathBuf> = tree2.files.iter().map(|f| f.hash).collect();
    intersection(&set1, &set2)
}


fn find_in_b_also_in_a(a: &[ScannedFile], b: &[ScannedFile]) -> Vec<ScannedFile> {
    let a_hashes: HashSet<u64> = a.iter().map(|x| x.hash).collect();

    b.iter()
        .filter(|item| a_hashes.contains(&item.hash))
        .cloned()
        .collect()
}   

fn things_in_B_also_in_A(a: &mut [ScannedFile], b: &mut [ScannedFile]) -> Vec<ScannedFile> {
    a.sort_unstable_by_key(|x| x.hash);
    b.sort_unstable_by_key(|x| x.hash);

    // Assert no duplicates (panics if found)
    assert!(a.windows(2).all(|w| w[0] != w[1]), "duplicates found. 'a' must not contain duplicates");

    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].hash.cmp(&b[j].hash) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                result.push(a[i].clone());
                i += 1;
                j += 1;
            }
        }
    }
    result
}   

pub fn difference(set1: &HashSet<PathBuf>, set2: &HashSet<PathBuf>) -> HashSet<PathBuf> {
    set1.difference(set2).cloned().collect()
}

pub fn union(set1: &HashSet<PathBuf>, set2: &HashSet<PathBuf>) -> HashSet<PathBuf> {
    set1.union(set2).cloned().collect()
}

pub fn intersection_of_multiple_sets(sets: &[HashSet<PathBuf>]) -> HashSet<PathBuf> {
    if sets.is_empty() {
        return HashSet::new();
    }

    let mut result = sets[0].clone();
    for set in &sets[1..] {
        result = intersection(&result, set);
    }
    result
}