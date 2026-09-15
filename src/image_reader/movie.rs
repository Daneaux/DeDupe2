//! MP4 / MOV (ISO base media file format), MTS / M2TS (MPEG transport
//! stream, AVCHD camcorders) and AVI (RIFF) support.
//!
//! For MP4/MOV the movie content lives in top-level `mdat` boxes; everything
//! else (`ftyp`, `moov`, metadata atoms) is container structure. For M2TS the
//! content lives in the packet payloads: every 192-byte packet carries a
//! 4-byte arrival timestamp, then a constant 0x47 sync byte, then 187 bytes
//! of transport packet — the timestamp and sync are container bookkeeping,
//! so only the 187 bytes are hashed. For AVI the content lives in the
//! `LIST movi` payload; everything else (`hdrl`, `INFO`/`IDIT` metadata,
//! `idx1` index) is container structure.
//! `hash_mdat` streams the complete (uncapped) content for deep scans, while
//! `image_data` caps the fast-scan header hash at 1 MB.

use std::path::Path;

use super::types::{ImageData, ImageReaderError, ReadLimit};

/// Upper bound on how much `mdat` is buffered. Content hashing only needs the
/// head of the stream, and this keeps a multi-GB movie from being read into
/// memory when `ReadLimit::All` is requested.
const MAX_MOVIE_DATA: usize = 1024 * 1024;

/// Concatenated `mdat` payload, in file order, capped at `cap` bytes.
fn extract_mdat(data: &[u8], cap: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0usize;

    while pos + 8 <= data.len() && out.len() < cap {
        let size32 = u32_at(data, pos)? as usize;
        let (header, size) = match size32 {
            0 => (8, data.len() - pos), // box extends to EOF
            1 => {
                let v = u64_at(data, pos + 8)?;
                if v > usize::MAX as u64 {
                    return None;
                }
                (16, v as usize)
            }
            n => (8, n),
        };
        if size < header || data.len() < pos + size {
            return None; // malformed box
        }
        if &data[pos + 4..pos + 8] == b"mdat" {
            out.extend_from_slice(&data[pos + header..pos + size]);
        }
        pos += size;
    }

    if out.is_empty() {
        None
    } else {
        Some(out[..out.len().min(cap)].to_vec())
    }
}

/// Stream the concatenated top-level `mdat` payload (in file order) into a
/// hash without ever buffering it whole. Used by deep scans so that movies
/// are compared on their full content, not a 1 MB head.
pub fn hash_mdat(path: &Path) -> Result<u64, ImageReaderError> {
    match container_kind(path) {
        Some(MovieContainer::RiffAvi) => return hash_riff_movi(path),
        Some(MovieContainer::TransportStream) => return hash_ts_payload(path),
        _ => {}
    }
    use std::hash::Hasher;
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let file_len = f
        .metadata()
        .map_err(|e| ImageReaderError::new(e.to_string()))?
        .len();

    let mut hasher = seahash::SeaHasher::new();
    let mut buf = [0u8; 64 * 1024];
    let mut payload_total: u64 = 0;
    let mut pos: u64 = 0;

    while pos + 8 <= file_len {
        let mut hdr = [0u8; 16];
        f.seek(SeekFrom::Start(pos))
            .map_err(|e| ImageReaderError::new(e.to_string()))?;
        f.read_exact(&mut hdr[..8])
            .map_err(|e| ImageReaderError::new(e.to_string()))?;
        let size32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let (header, size) = match size32 {
            0 => (8u64, file_len - pos), // box extends to EOF
            1 => {
                f.read_exact(&mut hdr[8..16])
                    .map_err(|e| ImageReaderError::new(e.to_string()))?;
                (16, u64::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11], hdr[12], hdr[13], hdr[14], hdr[15]]))
            }
            n => (8, n as u64),
        };
        if size < header || pos + size > file_len {
            return Err(ImageReaderError::new(format!(
                "malformed box at offset {pos}"
            )));
        }

        if &hdr[4..8] == b"mdat" {
            let payload = size - header;
            f.seek(SeekFrom::Start(pos + header))
                .map_err(|e| ImageReaderError::new(e.to_string()))?;
            let mut remaining = payload;
            while remaining > 0 {
                let want = (remaining as usize).min(buf.len());
                let n = f
                    .read(&mut buf[..want])
                    .map_err(|e| ImageReaderError::new(e.to_string()))?;
                if n == 0 {
                    return Err(ImageReaderError::new(format!(
                        "unexpected EOF inside mdat at offset {}",
                        pos + header + payload - remaining
                    )));
                }
                hasher.write(&buf[..n]);
                remaining -= n as u64;
            }
            payload_total += payload;
        }

        pos += size;
    }

    if payload_total == 0 {
        Err(ImageReaderError::new(
            "could not locate movie data (mdat)",
        ))
    } else {
        Ok(hasher.finish())
    }
}

