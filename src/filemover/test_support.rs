//! Fixtures shared by the filemover unit tests.

use std::path::{Path, PathBuf};

/// A checked-in sample under `tests/TestImages`.
pub(super) fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/TestImages")
        .join(rel)
}

/// Write a copy of `src` to `dst` whose first 64 KB of image data is
/// unchanged but whose tail differs: the shallow hash collides, the content
/// does not.
pub(super) fn lookalike(src: &Path, dst: &Path) {
    let mut bytes = std::fs::read(src).unwrap();
    let mut at = bytes.len() - 200;
    while bytes[at] == 0xFF || bytes[at - 1] == 0xFF || bytes[at] == 0xFE {
        at -= 1;
    }
    bytes[at] ^= 0x01;
    std::fs::write(dst, bytes).unwrap();
}
