mod ciff;

pub const EXIF_TIMEOUT: Duration = Duration::from_secs(10);

use std::fmt;
use std::time::Duration;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use nom_exif::{read_metadata, ExifTag, Metadata, TrackInfoTag};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CreationDate {
    DateCreated(String),
    Unknown,
}

impl CreationDate {
    pub fn value(&self) -> Option<&str> {
        match self {
            CreationDate::DateCreated(v) => Some(v),
            CreationDate::Unknown => None,
        }
    }
}

impl fmt::Display for CreationDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CreationDate::DateCreated(v) => f.write_str(v),
            CreationDate::Unknown => f.write_str("unknown"),
        }
    }
}

pub fn creation_date(path: &Path) -> CreationDate {
    creation_date_with_timeout(path, EXIF_TIMEOUT)
}

/// `creation_date` with a hard per-file time limit. Decoders can stall
/// indefinitely on malformed files; a timed-out file is abandoned and falls
/// back to the folder-name proxy date. (A leaked worker thread per stalled
/// file is the price of never freezing a scan.)
pub fn creation_date_with_timeout(path: &Path, timeout: Duration) -> CreationDate {
    let proxy_path = path.to_path_buf();
    date_with_timeout(
        path,
        timeout,
        creation_date_inner,
        folder_proxy_date(&proxy_path).unwrap_or(CreationDate::Unknown),
    )
}

fn date_with_timeout(
    path: &Path,
    timeout: Duration,
    f: fn(&Path) -> CreationDate,
    fallback: CreationDate,
) -> CreationDate {
    let worker_path = path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f(&worker_path));
    });

    match rx.recv_timeout(timeout) {
        Ok(date) => date,
        Err(_) => fallback,
    }
}

fn creation_date_inner(path: &Path) -> CreationDate {
    let date = creation_date_readers(path);
    if date != CreationDate::Unknown {
        return date;
    }
    folder_proxy_date(path).unwrap_or(CreationDate::Unknown)
}

/// Everything except the folder proxy: nom-exif -> CIFF -> rawler -> XMP.
/// Used per-file inside the batch resolver (with its own timeout) and by
/// `creation_date_inner` above.
fn creation_date_readers(path: &Path) -> CreationDate {
    let from_exif = match read_metadata(path) {
        Ok(metadata) => match metadata {
            Metadata::Exif(exif) => resolve(
                exif.get(ExifTag::DateTimeOriginal).map(|v| v.to_string()),
                exif.get(ExifTag::CreateDate).map(|v| v.to_string()),
            ),
            Metadata::Track(track) => resolve(
                None,
                track.get(TrackInfoTag::CreateDate).map(|v| v.to_string()),
            ),
        },
        Err(_) => CreationDate::Unknown,
    };

    if from_exif != CreationDate::Unknown {
        return from_exif;
    }

    // Canon CRW (CIFF): nom-exif can't read it and rawler's CRW decoder does
    // not parse metadata yet, so the shot date comes from the CIFF tree.
    if let Some(date) = ciff::creation_date(path) {
        return date;
    }

    // MP4/MOV/MTS are nom-exif's (and later exiftool's) domain; rawler's
    // movie decoders gain nothing here, so don't fall back into them.
    let movie = crate::image_reader::is_video(path);
    if !movie {
        let from_rawler = raw_creation_date(path);
        if from_rawler != CreationDate::Unknown {
            return from_rawler;
        }
    }

    // Adobe workflow exports (HDR blends, re-saved TIFF/PSD files) often
    // drop classic EXIF dates entirely and keep the shot time only in an
    // embedded XMP packet.
    if let Some(date) = xmp_creation_date(path) {
        return date;
    }

    CreationDate::Unknown
}