/// MPEG transport stream detection from the file's first bytes, so MTS/M2TS
/// content is handled by content rather than by extension alone: M2TS places
/// a 0x47 sync byte after each 4-byte arrival timestamp; raw 188-byte TS has
/// 0x47 at the start of every packet.
pub(crate) fn looks_like_transport_stream(head: &[u8]) -> bool {
    if head.len() >= M2TS_PACKET && head[4] == 0x47 {
        return true;
    }
    head.len() >= 189 && head[0] == 0x47 && head.get(188) == Some(&0x47)
}

/// AVI detection from the file's first bytes (`RIFF....AVI `).
pub(crate) fn looks_like_riff_avi(head: &[u8]) -> bool {
    head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"AVI "
}

const M2TS_PACKET: usize = 192;

/// Which container a movie file uses, decided by content so mislabeled
/// extensions still decode through the right path.
enum MovieContainer {
    IsoBox,
    TransportStream,
    RiffAvi,
}

fn container_kind(path: &Path) -> Option<MovieContainer> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; M2TS_PACKET];
    let n = read_up_to(&mut f, &mut head);
    head.truncate(n);
    if looks_like_riff_avi(&head) {
        return Some(MovieContainer::RiffAvi);
    }
    if looks_like_transport_stream(&head) {
        return Some(MovieContainer::TransportStream);
    }
    Some(MovieContainer::IsoBox)
}

pub fn image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    match container_kind(path) {
        Some(MovieContainer::RiffAvi) => return riff_image_data(path, limit),
        Some(MovieContainer::TransportStream) => return ts_image_data(path, limit),
        _ => {}
    }
    let data = std::fs::read(path).map_err(|e| ImageReaderError::new(e.to_string()))?;

    let cap = match limit {
        ReadLimit::All => MAX_MOVIE_DATA,
        ReadLimit::First(n) => n,
    };
    let mdat = extract_mdat(&data, cap)
        .ok_or_else(|| ImageReaderError::new("could not locate movie data (mdat)"))?;

    Ok(match limit {
        ReadLimit::All => mdat,
        ReadLimit::First(n) => mdat[..mdat.len().min(n)].to_vec(),
    })
}

/// Frame decoding is not supported for movies; the hash path uses `image_data`.
pub fn decode(path: &Path) -> Result<ImageData, ImageReaderError> {
    Err(ImageReaderError::new(format!(
        "movie frame decoding is not supported: {}",
        path.display()
    )))
}

/// Bounded `movi` payload of an AVI for the fast (64kb head) hash: content
/// bytes only, no `hdrl`/`INFO`/`IDIT` metadata and no `idx1` index.
fn riff_image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    let cap = match limit {
        ReadLimit::All => MAX_MOVIE_DATA,
        ReadLimit::First(n) => n,
    };
    let mut out = Vec::with_capacity(cap.min(64 * 1024));
    walk_riff_movi(path, |chunk| {
        out.extend_from_slice(chunk);
        out.len() < cap
    })?;
    out.truncate(cap);
    if out.is_empty() {
        return Err(ImageReaderError::new("could not locate avi movie data (movi)"));
    }
    Ok(out)
}

