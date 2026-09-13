use std::path::Path;

use super::bytes::read_bytes;
use super::types::{ImageReaderError, ReadLimit};

/// Hash the whole file by streaming it into the hasher in fixed-size chunks.
/// Identical to `seahash::hash(&fs::read(path)?)`, but never buffers the file
/// in memory (matters for multi-GB scans during deep compare/re-home).
pub fn hash_all_bytes(path: &Path) -> Result<u64, ImageReaderError> {
    use std::hash::Hasher;
    use std::io::Read;

    let file = std::fs::File::open(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let mut hasher = seahash::SeaHasher::new();
    let mut reader = std::io::BufReader::new(file);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| ImageReaderError::new(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
    }
    Ok(hasher.finish())
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

/// Full encoded-image-data hash for deep scans: complete scan data for stills
/// (metadata-stripped per format), the full raw decode, and the whole uncapped
/// `mdat` payload for movies — streamed, never buffered whole.
pub fn hash_image_data_all(path: &Path) -> Result<u64, ImageReaderError> {
    match super::detect_file_type(path)? {
        super::FileType::Movie => super::movie::hash_mdat(path),
        _ => hash_image_data(path, ReadLimit::All),
    }
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
