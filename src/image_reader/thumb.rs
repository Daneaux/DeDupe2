//! Small JPEG previews for the UI (similar-image tables). Uses the same
//! decoders as everything else, so formats the `image` crate cannot open
//! (HEIC, camera raw) still get a preview.

use std::path::Path;

/// Longest output edge in pixels. Small on purpose: previews are decorative.
const MAX_EDGE: u32 = 256;

/// Decode a file and re-encode it as a small JPEG. `None` when the file
/// cannot be decoded (or the decoder panics).
pub fn thumbnail_jpeg(path: &Path) -> Option<Vec<u8>> {
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::read_image(path)
    }));
    let image = match decoded {
        Ok(Ok(image)) => image,
        _ => return None,
    };
    let dynamic = super::phash::to_dynamic(&image)?;
    let small = dynamic.thumbnail(MAX_EDGE, MAX_EDGE);

    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 78);
    encoder.encode_image(&small).ok()?;
    Some(out)
}
