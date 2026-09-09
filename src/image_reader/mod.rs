mod bytes;
mod hash;
mod heic;
mod isobmff;
mod jpg;
mod raw;
mod types;

use std::path::Path;

pub use bytes::read_bytes;
pub use hash::{
    hash_all_bytes, hash_first_n_bytes, hash_image_data, hash_image_data_n,
    hash_image_data_status,
};
pub use types::{ImageData, ImageReaderError, PixelData, ReadLimit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileType {
    Jpg,
    Heic,
    Raw,
}

pub fn read_image(path: &Path) -> Result<ImageData, ImageReaderError> {
    match detect_file_type(path)? {
        FileType::Jpg => jpg::decode(path),
        FileType::Heic => heic::decode(path),
        FileType::Raw => raw::decode(path),
    }
}

pub fn read_image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    match detect_file_type(path)? {
        FileType::Jpg => jpg::image_data(path, limit),
        FileType::Heic => heic::image_data(path, limit),
        FileType::Raw => raw::image_data(path, limit),
    }
}

fn detect_file_type(path: &Path) -> Result<FileType, ImageReaderError> {
    match extension_of(path) {
        Some(ext) => match ext.as_str() {
            "heic" | "heif" => Ok(FileType::Heic),
            e if RAW_EXTENSIONS.contains(&e) => Ok(FileType::Raw),
            e if IMAGE_EXTENSIONS.contains(&e) => Ok(FileType::Jpg),
            _ => Err(ImageReaderError::new(format!("unsupported file type: .{ext}"))),
        },
        None => Err(ImageReaderError::new("file has no extension")),
    }
}

/// Whether a path has a supported image extension (jpeg/heic/raw/etc.), and so
/// should be considered during duplicate scanning and merging.
pub fn is_supported_image(path: &Path) -> bool {
    match extension_of(path) {
        Some(ext) => {
            ext == "heic"
                || ext == "heif"
                || RAW_EXTENSIONS.contains(&ext.as_str())
                || IMAGE_EXTENSIONS.contains(&ext.as_str())
        }
        None => false,
    }
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

const RAW_EXTENSIONS: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "rw2", "orf", "pef", "raf",
    "dng", "srw", "x3f", "iiq", "erf", "3fr", "kdc", "dcr", "dcs", "mef", "mos", "mrw",
    "raw", "rwl", "fff", "bay", "ari", "tif", "tiff",
];

const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "bmp", "ico", "pnm", "pbm",
    "pgm", "ppm", "pam", "qoi", "avif", "hdr", "exr", "ff",
];
