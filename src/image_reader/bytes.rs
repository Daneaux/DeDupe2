use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::types::{ImageReaderError, ReadLimit};

pub fn read_bytes(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    let mut file = File::open(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let mut buf = Vec::new();

    match limit {
        ReadLimit::All => {
            file.read_to_end(&mut buf)
                .map_err(|e| ImageReaderError::new(e.to_string()))?;
        }
        ReadLimit::First(n) => {
            file.take(n as u64)
                .read_to_end(&mut buf)
                .map_err(|e| ImageReaderError::new(e.to_string()))?;
        }
    }

    Ok(buf)
}