/// Full (uncapped) `movi` payload hash, streamed.
fn hash_riff_movi(path: &Path) -> Result<u64, ImageReaderError> {
    use std::hash::Hasher;

    let mut hasher = seahash::SeaHasher::new();
    let mut bytes: u64 = 0;
    walk_riff_movi(path, |chunk| {
        hasher.write(chunk);
        bytes += chunk.len() as u64;
        true
    })?;
    if bytes == 0 {
        return Err(ImageReaderError::new("could not locate avi movie data (movi)"));
    }
    Ok(hasher.finish())
}

/// Walk the top-level RIFF chunks of an AVI and feed every `LIST movi`
/// payload to `sink` in file order (chunk headers excluded). The sink
/// returns `false` to stop early. RIFF chunks are little-endian with
/// even-byte padding.
fn walk_riff_movi(
    path: &Path,
    mut sink: impl FnMut(&[u8]) -> bool,
) -> Result<(), ImageReaderError> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let file_len = f
        .metadata()
        .map_err(|e| ImageReaderError::new(e.to_string()))?
        .len();

    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr)
        .map_err(|e| ImageReaderError::new(e.to_string()))?;
    if !looks_like_riff_avi(&hdr) {
        return Err(ImageReaderError::new("not a RIFF/AVI file"));
    }

    let mut buf = [0u8; 64 * 1024];
    let mut pos: u64 = 12;
    while pos + 8 <= file_len {
        let mut chunk_hdr = [0u8; 8];
        f.seek(SeekFrom::Start(pos))
            .map_err(|e| ImageReaderError::new(e.to_string()))?;
        f.read_exact(&mut chunk_hdr)
            .map_err(|e| ImageReaderError::new(e.to_string()))?;
        let size = u32::from_le_bytes([chunk_hdr[4], chunk_hdr[5], chunk_hdr[6], chunk_hdr[7]]) as u64;
        let data_start = pos + 8;
        let data_end = match data_start.checked_add(size) {
            Some(end) if end <= file_len => end,
            _ => return Err(ImageReaderError::new(format!("malformed avi chunk at offset {pos}"))),
        };

        if &chunk_hdr[..4] == b"LIST" && size >= 4 {
            f.seek(SeekFrom::Start(data_start))
                .map_err(|e| ImageReaderError::new(e.to_string()))?;
            let mut subtype = [0u8; 4];
            f.read_exact(&mut subtype)
                .map_err(|e| ImageReaderError::new(e.to_string()))?;
            if &subtype == b"movi" {
                let mut remaining = size - 4;
                while remaining > 0 {
                    let want = (remaining as usize).min(buf.len());
                    let n = f
                        .read(&mut buf[..want])
                        .map_err(|e| ImageReaderError::new(e.to_string()))?;
                    if n == 0 {
                        return Err(ImageReaderError::new("unexpected EOF inside movi"));
                    }
                    if !sink(&buf[..n]) {
                        return Ok(());
                    }
                    remaining -= n as u64;
                }
            }
        }

        pos = data_end + (size & 1); // RIFF chunks are word-aligned
    }
    Ok(())
}

