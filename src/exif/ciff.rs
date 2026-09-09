//! Canon CIFF (CRW) container: `creation_date` support.
//!
//! nom-exif does not understand CIFF, and rawler's CRW decoder does not parse
//! metadata yet, so the shot date is read directly from the CIFF directory:
//! tag `0x180e` holds the capture time as a Unix timestamp (u32 LE) followed
//! by TimeZoneCode and TimeZoneInfo (u32 each).

use std::path::Path;

use chrono::TimeZone;

use super::CreationDate;

const CIFF_SIGNATURE: &[u8] = b"HEAPCCDR";
const CIFF_DATE_TAG: u16 = 0x180e;

fn u16_at(data: &[u8], i: usize) -> Option<u16> {
    let b = data.get(i..i + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], i: usize) -> Option<u32> {
    let b = data.get(i..i + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Read the CIFF shot date, if the file is a CIFF container.
pub fn creation_date(path: &Path) -> Option<CreationDate> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 16 || data.get(6..14) != Some(CIFF_SIGNATURE) {
        return None;
    }
    let heap = usize::try_from(u32_at(&data, 2)?).ok()?;
    let ts = find_date(&data, heap, data.len(), 0)?;
    let dt = chrono::Utc.timestamp_opt(i64::from(u32::from_le_bytes(ts)), 0).single()?;
    Some(CreationDate::DateCreated(
        dt.format("%Y-%m-%d %H:%M:%S").to_string(),
    ))
}

/// Depth-first walk of the CIFF directory tree looking for `CIFF_DATE_TAG`.
fn find_date(data: &[u8], start: usize, end: usize, depth: u32) -> Option<[u8; 4]> {
    if depth > 10 {
        return None;
    }
    let value_data_size = usize::try_from(u32_at(data, end.checked_sub(4)?)?).ok()?;
    let count = u16_at(data, start.checked_add(value_data_size)?)? as usize;

    for i in 0..count {
        let entry = start
            .checked_add(value_data_size)?
            .checked_add(2)?
            .checked_add(i.checked_mul(10)?)?;
        let prop = u16_at(data, entry)?;
        let tag = prop & 0x3fff;
        let location = prop & 0xc000;
        let kind = prop & 0x3800;

        let (size, offset) = if location == 0x4000 {
            (8usize, entry.checked_add(2)?)
        } else if location == 0x0000 {
            let size = usize::try_from(u32_at(data, entry.checked_add(2)?)?).ok()?;
            let off = usize::try_from(u32_at(data, entry.checked_add(6)?)?).ok()?;
            (size, start.checked_add(off)?)
        } else {
            continue;
        };
        let end = offset.checked_add(size)?;
        if data.len() < end {
            continue;
        }

        if kind == 0x2800 || kind == 0x3000 {
            if let Some(found) = find_date(data, offset, end, depth + 1) {
                return Some(found);
            }
        } else if tag == CIFF_DATE_TAG && size >= 4 {
            let b = data.get(offset..offset + 4)?;
            return Some([b[0], b[1], b[2], b[3]]);
        }
    }
    None
}