/// Derive a date from the file's folder layout: a `MM-DD` event folder under a
/// `YYYY` year folder, or a folder named `YYYY-MM-DD` directly.
pub fn folder_proxy_date(path: &Path) -> Option<CreationDate> {
    let parent = path.parent()?.file_name()?.to_str()?.to_string();

    if let Some((y, m, d)) = parse_ymd_folder(&parent) {
        return Some(date_ymd(y, m, d));
    }

    if let Some((m, d)) = parse_md_folder(&parent) {
        let grand = path.parent()?.parent()?.file_name()?.to_str()?.to_string();
        if grand.len() == 4 && grand.bytes().all(|c| c.is_ascii_digit()) {
            if let Ok(y) = grand.parse::<u32>() {
                if (1..=12).contains(&m) && (1..=31).contains(&d) {
                    return Some(date_ymd(y, m, d));
                }
            }
        }
    }
    None
}

fn date_ymd(y: u32, m: u32, d: u32) -> CreationDate {
    CreationDate::DateCreated(format!("{y:04}-{m:02}-{d:02}"))
}

pub fn parse_ymd_folder(name: &str) -> Option<(u32, u32, u32)> {
    let b = name.as_bytes();
    let digit = |i: usize| b.get(i).map(|c| c.is_ascii_digit()).unwrap_or(false);
    if b.len() >= 10
        && (0..4).all(digit)
        && b[4] == b'-'
        && digit(5)
        && digit(6)
        && b[7] == b'-'
        && digit(8)
        && digit(9)
    {
        let y = name.get(0..4)?.parse().ok()?;
        let m = name.get(5..7)?.parse().ok()?;
        let d = name.get(8..10)?.parse().ok()?;
        return Some((y, m, d));
    }
    None
}

pub fn parse_md_folder(name: &str) -> Option<(u32, u32)> {
    let b = name.as_bytes();
    let digit = |i: usize| b.get(i).map(|c| c.is_ascii_digit()).unwrap_or(false);
    if b.len() >= 5 && digit(0) && digit(1) && b[2] == b'-' && digit(3) && digit(4) {
        let m = name.get(0..2)?.parse().ok()?;
        let d = name.get(3..5)?.parse().ok()?;
        return Some((m, d));
    }
    None
}

/// XMP-date fallback by file kind: TIFFs expose their XMP packet through an
/// IFD entry (cheap, targeted read — the packet can sit past 100MB into a
/// huge scan stack), everything else is scanned with a bounded prefix scan.
fn xmp_creation_date(path: &Path) -> Option<CreationDate> {
    let bytes = xmp_packet_bytes(path)?;
    xmp_date(&bytes)
}

fn is_tiff_ext(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(ext) if ext.eq_ignore_ascii_case("tif") || ext.eq_ignore_ascii_case("tiff")
    )
}

const XMP_SCAN_CAP: u64 = 16 * 1024 * 1024;
const XMP_PACKET_MAX: usize = 4 * 1024 * 1024;

fn xmp_packet_bytes(path: &Path) -> Option<Vec<u8>> {
    if is_tiff_ext(path) {
        return tiff_xmp_bytes(path);
    }
    stream_scan_xmp(path)
}

/// Walk the TIFF IFD chain for an 0x02bc (XMP) entry and read its bytes.
fn tiff_xmp_bytes(path: &Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path).ok()?;
    let mut hdr = [0u8; 8];
    f.read_exact(&mut hdr).ok()?;
    let little = match &hdr[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let rd16 = |b: &[u8]| if little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) };
    let rd32 = |b: &[u8]| {
        if little {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        }
    };

    let mut ifd_off = rd32(&hdr[4..8]) as u64;
    let mut visited = 0;
    while visited < 8 && ifd_off > 0 {
        visited += 1;
        f.seek(SeekFrom::Start(ifd_off)).ok()?;
        let mut cnt_buf = [0u8; 2];
        f.read_exact(&mut cnt_buf).ok()?;
        let n = rd16(&cnt_buf) as usize;
        if n > 4096 {
            return None;
        }
        let mut entries = vec![0u8; n * 12 + 4];
        f.read_exact(&mut entries).ok()?;
        for i in 0..n {
            let e = &entries[i * 12..i * 12 + 12];
            let tag = rd16(&e[0..2]);
            let typ = rd16(&e[2..4]);
            let count = rd32(&e[4..8]) as usize;
            if tag != 0x02bc || !(typ == 1 || typ == 7) {
                continue;
            }
            if count == 0 || count > XMP_PACKET_MAX {
                return None;
            }
            let pos = if count <= 4 { ifd_off + 2 + (i as u64) * 12 + 8 } else { rd32(&e[8..12]) as u64 };
            f.seek(SeekFrom::Start(pos)).ok()?;
            let mut out = vec![0u8; count];
            f.read_exact(&mut out).ok()?;
            if out.starts_with(b"<?xpacket") || out.windows(10).any(|w| w == b"<x:xmpmeta") {
                return Some(out);
            }
            return None;
        }
        ifd_off = rd32(&entries[n * 12..n * 12 + 4]) as u64;
    }
    None
}

