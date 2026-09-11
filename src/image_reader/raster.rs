//! Generic raster formats (webp, gif, bmp, …) handled through the `image`
//! crate: the hashed bytes are the decoded RGBA pixels, so container metadata
//! never influences the hash.

use std::path::Path;

use super::types::{ImageData, ImageReaderError, PixelData, ReadLimit};

pub fn image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    let pixels = decoded_rgba(path)?;
    Ok(match limit {
        ReadLimit::All => pixels.clone(),
        ReadLimit::First(n) => pixels[..pixels.len().min(n)].to_vec(),
    })
}

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

fn decoded_rgba(path: &Path) -> Result<Vec<u8>, ImageReaderError> {
    let img = image::open(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    Ok(img.to_rgba8().into_raw())
}
