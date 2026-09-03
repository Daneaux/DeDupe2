#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn u16(&self, data: &[u8], pos: usize) -> Option<u16> {
        let b = data.get(pos..pos + 2)?;
        Some(match self {
            Endian::Little => u16::from_le_bytes([b[0], b[1]]),
            Endian::Big => u16::from_be_bytes([b[0], b[1]]),
        })
    }

    fn u32(&self, data: &[u8], pos: usize) -> Option<u32> {
        let b = data.get(pos..pos + 4)?;
        Some(match self {
            Endian::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Endian::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        })
    }
}

fn detect_endian(data: &[u8]) -> Option<Endian> {
    match data.get(0..2)? {
        b"II" => Some(Endian::Little),
        b"MM" => Some(Endian::Big),
        _ => None,
    }
}

fn type_size(ftype: u16) -> usize {
    match ftype {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 4,
    }
}

fn ifd_entries(data: &[u8], ifd: usize, endian: Endian) -> Option<Vec<(u16, u16, u32, u32)>> {
    let count = endian.u16(data, ifd)? as usize;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let pos = ifd + 2 + i * 12;
        let tag = endian.u16(data, pos)?;
        let ftype = endian.u16(data, pos + 2)?;
        let fcount = endian.u32(data, pos + 4)?;
        let value = endian.u32(data, pos + 8)?;
        entries.push((tag, ftype, fcount, value));
    }
    Some(entries)
}

fn read_values(data: &[u8], value: u32, count: u32, ftype: u16, endian: Endian) -> Vec<u32> {
    let tsize = type_size(ftype);
    let mut out = Vec::with_capacity(count as usize);
    if count as usize * tsize <= 4 {
        out.push(value);
    } else {
        for i in 0..count as usize {
            let pos = value as usize + i * tsize;
            let v = match tsize {
                1 => *data.get(pos).unwrap_or(&0) as u32,
                2 => endian.u16(data, pos).unwrap_or(0) as u32,
                4 => endian.u32(data, pos).unwrap_or(0),
                _ => 0,
            };
            out.push(v);
        }
    }
    out
}

fn sub_ifd_offsets(data: &[u8], ifd0: usize, endian: Endian) -> Vec<usize> {
    let entries = match ifd_entries(data, ifd0, endian) {
        Some(e) => e,
        None => return Vec::new(),
    };
    let mut subs = Vec::new();
    for (tag, ftype, count, value) in entries {
        if tag == 0x14A {
            for v in read_values(data, value, count, ftype, endian) {
                subs.push(v as usize);
            }
        }
    }
    subs
}

pub fn raw_data(data: &[u8]) -> Option<Vec<u8>> {
    let endian = detect_endian(data)?;

    let mut candidates = Vec::new();
    if data.len() >= 12 && &data[8..12] == b"CR\x02\0" {
        if let Some(off) = endian.u32(data, 12) {
            candidates.push(off as usize);
        }
    } else {
        let ifd0 = endian.u32(data, 4)? as usize;
        candidates.push(ifd0);
        candidates.extend(sub_ifd_offsets(data, ifd0, endian));
    }

    for ifd in candidates {
        if let Some(bytes) = extract_strips(data, ifd, endian) {
            return Some(bytes);
        }
    }
    None
}

fn extract_strips(data: &[u8], ifd: usize, endian: Endian) -> Option<Vec<u8>> {
    let entries = ifd_entries(data, ifd, endian)?;
    let mut offsets: Vec<u32> = Vec::new();
    let mut counts: Vec<u32> = Vec::new();
    for (tag, ftype, count, value) in entries {
        match tag {
            0x111 | 0x144 => offsets = read_values(data, value, count, ftype, endian),
            0x117 | 0x145 => counts = read_values(data, value, count, ftype, endian),
            _ => {}
        }
    }
    if offsets.is_empty() || offsets.len() != counts.len() {
        return None;
    }

    let mut out = Vec::new();
    for (o, l) in offsets.iter().zip(counts.iter()) {
        let start = *o as usize;
        let end = start + *l as usize;
        out.extend_from_slice(data.get(start..end)?);
    }
    Some(out)
}