/// Bounded prefix scan for an embedded XMP packet ("<?xpacket" /
/// "<x:xmpmeta"), for non-TIFF files (XMP in a JPEG APP1 segment sits in the
/// first kilobytes).
fn stream_scan_xmp(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;

    let mut f = std::fs::File::open(path).ok()?;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut scanned: u64 = 0;
    loop {
        let n = f.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        scanned += n as u64;
        buf.extend_from_slice(&chunk[..n]);
        if let Some(start) = find_xmp_marker(&buf) {
            // Gather until the packet closes (or the cap).
            loop {
                if buf.len() >= start + XMP_PACKET_MAX {
                    break;
                }
                let close = buf[start..]
                    .windows(11)
                    .position(|w| w == b"</x:xmpmeta");
                if close.is_some() {
                    break;
                }
                let n = f.read(&mut chunk).ok()?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            return Some(buf[start..].to_vec());
        }
        if scanned >= XMP_SCAN_CAP {
            return None;
        }
        // Keep a rolling tail so a marker can never be cut off mid-way.
        if buf.len() > 64 * 1024 {
            let keep = 8 * 1024;
            buf.drain(..buf.len() - keep);
        }
    }
}

fn find_xmp_marker(buf: &[u8]) -> Option<usize> {
    buf.windows(9)
        .position(|w| w == b"<?xpacket")
        .or_else(|| buf.windows(10).position(|w| w == b"<x:xmpmeta"))
}

/// XMP packet bytes -> shot date. Prefers the closest thing to "when the
/// shutter fired": exif:DateTimeOriginal, then photoshop:DateCreated, then
/// xmp:CreateDate.
fn xmp_date(packet: &[u8]) -> Option<CreationDate> {
    let text = String::from_utf8_lossy(packet);
    for tag in ["exif:DateTimeOriginal", "photoshop:DateCreated", "xmp:CreateDate"] {
        if let Some(value) = xmp_attribute(&text, tag) {
            if let Some(date) = xmp_timestamp(value) {
                return Some(date);
            }
        }
    }
    None
}

fn xmp_attribute<'t>(text: &'t str, tag: &str) -> Option<&'t str> {
    let start = text.find(tag)? + tag.len();
    let after = &text[start..];
    let eq = after.find('=')?;
    let after_eq = &after[eq + 1..];
    let q1 = after_eq.find('"')?;
    let value_rest = &after_eq[q1 + 1..];
    let q2 = value_rest.find('"')?;
    Some(&value_rest[..q2])
}

