fn read_u32(data: &[u8], pos: usize) -> Option<u32> {
    let b = data.get(pos..pos + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64(data: &[u8], pos: usize) -> Option<u64> {
    let b = data.get(pos..pos + 8)?;
    Some(u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

fn read_box_header(data: &[u8], pos: usize) -> Option<(usize, [u8; 4])> {
    let size = read_u32(data, pos)? as usize;
    let mut typ = [0u8; 4];
    typ.copy_from_slice(data.get(pos + 4..pos + 8)?);

    let (hdr, size) = match size {
        0 => return None,
        1 => (16, read_u64(data, pos + 8)? as usize),
        _ => (8, size),
    };

    if size < hdr {
        return None;
    }
    Some((size, typ))
}

fn find_child(data: &[u8], start: usize, end: usize, want: &[u8; 4]) -> Option<(usize, usize)> {
    let mut pos = start;
    while pos + 8 <= end {
        let (size, typ) = read_box_header(data, pos)?;
        if &typ == want {
            return Some((pos, size));
        }
        pos += size;
    }
    None
}

fn largest_sample(data: &[u8], trak_start: usize, trak_end: usize) -> Option<(usize, usize)> {
    let (mdia_start, mdia_size) = find_child(data, trak_start + 8, trak_end, b"mdia")?;
    let mdia_end = mdia_start + mdia_size;
    let (minf_start, minf_size) = find_child(data, mdia_start + 8, mdia_end, b"minf")?;
    let minf_end = minf_start + minf_size;
    let (stbl_start, stbl_size) = find_child(data, minf_start + 8, minf_end, b"stbl")?;
    let stbl_end = stbl_start + stbl_size;

    let mut sample_sizes: Vec<usize> = Vec::new();
    let mut chunk_offsets: Vec<u64> = Vec::new();
    let mut stsc: Vec<(u32, u32)> = Vec::new();

    let mut pos = stbl_start + 8;
    while pos + 8 <= stbl_end {
        let (size, typ) = read_box_header(data, pos)?;
        match &typ {
            b"stsz" => {
                let default = read_u32(data, pos + 12)? as usize;
                let count = read_u32(data, pos + 16)? as usize;
                if default != 0 {
                    sample_sizes = vec![default; count];
                } else {
                    sample_sizes = (0..count)
                        .filter_map(|i| read_u32(data, pos + 20 + i * 4).map(|v| v as usize))
                        .collect();
                }
            }
            b"stco" => {
                let count = read_u32(data, pos + 12)? as usize;
                chunk_offsets = (0..count)
                    .filter_map(|i| read_u32(data, pos + 16 + i * 4).map(u64::from))
                    .collect();
            }
            b"co64" => {
                let count = read_u32(data, pos + 12)? as usize;
                chunk_offsets = (0..count).filter_map(|i| read_u64(data, pos + 16 + i * 8)).collect();
            }
            b"stsc" => {
                let count = read_u32(data, pos + 12)? as usize;
                stsc = (0..count)
                    .filter_map(|i| {
                        let base = pos + 16 + i * 12;
                        Some((read_u32(data, base)?, read_u32(data, base + 4)?))
                    })
                    .collect();
            }
            _ => {}
        }
        pos += size;
    }

    if sample_sizes.is_empty() {
        return None;
    }

    let mut sample_to_chunk = Vec::with_capacity(sample_sizes.len());
    if !stsc.is_empty() {
        let total_chunks = chunk_offsets.len().max(1);
        let mut samples_per_chunk = vec![0usize; total_chunks];
        for (i, &(first_chunk, spc)) in stsc.iter().enumerate() {
            let next_first = stsc
                .get(i + 1)
                .map(|(nf, _)| *nf)
                .unwrap_or((total_chunks as u32) + 1);
            for chunk in (first_chunk as usize - 1)..(next_first as usize - 1).min(total_chunks) {
                samples_per_chunk[chunk] = spc as usize;
            }
        }

        let mut acc = 0usize;
        for (chunk, &spc) in samples_per_chunk.iter().enumerate() {
            for s in 0..spc {
                if acc + s < sample_sizes.len() {
                    sample_to_chunk.push(chunk);
                }
            }
            acc += spc;
        }
    }

    let mut best: Option<(usize, usize, usize)> = None;
    let mut chunk_running = 0usize;
    let mut cur_chunk = usize::MAX;
    for (sample, &sz) in sample_sizes.iter().enumerate() {
        let chunk = sample_to_chunk.get(sample).copied().unwrap_or(0);
        if chunk != cur_chunk {
            cur_chunk = chunk;
            chunk_running = 0;
        }
        let offset = *chunk_offsets.get(chunk)? as usize + chunk_running;
        chunk_running += sz;

        if best.as_ref().map_or(true, |(bs, _, _)| sz > *bs) {
            best = Some((sz, offset, sz));
        }
    }

    best.map(|(_, offset, size)| (offset, size))
}

pub fn raw_data(data: &[u8]) -> Option<Vec<u8>> {
    let (moov_start, moov_size) = find_child(data, 0, data.len(), b"moov")?;
    let moov_end = moov_start + moov_size;

    let mut best: Option<(usize, Vec<u8>)> = None;

    let mut pos = moov_start + 8;
    while pos + 8 <= moov_end {
        let (size, typ) = read_box_header(data, pos)?;
        if &typ == b"trak" {
            if let Some((offset, len)) = largest_sample(data, pos, pos + size) {
                let bytes = data.get(offset..offset + len)?.to_vec();
                if best.as_ref().map_or(true, |(bl, _)| len > *bl) {
                    best = Some((len, bytes));
                }
            }
        }
        pos += size;
    }

    best.map(|(_, bytes)| bytes)
}
