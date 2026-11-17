//! Deciding which files are duplicates *before* anything is purged.
//!
//! A shallow hash (first 64 KB of encoded image data) only nominates
//! candidates. A group is formed only from files whose full content is proven
//! identical by [`content_match`]; files that cannot be read or decoded are
//! never grouped with anything they are not byte-identical to.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use super::naming::name_len;
use super::Progress;
use crate::exif::embedded_capture_date;
use crate::Scanner::compare::{content_match, ContentMatch};
use crate::Scanner::scan::{hash_image_data_parallel, FileHash};

/// Bytes of encoded image data the shallow hash covers.
const SHALLOW_PREFIX: usize = 64 * 1024;

/// Partition `paths` (by index) into groups of files with identical content.
/// Every file appears in exactly one group; singletons are unique files.
/// Within a group the keeper comes first (see [`keeper_rank`]); groups are
/// ordered by their keeper's path.
pub(super) fn confirmed_groups(paths: &[PathBuf], progress: Progress) -> Vec<Vec<usize>> {
    let hashes = hash_image_data_parallel(paths, SHALLOW_PREFIX, progress);
    let mut groups = group_by_content(
        paths,
        &hashes,
        &|a, b| content_match(a, b) == ContentMatch::Same,
        progress,
    );
    groups
        .par_iter_mut()
        .filter(|g| g.len() > 1)
        .for_each(|group| {
            group.sort_by_cached_key(|&i| keeper_rank(&paths[i]));
        });
    groups.sort_by(|a, b| paths[a[0]].cmp(&paths[b[0]]));
    groups
}

/// Sort key for choosing which of several identical-content copies to keep
/// (smallest first). Copies can differ only in metadata, so prefer the one
/// that carries the most: an embedded capture date first, then the larger
/// file (identical image data plus more bytes means more metadata), then
/// the shorter name, then the path for determinism.
fn keeper_rank(path: &Path) -> (bool, Reverse<u64>, usize, PathBuf) {
    let undated = embedded_capture_date(path).is_none();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    (undated, Reverse(size), name_len(path), path.to_path_buf())
}

