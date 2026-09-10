//! MP4 / MOV (ISO base media file format) support.
//!
//! The movie content lives in top-level `mdat` boxes; everything else
//! (`ftyp`, `moov`, metadata atoms) is container structure. Hashing reads the
//! head of the concatenated `mdat` payload so that metadata-only changes do
//! not change the hash.

use std::path::Path;

use super::types::{ImageData, ImageReaderError, ReadLimit};

/// Upper bound on how much `mdat` is buffered. Content hashing only needs the
/// head of the stream, and this keeps a multi-GB movie from being read into
/// memory when `ReadLimit::All` is requested.
const MAX_MOVIE_DATA: usize = 1024 * 1024;

/// Concatenated `mdat` payload, in file order, capped at `cap` bytes.
fn extract_mdat(data: &[u8], cap: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0usize;

    while pos + 8 <= data.len() && out.len() < cap {
        let size32 = u32_at(data, pos)? as usize;
        let (header, size) = match size32 {
            0 => (8, data.len() - pos), // box extends to EOF
            1 => {
                let v = u64_at(data, pos + 8)?;
                if v > usize::MAX as u64 {
                    return None;
                }
                (16, v as usize)
            }
            n => (8, n),
        };
        if size < header || data.len() < pos + size {
            return None; // malformed box
        }
        if &data[pos + 4..pos + 8] == b"mdat" {
            out.extend_from_slice(&data[pos + header..pos + size]);
        }
        pos += size;
    }

    if out.is_empty() {
        None
    } else {
        Some(out[..out.len().min(cap)].to_vec())
    }
}

pub fn image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    let data = std::fs::read(path).map_err(|e| ImageReaderError::new(e.to_string()))?;

    let cap = match limit {
        ReadLimit::All => MAX_MOVIE_DATA,
        ReadLimit::First(n) => n,
    };
    let mdat = extract_mdat(&data, cap)
        .ok_or_else(|| ImageReaderError::new("could not locate movie data (mdat)"))?;

    Ok(match limit {
        ReadLimit::All => mdat,
        ReadLimit::First(n) => mdat[..mdat.len().min(n)].to_vec(),
    })
}

/// Frame decoding is not supported for movies; the hash path uses `image_data`.
pub fn decode(path: &Path) -> Result<ImageData, ImageReaderError> {
    Err(ImageReaderError::new(format!(
        "movie frame decoding is not supported: {}",
        path.display()
    )))
}

fn u32_at(data: &[u8], i: usize) -> Option<u32> {
    let b = data.get(i..i + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(data: &[u8], i: usize) -> Option<u64> {
    let b = data.get(i..i + 8)?;
    Some(u64::from_be_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}