/// Bounded payload of an MTS/M2TS stream for the fast (64kb head) hash: all
/// bytes except the per-packet 4-byte arrival timestamps.
fn ts_image_data(path: &Path, limit: ReadLimit) -> Result<Vec<u8>, ImageReaderError> {
    use std::io::{Read, Seek};

    let cap = match limit {
        ReadLimit::All => MAX_MOVIE_DATA,
        ReadLimit::First(n) => n,
    };
    let mut f = std::fs::File::open(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let mut first = vec![0u8; M2TS_PACKET];
    let n = read_up_to(&mut f, &mut first);
    if n < M2TS_PACKET {
        return Err(ImageReaderError::new("file too small to be an m2ts stream"));
    }
    first.truncate(n);
    if !looks_like_transport_stream(&first) {
        return Err(ImageReaderError::new("not an mpeg transport stream"));
    }
    let m2ts = first[4] == 0x47;
    if !m2ts {
        // Raw 188-byte transport stream: no timestamp prefix to strip.
        let data = std::fs::read(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
        return Ok(data[..data.len().min(cap)].to_vec());
    }
    // The detection read consumed the first packet; start over.
    f.seek(std::io::SeekFrom::Start(0))
        .map_err(|e| ImageReaderError::new(e.to_string()))?;

    let mut out = Vec::with_capacity(cap.min(64 * 1024));
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| ImageReaderError::new(e.to_string()))?;
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        let packets = pending.len() / M2TS_PACKET;
        let consumed = packets * M2TS_PACKET;
        for i in 0..packets {
            // Skip the 4-byte arrival timestamp and the constant sync byte.
            let start = i * M2TS_PACKET + 5;
            out.extend_from_slice(&pending[start..start + 187]);
            if out.len() >= cap {
                out.truncate(cap);
                return Ok(out);
            }
        }
        pending.drain(..consumed);
    }
    out.extend_from_slice(&pending);
    out.truncate(cap);
    Ok(out)
}

/// Full (uncapped) MTS/M2TS payload hash, streamed: per-packet arrival
/// timestamps excluded, everything else hashed in file order.
fn hash_ts_payload(path: &Path) -> Result<u64, ImageReaderError> {
    use std::hash::Hasher;
    use std::io::{Read, Seek};

    let mut f = std::fs::File::open(path).map_err(|e| ImageReaderError::new(e.to_string()))?;
    let mut first = vec![0u8; M2TS_PACKET];
    let n = read_up_to(&mut f, &mut first);
    if n < M2TS_PACKET {
        return Err(ImageReaderError::new("file too small to be an m2ts stream"));
    }
    first.truncate(n);
    if !looks_like_transport_stream(&first) {
        return Err(ImageReaderError::new("not an mpeg transport stream"));
    }
    let m2ts = first[4] == 0x47;
    // The detection read consumed the first packet; start over.
    f.seek(std::io::SeekFrom::Start(0))
        .map_err(|e| ImageReaderError::new(e.to_string()))?;

    let mut hasher = seahash::SeaHasher::new();
    if !m2ts {
        // Raw 188-byte transport stream: hash every byte.
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = f.read(&mut buf).map_err(|e| ImageReaderError::new(e.to_string()))?;
            if n == 0 {
                break;
            }
            hasher.write(&buf[..n]);
        }
        return Ok(hasher.finish());
    }

    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| ImageReaderError::new(e.to_string()))?;
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        let packets = pending.len() / M2TS_PACKET;
        let consumed = packets * M2TS_PACKET;
        for i in 0..packets {
            // Skip the 4-byte arrival timestamp and the constant sync byte.
            let start = i * M2TS_PACKET + 5;
            hasher.write(&pending[start..start + 187]);
        }
        pending.drain(..consumed);
    }
    // Trailing partial packet (should not happen in a well-formed file) is
    // hashed as-is so the value stays deterministic.
    hasher.write(&pending);
    Ok(hasher.finish())
}

/// Read as much as fits into `buf`, returning the byte count (EOF is not an
/// error here).
fn read_up_to(f: &mut std::fs::File, buf: &mut [u8]) -> usize {
    use std::io::Read;
    let mut total = 0;
    while total < buf.len() {
        match f.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(_) => break,
        }
    }
    total
}

fn u32_at(data: &[u8], i: usize) -> Option<u32> {
    let b = data.get(i..i + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(data: &[u8], i: usize) -> Option<u64> {
    let b = data.get(i..i + 8)?;
    Some(u64::from_be_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}