/// "2011-11-05T16:56:37.03" | "2011-11-05T20:55:20+01:00" | "2011-11-05"
/// -> normalized "YYYY-MM-DD HH:MM:SS" | "YYYY-MM-DD".
fn xmp_timestamp(value: &str) -> Option<CreationDate> {
    let b = value.as_bytes();
    let digit = |i: usize| b.get(i).map(|c| c.is_ascii_digit()).unwrap_or(false);
    if !(digit(0) && digit(1) && digit(2) && digit(3))
        || b.get(4) != Some(&b'-')
        || !(digit(5) && digit(6))
        || b.get(7) != Some(&b'-')
        || !(digit(8) && digit(9))
    {
        return None;
    }
    let date = std::str::from_utf8(&b[..10]).ok()?.to_string();
    let time = match b.get(10) {
        Some(b'T')
            if b.len() >= 19
                && digit(11) && digit(12) && b.get(13) == Some(&b':')
                && digit(14) && digit(15) && b.get(16) == Some(&b':')
                && digit(17) && digit(18) =>
        {
            Some(std::str::from_utf8(&b[11..19]).ok()?.to_string())
        }
        _ => None,
    };
    Some(CreationDate::DateCreated(match time {
        Some(t) => format!("{date} {t}"),
        None => date,
    }))
}

/// Resolve creation dates for a whole scan: in-process readers per file
/// (each with its own stall timeout), one batched exiftool subprocess for
/// whatever the readers could not date, and the folder-name proxy last.
/// Exactly one exiftool process is spawned no matter how many files fail
/// the in-process readers.
pub fn creation_dates_batch(
    paths: &[PathBuf],
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<CreationDate> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let total = paths.len();
    let done = AtomicUsize::new(0);
    let mut dates: Vec<CreationDate> = paths
        .par_iter()
        .map(|path| {
            let date = date_with_timeout(path, EXIF_TIMEOUT, creation_date_readers, CreationDate::Unknown);
            // Report as each file resolves so the UI moves during the (slow)
            // reader pass — not only after the whole pass has finished.
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            date
        })
        .collect();

    let mut unresolved: Vec<usize> = Vec::new();
    for (i, date) in dates.iter().enumerate() {
        if *date == CreationDate::Unknown {
            unresolved.push(i);
        }
    }

    if !unresolved.is_empty() {
        let paths_to_ask: Vec<PathBuf> = unresolved.iter().map(|&i| paths[i].clone()).collect();
        if let Ok(map) = exiftool_dates_batch_with_timeout(&paths_to_ask) {
            for &i in &unresolved {
                if let Some(date) = map.get(&paths[i]) {
                    dates[i] = date.clone();
                }
            }
        }
    }

    for &i in &unresolved {
        if dates[i] == CreationDate::Unknown {
            dates[i] = folder_proxy_date(&paths[i]).unwrap_or(CreationDate::Unknown);
        }
    }

    dates
}

const EXIFTOOL_BATCH_MAX_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// One exiftool process for every undated file, guarded by an overall
/// timeout (a hung binary must never freeze the scan; files fall back to
/// the folder proxy).
fn exiftool_dates_batch_with_timeout(
    paths: &[PathBuf],
) -> Result<HashMap<PathBuf, CreationDate>, ()> {
    let owned: Vec<PathBuf> = paths.to_vec();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(exiftool_dates_batch(&owned));
    });

    let timeout = (EXIF_TIMEOUT + Duration::from_millis(25 * paths.len() as u64))
        .min(EXIFTOOL_BATCH_MAX_TIMEOUT);
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!(
                "exiftool batch timed out after {timeout:?} for {} files; falling back to folder dates",
                paths.len()
            );
            Err(())
        }
    }
}

