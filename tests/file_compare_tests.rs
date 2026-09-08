use dedupe2::Scanner::scanner::ScannedFile;
use dedupe2::dedupe::fileCompare::{files_in_b_also_in_a, files_in_b_not_in_a};
use std::path::PathBuf;
use std::time::SystemTime;
use dedupe2::volumes::FileType;
use dedupe2::exif::CreationDate;

fn mock_file(path: &str, hash: u64) -> ScannedFile {
    ScannedFile {
        path: PathBuf::from(path),
        size: 100,
        modified: SystemTime::now(),
        file_type: FileType { ext: "jpg".to_string() },
        hash,
        creation_date: CreationDate::Unknown,
    }
}

#[test]
fn test_files_in_b_also_in_a() {
    // Case 1: Standard positive/negative
    let a = vec![mock_file("a/1", 10), mock_file("a/2", 20)];
    let b = vec![mock_file("b/1", 10), mock_file("b/2", 30)];
    let result = files_in_b_also_in_a(&a, &b);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("b/1"));

    // Case 2: Duplicates in B (both should be returned if hash matches A)
    let a_dup = vec![mock_file("a/1", 10)];
    let b_dup = vec![mock_file("b/1", 10), mock_file("b/2", 10)];
    let result_dup_b = files_in_b_also_in_a(&a_dup, &b_dup);
    assert_eq!(result_dup_b.len(), 2);

    // Case 3: Duplicates in A (should only return the file from B)
    let a_many = vec![mock_file("a/1", 10), mock_file("a/2", 10)];
    let b_one = vec![mock_file("b/1", 10)];
    let result_dup_a = files_in_b_also_in_a(&a_many, &b_one);
    assert_eq!(result_dup_a.len(), 1);
    assert_eq!(result_dup_a[0].path, PathBuf::from("b/1"));

    // Case 4: Empty A
    let result_empty_a = files_in_b_also_in_a(&[], &b);
    assert!(result_empty_a.is_empty());

    // Case 5: Empty B
    let result_empty_b = files_in_b_also_in_a(&a, &[]);
    assert!(result_empty_b.is_empty());
}

#[test]
fn test_files_in_b_not_in_a() {
    // Case 1: Standard positive/negative
    let a = vec![mock_file("a/1", 10), mock_file("a/2", 20)];
    let b = vec![mock_file("b/1", 10), mock_file("b/2", 30)];
    let result = files_in_b_not_in_a(&a, &b);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("b/2"));

    // Case 2: Duplicates in B (none in A)
    let a_empty = vec![];
    let b_dup = vec![mock_file("b/1", 40), mock_file("b/2", 40)];
    let result_dup_b = files_in_b_not_in_a(&a_empty, &b_dup);
    assert_eq!(result_dup_b.len(), 2);

    // Case 3: Duplicates in B (both hashes exist in A)
    let a_exists = vec![mock_file("a/1", 40)];
    let b_dup_exists = vec![mock_file("b/1", 40), mock_file("b/2", 40)];
    let result_dup_exists = files_in_b_not_in_a(&a_exists, &b_dup_exists);
    assert!(result_dup_exists.is_empty());

    // Case 4: Duplicates in A
    let a_many = vec![mock_file("a/1", 50), mock_file("a/2", 50)];
    let b_one = vec![mock_file("b/1", 50)];
    let result_dup_a = files_in_b_not_in_a(&a_many, &b_one);
    assert!(result_dup_a.is_empty());

    // Case 5: Empty A (everything in B is not in A)
    let a_none = vec![];
    let b_some = vec![mock_file("b/1", 60), mock_file("b/2", 70)];
    let result_empty_a = files_in_b_not_in_a(&a_none, &b_some);
    assert_eq!(result_empty_a.len(), 2);

    // Case 6: Empty B
    let result_empty_b = files_in_b_not_in_a(&a, &[]);
    assert!(result_empty_b.is_empty());
}

#[test]
fn test_large_dataset_with_duplicates() {
    // Set A: 50 files. Hashes 0..24 appearing twice each.
    // Total unique hashes in A: 25.
    let mut a = Vec::new();
    for i in 0..25 {
        a.push(mock_file(&format!("a/first_{}", i), i as u64));
        a.push(mock_file(&format!("a/second_{}", i), i as u64));
    }

    // Set B:
    // - Hashes 0..9 appearing 3 times each (Total 30 files, all in A)
    // - Hashes 10..14 appearing 1 time each (Total 5 files, all in A)
    // - Hashes 100..119 appearing 1 time each (Total 20 files, all NOT in A)
    let mut b = Vec::new();
    for i in 0..10 {
        for j in 0..3 {
            b.push(mock_file(&format!("b/match_{}_{}", i, j), i as u64));
        }
    }
    for i in 10..15 {
        b.push(mock_file(&format!("b/match_{}", i), i as u64));
    }
    for i in 100..120 {
        b.push(mock_file(&format!("b/unique_{}", i), i as u64));
    }

    // total b = 30 + 5 + 20 = 55 files.
    assert_eq!(b.len(), 55);

    // Test "also in a"
    // Expected: the 30 + 5 = 35 files that match hashes 0..14.
    let also_in_a = files_in_b_also_in_a(&a, &b);
    assert_eq!(also_in_a.len(), 35, "Should find exactly 35 files from B that match A");
    for file in also_in_a {
        assert!(file.hash < 25, "File hash {} should be within A's range (0..24)", file.hash);
    }

    // Test "not in a"
    // Expected: the 20 files with hashes 100..119.
    let not_in_a = files_in_b_not_in_a(&a, &b);
    assert_eq!(not_in_a.len(), 20, "Should find exactly 20 files from B that are not in A");
    for file in not_in_a {
        assert!(file.hash >= 100, "File hash {} should be in the unique range (100..119)", file.hash);
    }
}
