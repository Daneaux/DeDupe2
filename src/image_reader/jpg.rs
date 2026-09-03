use std::path::Path;

use super::types::{ImageData, ImageReaderError, PixelData, ReadLimit};

pub fn decode(path: &Path) -> Result<ImageData, ImageReaderError> {
    let img = image::open(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    Ok(ImageData {
        width: width as usize,
        height: height as usize,
        cpp: 4,
        data: PixelData::U8(rgba.into_raw()),
    })
}

pub fn image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    let data = std::fs::read(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let stripped = strip_metadata(&data)?;

    Ok(match limit {
        ReadLimit::All => stripped,
        ReadLimit::First(n) => stripped[..stripped.len().min(n)].to_vec(),
    })
}

fn strip_metadata(data: &[u8]) -> Result<Vec<u8>, ImageReaderError> {
    if data.len() < 2 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(ImageReaderError::new("not a JPEG file"));
    }

    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..2]);

    let mut i = 2;
    while i < data.len() {
        if data[i] != 0xFF {
            out.push(data[i]);
            i += 1;
            continue;
        }

        if i + 1 >= data.len() {
            break;
        }
        let marker = data[i + 1];

        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) || marker == 0xD9 {
            out.extend_from_slice(&data[i..i + 2]);
            if marker == 0xD9 {
                break;
            }
            i += 2;
            continue;
        }

        if i + 4 > data.len() {
            break;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if len < 2 || i + 2 + len > data.len() {
            return Err(ImageReaderError::new("malformed JPEG segment"));
        }

        if marker != 0xE1 {
            out.extend_from_slice(&data[i..i + 2 + len]);
        }

        if marker == 0xDA {
            out.extend_from_slice(&data[i + 2 + len..]);
            break;
        }

        i += 2 + len;
    }

    Ok(out)
}
