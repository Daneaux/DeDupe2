mod bytes;
mod hash;
mod heic;
mod isobmff;
mod jpg;
mod raster;
mod png;
mod phash;
mod movie;
mod raw;
mod types;

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

pub use bytes::read_bytes;
pub use phash::{distance, distance_of, lossy_kind, phash, phash_of, Phash, SIMILAR_MAX_DISTANCE};
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
    let primary = detect_file_type(path);
    let primary_kind = primary.as_ref().ok().copied();
    let first = match primary {
        Ok(kind) => dispatch_read_image(path, kind),
        Err(e) => Err(e),
    };
    match first {
        Ok(data) => Ok(data),
        Err(first_err) => match rescue_kind(path, primary_kind) {
            Some(kind) => dispatch_read_image(path, kind).map_err(|_| first_err),
            None => Err(first_err),
        },
    }
}

pub fn read_image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    let primary = detect_file_type(path);
    let primary_kind = primary.as_ref().ok().copied();
    let first = match primary {
        Ok(kind) => dispatch_image_data(path, kind, limit),
        Err(e) => Err(e),
    };
    match first {
        Ok(data) => Ok(data),
        Err(first_err) => match rescue_kind(path, primary_kind) {
            Some(kind) => dispatch_image_data(path, kind, limit).map_err(|_| first_err),
            None => Err(first_err),
        },
    }
}

fn dispatch_read_image(path: &Path, kind: FileType) -> Result<ImageData, ImageReaderError> {
    match kind {
        FileType::Jpg => jpg::decode(path),
        FileType::Png => png::decode(path),
        FileType::Raster => raster::decode(path),
        FileType::Heic => heic::decode(path),
        FileType::Movie => movie::decode(path),
        FileType::Raw => raw::decode(path),
    }
}

fn dispatch_image_data(
    path: &Path,
    kind: FileType,
    limit: ReadLimit,
) -> Result<Vec<u8>, ImageReaderError> {
    match kind {
        FileType::Jpg => jpg::image_data(path, limit),
        FileType::Png => png::image_data(path, limit),
        FileType::Raster => raster::image_data(path, limit),
        FileType::Heic => heic::image_data(path, limit),
        FileType::Movie => movie::image_data(path, limit),
        FileType::Raw => raw::image_data(path, limit),
    }
}

/// The extension's decoder failed — the file may be mislabeled (a PNG named
/// .JPG, a JPEG named .MOV, an M2TS named .MOV ...). Sniff the content and
/// only when it disagrees with the extension try the matching decoder.
fn rescue_kind(path: &Path, primary: Option<FileType>) -> Option<FileType> {
    let sniffed = sniff_file_type(path)?;
    if primary == Some(sniffed) {
        None
    } else {
        Some(sniffed)
    }
}

/// Content-based type detection from the file's first bytes, used only when
/// the extension-driven decoder has already failed.
fn sniff_file_type(path: &Path) -> Option<FileType> {
    use std::io::Read;

    let mut head = [0u8; 32];
    let mut f = std::fs::File::open(path).ok()?;
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(read) => n += read,
            Err(_) => return None,
        }
    }
    let head = &head[..n];

    if head.len() >= 3 && head[0] == 0xFF && head[1] == 0xD8 {
        return Some(FileType::Jpg);
    }
    if head.len() >= 8 && head[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return Some(FileType::Png);
    }
    if movie::looks_like_riff_avi(head) {
        return Some(FileType::Movie);
    }
    if movie::looks_like_transport_stream(head) {
        return Some(FileType::Movie);
    }
    // ISO base media boxes: 4-byte size, then a known box type.
    if head.len() >= 8 {
        let box_type = &head[4..8];
        if matches!(
            box_type,
            b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"styp" | b"moof"
        ) {
            let brand = head.get(8..12);
            let heic_brand = brand
                .map(|b| matches!(b, b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1" | b"avif" | b"avis"))
                .unwrap_or(false);
            return Some(if heic_brand {
                FileType::Heic
            } else {
                FileType::Movie
            });
        }
    }
    if head.len() >= 4 && (&head[..2] == b"II" || &head[..2] == b"MM") && head[2..4] == [0x2A, 0x00]
    {
        return Some(FileType::Raw);
    }
    None
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
        // A tree can change while a long scan runs (a folder renamed or a
        // file deleted mid-walk). Skipping the unreadable entry keeps the
        // rest of the scan useful instead of aborting after minutes of work;
        // genuinely bad roots are rejected up front by the handlers.
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                tracing::debug!("skipping unreadable path during scan: {e}");
                continue;
            }
        };
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

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v", "mts", "m2ts", "avi"];
