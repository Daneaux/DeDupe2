//! PNG support: the image data is the concatenated `IDAT` chunk stream —
//! exactly the picture bytes, with text/metadata chunks excluded.

use std::path::Path;

use super::types::{ImageReaderError, ReadLimit};

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

pub fn image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    let data = std::fs::read(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let idat = extract_idat(&data)
        .ok_or_else(|| ImageReaderError::new("could not locate PNG image data (IDAT)"))?;

    Ok(match limit {
        ReadLimit::All => idat,
        ReadLimit::First(n) => idat[..idat.len().min(n)].to_vec(),
    })
}

/// Frame/pixel decode via the `image` crate (shared with the other rasters).
pub fn decode(path: &Path) -> Result<super::types::ImageData, ImageReaderError> {
    super::raster::decode(path)
}

fn extract_idat(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 8 || data[..8] != PNG_SIGNATURE {
        return None;
    }

    let mut out = Vec::new();
    let mut pos = 8usize;
    while pos + 12 <= data.len() {
        let b = data.get(pos..pos + 4)?;
        let len = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
        let typ = data.get(pos + 4..pos + 8)?;
        let body = data.get(pos + 8..)?; // just for bounds
        if body.len() < len {
            return None;
        }

        if typ == b"IDAT" {
            out.extend_from_slice(&data[pos + 8..pos + 8 + len]);
        }
        if typ == b"IEND" {
            break;
        }
        pos = pos + 12 + len; // len + type + data + crc
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