/// Run exiftool once over every path (via an `-@` argsfile so argument
/// length and odd path characters are not a problem) and pick the shot date
/// out of its `-j -G1` JSON report.
fn exiftool_dates_batch(paths: &[PathBuf]) -> Result<HashMap<PathBuf, CreationDate>, ()> {
    use std::io::Write;
    use std::process::Command;

    if paths.is_empty() {
        return Ok(HashMap::new());
    }

    let argsfile = std::env::temp_dir().join(format!(
        "dedupe2-exiftool-{}.args",
        std::process::id()
    ));
    let mut f = std::fs::File::create(&argsfile).map_err(|_| ())?;
    for opt in [
        "-q",
        "-j",
        "-G1",
        "-DateTimeOriginal",
        "-CreateDate",
        "-DateCreated",
        "-DigitalCreationDate",
        "-DateTimeCreated",
    ] {
        writeln!(f, "{opt}").map_err(|_| ())?;
    }
    for path in paths {
        let mut line = path.display().to_string();
        if line.starts_with('-') {
            line.insert_str(0, "./"); // exiftool would read it as an option
        }
        writeln!(f, "{line}").map_err(|_| ())?;
    }
    drop(f);

    let out = Command::new("exiftool")
        .arg("-@")
        .arg(&argsfile)
        .output();
    let _ = std::fs::remove_file(&argsfile);

    let out = out.map_err(|_| ())?;
    if !out.status.success() {
        return Err(());
    }
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).map_err(|_| ())?;

    let mut map = HashMap::new();
    for row in &rows {
        let Some(source) = row.get("SourceFile").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(date) = pick_exiftool_date(row) {
            map.insert(PathBuf::from(source), date);
        }
    }
    Ok(map)
}

/// Date selection from one `-j -G1` row, in priority order. `IPTC:DateCreated`
/// and `IPTC:DigitalCreationDate` are date-only, so their companion time
/// fields (`TimeCreated` / `DigitalCreationTime`) complete the stamp when
/// present. Public so tests can exercise it without an exiftool binary.
pub fn pick_exiftool_date(row: &serde_json::Value) -> Option<CreationDate> {
    // Tag names in priority order (closest to "when the shutter fired"
    // first), matched regardless of exiftool's group prefix so QuickTime,
    // M2TS, XMP, ExifIFD, Composite and IPTC records are all eligible.
    const NAME_PRIORITY: &[&str] = &[
        "DateTimeOriginal",
        "DateTimeCreated",
        "CreateDate",
        "DateCreated",
        "DigitalCreationDate",
    ];
    // For equal tag names, prefer the group closer to the camera.
    const GROUP_PREFERENCE: &[&str] = &[
        "ExifIFD:", "XMP:", "Composite:", "QuickTime:", "Track:", "M2TS:", "IPTC:",
    ];

    let obj = row.as_object()?;
    let tag_name = |key: &str| key.rsplit(':').next().unwrap_or(key).to_string();
    let group_rank = |key: &str| {
        GROUP_PREFERENCE
            .iter()
            .position(|g| key.starts_with(g))
            .unwrap_or(GROUP_PREFERENCE.len())
    };

    for name in NAME_PRIORITY {
        let mut keys: Vec<&String> = obj
            .keys()
            .filter(|k| {
                tag_name(k) == *name
                    && obj.get(k.as_str()).map(|v| v.is_string()).unwrap_or(false)
            })
            .collect();
        keys.sort_by_key(|k| (group_rank(k), k.as_str()));

        for key in keys {
            let Some(raw) = obj.get(key.as_str()).and_then(|v| v.as_str()) else {
                continue;
            };
            // IPTC date-only tags carry the clock in a companion field.
            let companion = if *name == "DateCreated" {
                Some("TimeCreated")
            } else if *name == "DigitalCreationDate" {
                Some("DigitalCreationTime")
            } else {
                None
            };
            let time = companion.and_then(|companion| {
                let prefix = key.rsplit_once(':').map(|(p, _)| p.to_string())?;
                let full_key = format!("{prefix}:{companion}");
                obj.get(full_key.as_str()).and_then(|v| v.as_str())
            });
            if let Some(date) = exiftool_timestamp(raw, time) {
                return Some(date);
            }
        }
    }
    None
}

