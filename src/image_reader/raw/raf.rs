pub fn raw_data(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 108 || &data[0..16] != b"FUJIFILMCCD-RAW " {
        return None;
    }

    let offset = read_be_u32(data, 100)? as usize;
    let length = read_be_u32(data, 104)? as usize;
    let end = if offset.saturating_add(length) <= data.len() {
        offset + length
    } else {
        data.len()
    };
    data.get(offset..end).map(|s| s.to_vec())
}

fn read_be_u32(data: &[u8], pos: usize) -> Option<u32> {
    let b = data.get(pos..pos + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
