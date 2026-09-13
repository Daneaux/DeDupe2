//! MP4 / MOV (ISO base media file format) support.
//!
//! The movie content lives in top-level `mdat` boxes; everything else
//! (`ftyp`, `moov`, metadata atoms) is container structure. Hashing reads the
//! concatenated `mdat` payload so that metadata-only changes do not change
//! the hash. `hash_mdat` streams the complete (uncapped) payload for deep
//! scans, while `image_data` caps the fast-scan header hash at 1 MB.

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

/// Stream the concatenated top-level `mdat` payload (in file order) into a
/// hash without ever buffering it whole. Used by deep scans so that movies
/// are compared on their full content, not a 1 MB head.
pub fn hash_mdat(path: &Path) -> Result<u64, ImageReaderError> {
    use std::hash::Hasher;
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let file_len = f
        .metadata()
        .map_err(|e| ImageReaderError::new(e.to_string()))?
        .len();

    let mut hasher = seahash::SeaHasher::new();
    let mut buf = [0u8; 64 * 1024];
    let mut payload_total: u64 = 0;
    let mut pos: u64 = 0;

    while pos + 8 <= file_len {
        let mut hdr = [0u8; 16];
        f.seek(SeekFrom::Start(pos))
            .map_err(|e| ImageReaderError::new(e.to_string()))?;
        f.read_exact(&mut hdr[..8])
            .map_err(|e| ImageReaderError::new(e.to_string()))?;
        let size32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let (header, size) = match size32 {
            0 => (8u64, file_len - pos), // box extends to EOF
            1 => {
                f.read_exact(&mut hdr[8..16])
                    .map_err(|e| ImageReaderError::new(e.to_string()))?;
                (16, u64::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11], hdr[12], hdr[13], hdr[14], hdr[15]]))
            }
            n => (8, n as u64),
        };
        if size < header || pos + size > file_len {
            return Err(ImageReaderError::new(format!(
                "malformed box at offset {pos}"
            )));
        }

        if &hdr[4..8] == b"mdat" {
            let payload = size - header;
            f.seek(SeekFrom::Start(pos + header))
                .map_err(|e| ImageReaderError::new(e.to_string()))?;
            let mut remaining = payload;
            while remaining > 0 {
                let want = (remaining as usize).min(buf.len());
                let n = f
                    .read(&mut buf[..want])
                    .map_err(|e| ImageReaderError::new(e.to_string()))?;
                if n == 0 {
                    return Err(ImageReaderError::new(format!(
                        "unexpected EOF inside mdat at offset {}",
                        pos + header + payload - remaining
                    )));
                }
                hasher.write(&buf[..n]);
                remaining -= n as u64;
            }
            payload_total += payload;
        }

        pos += size;
    }

    if payload_total == 0 {
        Err(ImageReaderError::new(
            "could not locate movie data (mdat)",
        ))
    } else {
        Ok(hasher.finish())
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
