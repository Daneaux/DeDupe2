//! Perceptual hashing (pHash) for near-duplicate detection: finding copies
//! of the same image that were scaled down or recompressed, where pixel
//! equality fails but the picture is visibly the same.
//!
//! The hash itself comes from the `image_hasher` crate — median hashing over
//! a DCT-preprocessed image (the classic pHash recipe), 8x8 bits. Scaled and
//! recompressed copies usually land within a small Hamming distance of each
//! other, while unrelated photos sit around half the bits apart.
//!
//! Similarity is only meaningful between *like kinds* of lossy files (jpg
//! with jpg, heic with heic): lossless formats (png) and raw files have no
//! generational loss to compare against, and cross-format pairs are already
//! handled by decoded-pixel equality.

use std::path::Path;

use image_hasher::{FilterType, HashAlg, HasherConfig};

use super::types::{ImageData, PixelData};

/// Hamming distance at or below which two lossy same-kind images count as
/// near-duplicates (scaled/recompressed copies of the same photo).
pub const SIMILAR_MAX_DISTANCE: u32 = 10;

/// Perceptual hash plus the source dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Phash {
    pub hash: u64,
    pub width: u32,
    pub height: u32,
}

impl Phash {
    pub fn pixels(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// Which family of lossy format a file belongs to. Only files of the same
/// family are phash-compared.
pub fn lossy_kind(path: &Path) -> Option<u8> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => Some(0),
        Some("heic") | Some("heif") | Some("avif") => Some(1),
        _ => None,
    }
}

fn hasher() -> image_hasher::Hasher {
    // Median + DCT preprocessing is pHash per the crate's documentation.
    HasherConfig::new()
        .hash_size(8, 8)
        .hash_alg(HashAlg::Median)
        .preproc_dct()
        .resize_filter(FilterType::Lanczos3)
        .to_hasher()
}

/// pHash of a decoded image; `None` when the image has no pixels.
pub fn phash_of(image: &ImageData) -> Option<Phash> {
    let dynamic = to_dynamic(image)?;
    let hash = hasher().hash_image(&dynamic);
    let bytes = hash.as_bytes();
    if bytes.len() < 8 {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[..8]);
    Some(Phash {
        hash: u64::from_le_bytes(buf),
        width: image.width as u32,
        height: image.height as u32,
    })
}

/// pHash of a file: decode (panics contained) then hash the pixels.
pub fn phash(path: &Path) -> Option<Phash> {
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::read_image(path)
    }));
    match decoded {
        Ok(Ok(image)) => phash_of(&image),
        _ => None,
    }
}

/// Mean chroma (max-min per RGB pixel, 0-255) — a cheap colorfulness score.
/// pHash works on luminance structure, so a black-and-white conversion of a
/// color photo hashes identically; comparing colorfulness separates them.
/// `None` for images without RGB channels.
pub fn mean_chroma(image: &ImageData) -> Option<u8> {
    if image.cpp < 3 || image.width == 0 || image.height == 0 {
        return None;
    }
    let mut sum: u64 = 0;
    let mut count: u64 = 0;
    for p in 0..(image.width * image.height) {
        let base = p * image.cpp;
        let (r, g, b) = match &image.data {
            PixelData::U8(v) => (v[base] as i32, v[base + 1] as i32, v[base + 2] as i32),
            PixelData::U16(v) => (
                (v[base] >> 8) as i32,
                (v[base + 1] >> 8) as i32,
                (v[base + 2] >> 8) as i32,
            ),
        };
        sum += (r.max(g).max(b) - r.min(g).min(b)) as u64;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    Some((sum / count) as u8)
}

/// Number of differing bits.
pub fn distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Hamming distance between two perceptual hashes.
pub fn distance_of(a: &Phash, b: &Phash) -> u32 {
    distance(a.hash, b.hash)
}

/// Bridge our decoded pixel container into the `image` crate type the hasher
/// works on. 16-bit samples are scaled to 8-bit; unusual channel counts fall
/// back to their first channel as luminance.
fn to_dynamic(image: &ImageData) -> Option<image::DynamicImage> {
    let (width, height) = (image.width as u32, image.height as u32);
    if width == 0 || height == 0 || image.cpp == 0 {
        return None;
    }
    let pixels = (image.width * image.height) as usize;
    let gray: Vec<u8> = match &image.data {
        PixelData::U8(bytes) => match image.cpp {
            3 => {
                let rgb = image::RgbImage::from_raw(width, height, bytes[..pixels * 3].to_vec())?;
                return Some(image::DynamicImage::ImageRgb8(rgb));
            }
            4 => {
                let rgba = image::RgbaImage::from_raw(width, height, bytes[..pixels * 4].to_vec())?;
                return Some(image::DynamicImage::ImageRgba8(rgba));
            }
            _ => bytes.chunks(image.cpp).map(|p| p[0]).collect(),
        },
        PixelData::U16(words) => match image.cpp {
            3 => {
                let mut rgb = Vec::with_capacity(pixels * 3);
                for pixel in words[..pixels * 3].chunks(3) {
                    rgb.push((pixel[0] >> 8) as u8);
                    rgb.push((pixel[1] >> 8) as u8);
                    rgb.push((pixel[2] >> 8) as u8);
                }
                let rgb = image::RgbImage::from_raw(width, height, rgb)?;
                return Some(image::DynamicImage::ImageRgb8(rgb));
            }
            4 => {
                let mut rgba = Vec::with_capacity(pixels * 4);
                for pixel in words[..pixels * 4].chunks(4) {
                    rgba.push((pixel[0] >> 8) as u8);
                    rgba.push((pixel[1] >> 8) as u8);
                    rgba.push((pixel[2] >> 8) as u8);
                    rgba.push((pixel[3] >> 8) as u8);
                }
                let rgba = image::RgbaImage::from_raw(width, height, rgba)?;
                return Some(image::DynamicImage::ImageRgba8(rgba));
            }
            _ => words
                .chunks(image.cpp)
                .take(pixels)
                .map(|p| (p[0] >> 8) as u8)
                .collect(),
        },
    };
    let luma = image::GrayImage::from_raw(width, height, gray)?;
    Some(image::DynamicImage::ImageLuma8(luma))
}
