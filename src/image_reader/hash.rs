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
/// whole-file hash when the file isn't a recognized image.
pub fn hash_image_data_n(path: &Path, n: usize) -> u64 {
    match hash_image_data(path, ReadLimit::First(n)) {
        Ok(hash) => hash,
        Err(_) => hash_all_bytes(path).unwrap_or(0),
    }
}
