mod bytes;
mod hash;
mod heic;
mod isobmff;
mod jpg;
mod raster;
mod png;
mod movie;
mod raw;
mod types;

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

pub use bytes::read_bytes;
pub use hash::{
    hash_all_bytes, hash_first_n_bytes, hash_image_data, hash_image_data_all,
    hash_image_data_n, hash_image_data_status,
};
pub use types::{ImageData, ImageReaderError, PixelData, ReadLimit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileType {
    Jpg,
    Heic,
    Movie,
    Raw,
    Png,
    Raster,
}

pub fn read_image(path: &Path) -> Result<ImageData, ImageReaderError> {
    match detect_file_type(path)? {
        FileType::Jpg => jpg::decode(path),
        FileType::Png => png::decode(path),
        FileType::Raster => raster::decode(path),
        FileType::Heic => heic::decode(path),
        FileType::Movie => movie::decode(path),
        FileType::Raw => raw::decode(path),
    }
}

pub fn read_image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    match detect_file_type(path)? {
        FileType::Jpg => jpg::image_data(path, limit),
        FileType::Png => png::image_data(path, limit),
        FileType::Raster => raster::image_data(path, limit),
        FileType::Heic => heic::image_data(path, limit),
        FileType::Movie => movie::image_data(path, limit),
        FileType::Raw => raw::image_data(path, limit),
    }
}

fn detect_file_type(path: &Path) -> Result<FileType, ImageReaderError> {
    match extension_of(path) {
        Some(ext) => match ext.as_str() {
            "heic" | "heif" | "avif" => Ok(FileType::Heic),
            e if VIDEO_EXTENSIONS.contains(&e) => Ok(FileType::Movie),
            e if RAW_EXTENSIONS.contains(&e) => Ok(FileType::Raw),
            "png" => Ok(FileType::Png),
            "jpg" | "jpeg" => Ok(FileType::Jpg),
            e if IMAGE_EXTENSIONS.contains(&e) => Ok(FileType::Raster),
            _ => Err(ImageReaderError::new(format!("unsupported file type: .{ext}"))),
        },
        None => Err(ImageReaderError::new("file has no extension")),
    }
}

/// Whether a path has a supported image extension (jpeg/heic/raw/etc.), and so
/// should be considered during duplicate scanning and merging.
/// Recursively list every supported image/video file under `dir`, sorted.
/// The single definition of "what counts as media" for scans and transfers.
pub fn collect_image_files(dir: &Path) -> Result<Vec<PathBuf>, ImageReaderError> {
    let mut out = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry.map_err(|e| ImageReaderError::new(e.to_string()))?;
        if entry.file_type().is_file() && is_supported_image(entry.path()) {
            out.push(entry.into_path());
        }
    }
    out.sort();
    Ok(out)
}

/// Whether a path names a video (has a video extension).
pub fn is_video(path: &Path) -> bool {
    matches!(
        extension_of(path).as_deref(),
        Some(e) if VIDEO_EXTENSIONS.contains(&e)
    )
}

pub fn is_supported_image(path: &Path) -> bool {
    match extension_of(path) {
        Some(ext) => {
            ext == "heic"
                || ext == "heif"
                || RAW_EXTENSIONS.contains(&ext.as_str())
                || IMAGE_EXTENSIONS.contains(&ext.as_str())
                || VIDEO_EXTENSIONS.contains(&ext.as_str())
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

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v"];
