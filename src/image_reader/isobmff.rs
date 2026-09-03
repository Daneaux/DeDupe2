use std::collections::HashMap;

#[derive(Debug)]
struct Extent {
    construction_method: u8,
    offset: u64,
    length: u64,
}

fn read_u16(data: &[u8], pos: usize) -> Option<u16> {
    let b = data.get(pos..pos + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn read_u32(data: &[u8], pos: usize) -> Option<u32> {
    let b = data.get(pos..pos + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64(data: &[u8], pos: usize) -> Option<u64> {
    let b = data.get(pos..pos + 8)?;
    Some(u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

fn read_uint(data: &[u8], pos: usize, size: u8) -> Option<u64> {
    let mut v = 0u64;
    for i in 0..size as usize {
        v = (v << 8) | (*data.get(pos + i)? as u64);
    }
    Some(v)
}

fn find_box(data: &[u8], start: usize, end: usize, box_type: &[u8; 4]) -> Option<(usize, usize)> {
    let mut pos = start;
    while pos + 8 <= end {
        let size = read_u32(data, pos)? as usize;
        let typ = data.get(pos + 4..pos + 8)?;
        let (hdr, box_size) = match size {
            0 => (8, end - pos),
            1 => (16, read_u64(data, pos + 8)? as usize),
            n => (8, n),
        };
        if typ == &box_type[..] {
            return Some((pos, box_size));
        }
        if box_size < hdr {
            return None;
        }
        pos += box_size;
    }
    None
}

pub(crate) fn extract_primary_image_data(data: &[u8]) -> Option<Vec<u8>> {
    let (meta_start, meta_size) = find_box(data, 0, data.len(), b"meta")?;
    let children_start = meta_start + 12;
    let children_end = meta_start + meta_size;

    let idat_offset = find_box(data, children_start, children_end, b"idat").map(|(s, _)| s + 8);

    let (pitm_start, _) = find_box(data, children_start, children_end, b"pitm")?;
    let primary_id = parse_pitm(data, pitm_start)?;

    let (iloc_start, _) = find_box(data, children_start, children_end, b"iloc")?;
    let locations = parse_iloc(data, iloc_start)?;

    let image_ids = match find_box(data, children_start, children_end, b"iref") {
        Some((iref_start, _)) => {
            let refs = parse_dimg_refs(data, iref_start, primary_id);
            if refs.is_empty() {
                vec![primary_id]
            } else {
                refs
            }
        }
        None => vec![primary_id],
    };

    let mut out = Vec::new();
    for id in &image_ids {
        let exts = locations.get(id)?;
        for ext in exts {
            let bytes = extract_extent(data, ext, idat_offset)?;
            out.extend_from_slice(bytes);
        }
    }
    Some(out)
}

fn parse_pitm(data: &[u8], box_start: usize) -> Option<u32> {
    let version = *data.get(box_start + 8)?;
    let content = box_start + 12;
    match version {
        0 => read_u16(data, content).map(u32::from),
        _ => read_u32(data, content),
    }
}

fn parse_iloc(data: &[u8], box_start: usize) -> Option<HashMap<u32, Vec<Extent>>> {
    let version = *data.get(box_start + 8)?;
    let sizes = *data.get(box_start + 12)?;
    let offset_size = (sizes >> 4) & 0x0F;
    let length_size = sizes & 0x0F;
    let base_index = *data.get(box_start + 13)?;
    let base_offset_size = (base_index >> 4) & 0x0F;
    let index_size = base_index & 0x0F;

    let mut pos = box_start + 14;
    let item_count = if version < 2 {
        read_u16(data, pos)? as u32
    } else {
        read_u32(data, pos)?
    };
    pos += if version < 2 { 2 } else { 4 };

    let mut map = HashMap::new();
    for _ in 0..item_count {
        let item_id = if version < 2 {
            read_u16(data, pos)? as u32
        } else {
            read_u32(data, pos)?
        };
        pos += if version < 2 { 2 } else { 4 };

        let mut construction_method = 0u8;
        if version == 1 || version == 2 {
            let cm = read_u16(data, pos)?;
            construction_method = (cm & 0x0F) as u8;
            pos += 2;
        }
        pos += 2;
        let base_offset = read_uint(data, pos, base_offset_size)?;
        pos += base_offset_size as usize;

        let extent_count = read_u16(data, pos)? as usize;
        pos += 2;

        let mut exts = Vec::with_capacity(extent_count);
        for _ in 0..extent_count {
            if (version == 1 || version == 2) && index_size > 0 {
                pos += index_size as usize;
            }
            let extent_offset = read_uint(data, pos, offset_size)?;
            pos += offset_size as usize;
            let extent_length = read_uint(data, pos, length_size)?;
            pos += length_size as usize;

            exts.push(Extent {
                construction_method,
                offset: base_offset + extent_offset,
                length: extent_length,
            });
        }
        map.insert(item_id, exts);
    }
    Some(map)
}

fn parse_dimg_refs(data: &[u8], box_start: usize, primary_id: u32) -> Vec<u32> {
    let version = *data.get(box_start + 8).unwrap_or(&0);
    let id_size = if version == 0 { 2usize } else { 4usize };
    let box_end = box_start + read_u32(data, box_start).unwrap_or(0) as usize;

    let mut pos = box_start + 12;
    while pos + 8 <= box_end {
        let size = match read_u32(data, pos) {
            Some(s) => s as usize,
            None => break,
        };
        let typ = match data.get(pos + 4..pos + 8) {
            Some(t) => t,
            None => break,
        };
        let from_id = if id_size == 2 {
            read_u16(data, pos + 8).unwrap_or(0) as u32
        } else {
            read_u32(data, pos + 8).unwrap_or(0)
        };
        let count = read_u16(data, pos + 8 + id_size).unwrap_or(0) as usize;

        if typ == b"dimg" && from_id == primary_id {
            let refs_start = pos + 10 + id_size;
            let mut refs = Vec::with_capacity(count);
            for i in 0..count {
                let off = refs_start + i * id_size;
                let to_id = if id_size == 2 {
                    read_u16(data, off).unwrap_or(0) as u32
                } else {
                    read_u32(data, off).unwrap_or(0)
                };
                refs.push(to_id);
            }
            return refs;
        }
        pos += size;
    }
    Vec::new()
}

fn extract_extent<'a>(data: &'a [u8], ext: &Extent, idat_offset: Option<usize>) -> Option<&'a [u8]> {
    let base = match ext.construction_method {
        0 => 0,
        1 => idat_offset?,
        _ => return None,
    };
    let start = base + ext.offset as usize;
    let end = start + ext.length as usize;
    data.get(start..end)
}
