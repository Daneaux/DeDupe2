use std::collections::HashSet;
use std::path::PathBuf;

use crate::Scanner::scanner::{ScannedFile};

pub fn intersection(set1: &HashSet<PathBuf>, set2: &HashSet<PathBuf>) -> HashSet<PathBuf> {
    set1.intersection(set2).cloned().collect()
}

/// Returns all files from B that have a corresponding hash in A.
pub fn files_in_b_also_in_a(a: &[ScannedFile], b: &[ScannedFile]) -> Vec<ScannedFile> {
    let a_hashes: HashSet<u64> = a.iter().map(|f| f.hash).collect();
    b.iter()
        .filter(|f| a_hashes.contains(&f.hash))
        .cloned()
        .collect()
}

/// Returns all files from B whose hashes do not exist in A.
pub fn files_in_b_not_in_a(a: &[ScannedFile], b: &[ScannedFile]) -> Vec<ScannedFile> {
    let a_hashes: HashSet<u64> = a.iter().map(|f| f.hash).collect();
    b.iter()
        .filter(|f| !a_hashes.contains(&f.hash))
        .cloned()
        .collect()
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
