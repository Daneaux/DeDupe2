use std::path::Path;

use super::types::{ImageData, ImageReaderError, PixelData, ReadLimit};

mod cr3;
mod crw;
mod raf;
mod tiff;

fn load_raw(path: &Path) -> Result<rawler::RawImage, ImageReaderError> {
    let loader = rawler::RawLoader::new();
    loader
        .decode_file(path)
        .map_err(|e| ImageReaderError::new(e.to_string()))
}

pub fn decode(path: &Path) -> Result<ImageData, ImageReaderError> {
    let mut raw = load_raw(path)?;
    raw.data.force_integer();

    Ok(ImageData {
        width: raw.width,
        height: raw.height,
        cpp: raw.cpp,
        data: PixelData::U16(raw.pixels_u16().to_vec()),
    })
}

#[derive(Debug, Clone, Copy)]
enum RawFormat {
    Tiff,
    Raf,
    Cr3,
    Crw,
}

pub fn image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    let data = std::fs::read(path).map_err(|e| ImageReaderError::new(e.to_string()))?;

    let raw = match detect(&data) {
        Some(RawFormat::Tiff) => tiff::raw_data(&data),
        Some(RawFormat::Raf) => raf::raw_data(&data),
        Some(RawFormat::Cr3) => cr3::raw_data(&data),
        Some(RawFormat::Crw) => crw::raw_data(&data),
        None => None,
    }
    .ok_or_else(|| ImageReaderError::new("could not locate raw image data"))?;

    Ok(match limit {
        ReadLimit::All => raw,
        ReadLimit::First(n) => raw[..raw.len().min(n)].to_vec(),
    })
}

fn detect(data: &[u8]) -> Option<RawFormat> {
    if data.len() >= 16 && &data[0..16] == b"FUJIFILMCCD-RAW " {
        return Some(RawFormat::Raf);
    }
    if data.len() >= 14 && &data[0..2] == b"II" && &data[6..14] == b"HEAPCCDR" {
        return Some(RawFormat::Crw);
    }
    if data.len() >= 8 && &data[4..8] == b"ftyp" {
        return Some(RawFormat::Cr3);
    }
    if data.len() >= 8 && (&data[0..2] == b"II" || &data[0..2] == b"MM") {
        let magic = &data[2..4];
        if magic == b"\x2A\x00" || magic == b"\x00\x2A" {
            return Some(RawFormat::Tiff);
        }
    }
    None
}
