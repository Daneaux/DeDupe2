use std::path::Path;

use super::bytes::read_bytes;
use super::types::{ImageReaderError, ReadLimit};

pub fn hash_all_bytes(path: &Path) -> Result<u64, ImageReaderError> {
    let bytes = read_bytes(path, ReadLimit::All)?;
    Ok(seahash::hash(&bytes))
}

pub fn hash_first_n_bytes(path: &Path, n: usize) -> Result<u64, ImageReaderError> {
    let bytes = read_bytes(path, ReadLimit::First(n))?;
    Ok(seahash::hash(&bytes))
}

pub fn hash_image_data(path: &Path, limit: ReadLimit) -> Result<u64, ImageReaderError> {
    let data = super::read_image_data(path, limit)?;
    Ok(seahash::hash(&data))
}

/// Hash the first `n` bytes of a file's encoded image data, falling back to the
/// whole-file hash when the file isn't a recognized image. Decoders may panic
/// on malformed files, so the decode is unwound per file and treated like any
/// other decode failure.
pub fn hash_image_data_n(path: &Path, n: usize) -> u64 {
    hash_image_data_status(path, n).0
}

/// Hash plus whether the decoder actually produced the hash (`true`), or the
/// value came from the raw-bytes fallback because the file could not be
/// decoded (`false` — e.g. malformed or decoder panicked).
pub fn hash_image_data_status(path: &Path, n: usize) -> (u64, bool) {
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        hash_image_data(path, ReadLimit::First(n))
    }));
    match decoded {
        Ok(Ok(hash)) => (hash, true),
        _ => (hash_all_bytes(path).unwrap_or(0), false),
    }
}