/// Parse an exiftool timestamp string ("2011:11:05 16:56:37",
/// "2011-11-05T16:56:37.03+01:00", "2011:11:05" ...) into the canonical
/// `YYYY-MM-DD HH:MM:SS` / `YYYY-MM-DD` shape, optionally completing a
/// date-only value with an IPTC time field.
fn exiftool_timestamp(raw: &str, time: Option<&str>) -> Option<CreationDate> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime};

    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    // timezone-aware forms first (input carries an offset)
    for fmt in [
        "%Y:%m:%d %H:%M:%S%z",
        "%Y-%m-%dT%H:%M:%S%.f%z",
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%d %H:%M:%S%z",
    ] {
        if let Ok(dt) = DateTime::parse_from_str(raw, fmt) {
            // Keep the wall-clock time as recorded (offsets are metadata,
            // typically DST-inflated); drop the timezone.
            return Some(CreationDate::DateCreated(
                dt.naive_local().format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
    }

    // naive forms
    for fmt in [
        "%Y:%m:%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(raw, fmt) {
            return Some(CreationDate::DateCreated(
                ndt.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
    }

    let day = ["%Y:%m:%d", "%Y-%m-%d"]
        .iter()
        .find_map(|fmt| NaiveDate::parse_from_str(raw, fmt).ok())?;

    if let Some(t) = time {
        if let Ok(clock) = NaiveTime::parse_from_str(t.trim(), "%H:%M:%S") {
            return Some(CreationDate::DateCreated(
                day.and_time(clock).format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
    }

    Some(CreationDate::DateCreated(day.format("%Y-%m-%d").to_string()))
}


/// Pick the capture date from rawler's parsed EXIF fields, in priority
/// order: DateTimeOriginal (when the shutter fired) > CreateDate > ModifyDate
/// (the file-change date, which can be arbitrarily recent and wrong).
pub fn select_capture_date(exif: &rawler::exif::Exif) -> Option<CreationDate> {
    for value in [
        exif.date_time_original.as_ref(),
        exif.create_date.as_ref(),
        exif.modify_date.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        for fmt in ["%Y:%m:%d %H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y:%m:%d"] {
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(value, fmt) {
                return Some(CreationDate::DateCreated(
                    dt.format("%Y-%m-%d %H:%M:%S").to_string(),
                ));
            }
            if let Ok(d) = chrono::NaiveDate::parse_from_str(value, fmt) {
                return Some(CreationDate::DateCreated(d.format("%Y-%m-%d").to_string()));
            }
        }
    }
    None
}

/// rawler can panic on corrupt/mislabeled raw files (it slices at offsets
/// read from the file), so the whole decode is unwound per file and treated
/// like any other unreadable file.
fn raw_creation_date(path: &Path) -> CreationDate {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| raw_creation_date_inner(path)))
        .unwrap_or(CreationDate::Unknown)
}

fn raw_creation_date_inner(path: &Path) -> CreationDate {
    let rawfile = match rawler::rawsource::RawSource::new(path) {
        Ok(rawfile) => rawfile,
        Err(_) => return CreationDate::Unknown,
    };

    let loader = rawler::RawLoader::new();
    let decoder = match loader.get_decoder(&rawfile) {
        Ok(decoder) => decoder,
        Err(_) => return CreationDate::Unknown,
    };

    let metadata = match decoder.raw_metadata(&rawfile, &rawler::decoders::RawDecodeParams::default()) {
        Ok(metadata) => metadata,
        Err(_) => return CreationDate::Unknown,
    };

    // Prefer the EXIF capture dates (DateTimeOriginal > CreateDate >
    // ModifyDate). rawler's `last_modified()` only reads ModifyDate (0x0132,
    // the "file changed" date), which many files do not carry — e.g.
    // Panasonic RW2 stores only DateTimeOriginal/CreateDate.
    if let Some(date) = select_capture_date(&metadata.exif) {
        return date;
    }

    match metadata.last_modified() {
        Ok(Some(t)) => CreationDate::DateCreated(format_system_time(t)),
        _ => CreationDate::Unknown,
    }
}

fn format_system_time(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn resolve(dt_original: Option<String>, create_date: Option<String>) -> CreationDate {
    dt_original
        .or(create_date)
        .map(CreationDate::DateCreated)
        .unwrap_or(CreationDate::Unknown)
}
