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
