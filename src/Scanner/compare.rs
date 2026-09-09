use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::Scanner::fastScan::hash_image_data_parallel;
use crate::exif::{creation_date, CreationDate};
use crate::image_reader::is_supported_image;

#[derive(Debug)]
pub struct Comparison {
    pub a: PathBuf,
    pub b: PathBuf,
    pub duplicates: Vec<DuplicateGroup>,
    pub a_only: Vec<PathBuf>,
    pub b_only: Vec<PathBuf>,
    /// Candidate-tree (B) files whose decoder failed or panicked. They are
    /// not classified as duplicates or originals; they are listed for
    /// review. Unreadable files in the destination tree are ignored.
    pub unreadable: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct DuplicateGroup {
    pub a: Vec<PathBuf>,
    pub b: Vec<PathBuf>,
}

/// An original from tree B after an EXIF scan.
#[derive(Debug)]
pub struct DatedOriginal {
    pub path: PathBuf,
    pub creation_date: CreationDate,
}

pub fn compare_folders(a: &Path, b: &Path) -> Result<Comparison, Box<dyn std::error::Error>> {
    compare_folders_with_progress(a, b, &|_, _| {})
}

/// Fast scan: 64kb encoded-image-data hash only. EXIF is a separate,
/// explicit pass (see `scan_exif_with_progress`).
pub fn compare_folders_with_progress(
    a: &Path,
    b: &Path,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<Comparison, Box<dyn std::error::Error>> {
    let a_files = collect_image_files(a)?;
    let b_files = collect_image_files(b)?;

    let a_len = a_files.len();
    let mut all = a_files.clone();
    all.extend(b_files.clone());

    let hashes = hash_image_data_parallel(&all, 64 * 1024, progress);

    let mut map: HashMap<u64, (Vec<PathBuf>, Vec<PathBuf>)> = HashMap::new();
    let mut unreadable = Vec::new();
    for (i, path) in all.iter().enumerate() {
        if !hashes[i].decoded {
            // Only candidate-tree (B) files are surfaced as unreadable; an
            // unreadable file in the destination library is simply ignored.
            if i >= a_len {
                unreadable.push(path.clone());
            }
            continue;
        }
        let entry = map.entry(hashes[i].hash).or_default();
        if i < a_len {
            entry.0.push(path.clone());
        } else {
            entry.1.push(path.clone());
        }
    }

    let mut duplicates = Vec::new();
    let mut a_only = Vec::new();
    let mut b_only = Vec::new();

    let mut groups: Vec<(Vec<PathBuf>, Vec<PathBuf>)> = map.into_values().collect();
    groups.sort_by_key(|(a, b)| {
        a.first()
            .cloned()
            .or_else(|| b.first().cloned())
            .unwrap_or_default()
    });

    for (a_list, b_list) in groups {
        if !a_list.is_empty() && !b_list.is_empty() {
            duplicates.push(DuplicateGroup {
                a: a_list,
                b: b_list,
            });
        } else if !b_list.is_empty() {
            b_only.extend(b_list);
        } else {
            a_only.extend(a_list);
        }
    }

    a_only.sort();
    b_only.sort();
    unreadable.sort();

    Ok(Comparison {
        a: a.to_path_buf(),
        b: b.to_path_buf(),
        duplicates,
        a_only,
        b_only,
        unreadable,
    })
}

/// Resolve EXIF capture dates (DateTimeOriginal, then CreateDate, else
/// Unknown) for the given paths, in parallel.
pub fn scan_exif(paths: &[PathBuf]) -> Vec<DatedOriginal> {
    scan_exif_with_progress(paths, &|_, _| {})
}

pub fn scan_exif_with_progress(
    paths: &[PathBuf],
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<DatedOriginal> {
    let total = paths.len();
    let done = AtomicUsize::new(0);

    paths
        .par_iter()
        .map(|path| {
            let creation_date = creation_date(path);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            DatedOriginal {
                path: path.clone(),
                creation_date,
            }
        })
        .collect()
}

fn collect_image_files(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry?;
        if entry.file_type().is_file() && is_supported_image(entry.path()) {
            out.push(entry.into_path());
        }
    }
    out.sort();
    Ok(out)
}
