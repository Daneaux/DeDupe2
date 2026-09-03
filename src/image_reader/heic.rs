use std::path::Path;

use super::isobmff::extract_primary_image_data;
use super::types::{ImageData, ImageReaderError, PixelData, ReadLimit};

pub fn decode(path: &Path) -> Result<ImageData, ImageReaderError> {
    let data = std::fs::read(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let config = heic::DecoderConfig::new();
    let output = config
        .decode(&data, heic::PixelLayout::Rgba8)
        .map_err(|e| ImageReaderError::new(e.to_string()))?;

    Ok(ImageData {
        width: output.width as usize,
        height: output.height as usize,
        cpp: 4,
        data: PixelData::U8(output.data),
    })
}

pub fn image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    let data = std::fs::read(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let image_data = extract_primary_image_data(&data)
        .ok_or_else(|| ImageReaderError::new("could not locate primary image data"))?;

    Ok(match limit {
        ReadLimit::All => image_data,
        ReadLimit::First(n) => image_data[..image_data.len().min(n)].to_vec(),
    })
}