/// [`confirmed_groups`] with the shallow hashes and the content test given.
fn group_by_content(
    paths: &[PathBuf],
    hashes: &[FileHash],
    same: &(dyn Fn(&Path, &Path) -> bool + Sync),
    progress: Progress,
) -> Vec<Vec<usize>> {
    // Decoded and fallback (raw-bytes) hashes live in different spaces, and
    // every unreadable file shares the fallback value, so they never mix.
    let mut buckets: HashMap<(u64, bool), Vec<usize>> = HashMap::new();
    for (i, fh) in hashes.iter().enumerate() {
        buckets.entry((fh.hash, fh.decoded)).or_default().push(i);
    }

    let keeper_order = |&a: &usize, &b: &usize| {
        name_len(&paths[a])
            .cmp(&name_len(&paths[b]))
            .then_with(|| paths[a].cmp(&paths[b]))
    };
    let buckets: Vec<Vec<usize>> = buckets
        .into_values()
        .map(|mut bucket| {
            bucket.sort_by(keeper_order);
            bucket
        })
        .collect();

    let total: usize = buckets.iter().map(|b| b.len().saturating_sub(1)).sum();
    let done = AtomicUsize::new(0);

    let mut groups: Vec<Vec<usize>> = buckets
        .into_par_iter()
        .flat_map_iter(|bucket| {
            if bucket.len() == 1 {
                return vec![bucket];
            }
            let mut clusters: Vec<Vec<usize>> = Vec::new();
            for (n, i) in bucket.into_iter().enumerate() {
                match clusters.iter_mut().find(|c| same(&paths[c[0]], &paths[i])) {
                    Some(cluster) => cluster.push(i),
                    None => clusters.push(vec![i]),
                }
                if n > 0 {
                    progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
                }
            }
            clusters
        })
        .collect();

    groups.sort_by(|a, b| paths[a[0]].cmp(&paths[b[0]]));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filemover::test_support::{fixture, lookalike};

    fn no_progress(_: usize, _: usize) {}

    fn sorted(mut groups: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
        for g in &mut groups {
            g.sort();
        }
        groups.sort();
        groups
    }

    /// `src` with its EXIF (APP1) segment removed: identical image data, no
    /// embedded date, fewer bytes.
    fn strip_exif(src: &Path, dst: &Path) {
        let bytes = std::fs::read(src).unwrap();
        let at = bytes
            .windows(2)
            .position(|w| w == [0xFF, 0xE1])
            .expect("fixture has an APP1 segment");
        let len = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        let mut out = bytes[..at].to_vec();
        out.extend_from_slice(&bytes[at + 2 + len..]);
        std::fs::write(dst, out).unwrap();
    }

    #[test]
    fn the_copy_with_its_capture_date_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let dated = dir.path().join("IMG_0001 original with exif.jpg");
        let stripped = dir.path().join("a.jpg");
        std::fs::copy(fixture("jpg-exif-mod/image1.JPG"), &dated).unwrap();
        strip_exif(&dated, &stripped);
        assert!(embedded_capture_date(&stripped).is_none(), "precondition");

        let paths = vec![stripped, dated];
        let groups = confirmed_groups(&paths, &no_progress);

        assert_eq!(
            groups,
            vec![vec![1, 0]],
            "dated copy kept despite the longer name"
        );
    }

    #[test]
    fn among_dated_copies_the_bigger_one_is_kept() {
        // Same image data and date; the bigger file carries more metadata.
        let paths = vec![
            fixture("HEIC-exif-mod/heic1.HEIC"),
            fixture("HEIC-exif-mod/heic1-copy.heic"),
        ];
        assert_eq!(
            confirmed_groups(&paths, &no_progress),
            vec![vec![1, 0]],
            "1489600 bytes beats 1489060"
        );
    }

    #[test]
    fn shallow_collisions_without_matching_content_stay_apart() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("a.jpg");
        let copy = dir.path().join("b.jpg");
        let lookalike_path = dir.path().join("c.jpg");
        std::fs::copy(fixture("jpg-exif-mod/image1.JPG"), &original).unwrap();
        std::fs::copy(&original, &copy).unwrap();
        lookalike(&original, &lookalike_path);

        let paths = vec![original, copy, lookalike_path];
        let hashes = hash_image_data_parallel(&paths, SHALLOW_PREFIX, &no_progress);
        assert!(hashes.iter().all(|h| h.decoded));
        assert!(
            hashes.iter().all(|h| h.hash == hashes[0].hash),
            "precondition: the three files share a shallow hash"
        );

        let groups = confirmed_groups(&paths, &no_progress);
        assert_eq!(sorted(groups), vec![vec![0, 1], vec![2]]);
    }

    #[test]
    fn metadata_only_differences_still_group() {
        // Same image data, different EXIF: a true duplicate for this app.
        let paths = vec![
            fixture("jpg-exif-mod/image1.JPG"),
            fixture("jpg-exif-mod/image1-exif.JPG"),
        ];
        assert_eq!(confirmed_groups(&paths, &no_progress).len(), 1);
    }

    #[test]
    fn unreadable_files_are_never_grouped() {
        // Both fell back to the same raw hash (0 = read failed) and neither
        // can be read now.
        let dir = tempfile::tempdir().unwrap();
        let paths = vec![dir.path().join("gone1.jpg"), dir.path().join("gone2.jpg")];
        let hashes = [
            FileHash {
                hash: 0,
                decoded: false,
            },
            FileHash {
                hash: 0,
                decoded: false,
            },
        ];
        let same = |a: &Path, b: &Path| content_match(a, b) == ContentMatch::Same;

        let groups = group_by_content(&paths, &hashes, &same, &no_progress);
        assert_eq!(sorted(groups), vec![vec![0], vec![1]]);
    }

    #[test]
    fn byte_identical_undecodable_files_still_group() {
        let dir = tempfile::tempdir().unwrap();
        let paths = vec![dir.path().join("x.jpg"), dir.path().join("y.jpg")];
        for p in &paths {
            std::fs::write(p, b"not really a jpeg").unwrap();
        }
        let hashes = hash_image_data_parallel(&paths, SHALLOW_PREFIX, &no_progress);
        assert!(hashes.iter().all(|h| !h.decoded));

        assert_eq!(confirmed_groups(&paths, &no_progress), vec![vec![0, 1]]);
    }

    #[test]
    fn decoded_and_fallback_hashes_never_meet() {
        let paths = vec![PathBuf::from("/d.jpg"), PathBuf::from("/f.jpg")];
        let hashes = [
            FileHash {
                hash: 7,
                decoded: true,
            },
            FileHash {
                hash: 7,
                decoded: false,
            },
        ];
        let groups = group_by_content(&paths, &hashes, &|_, _| true, &no_progress);
        assert_eq!(groups.len(), 2, "never even compared");
    }

    #[test]
    fn a_bucket_splits_into_content_clusters_with_the_keeper_first() {
        // One shallow bucket holding two distinct images, two copies each.
        let paths: Vec<PathBuf> = ["/x/long_a.jpg", "/x/a.jpg", "/x/bb.jpg", "/x/b.jpg"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let hashes = vec![
            FileHash {
                hash: 1,
                decoded: true
            };
            4
        ];
        let image = |p: &Path| p.to_string_lossy().contains('a');
        let same = |a: &Path, b: &Path| image(a) == image(b);

        let groups = group_by_content(&paths, &hashes, &same, &no_progress);
        assert_eq!(groups, vec![vec![1, 0], vec![3, 2]], "shortest name keeps");
    }
}
