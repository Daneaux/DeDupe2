use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, IFD};
use rawler::rawsource::RawSource;
use rawler::tags::TiffCommonTag;

pub fn raw_data(data: &[u8]) -> Option<Vec<u8>> {
    let src = RawSource::new_from_slice(data);
    let mut reader = src.reader();
    let tiff = GenericTiffReader::new(&mut reader, 0, 0, None, &[]).ok()?;

    let mut ifds: Vec<&IFD> = Vec::new();
    for ifd in tiff.chains() {
        collect(ifd, &mut ifds);
    }

    let mut best: Option<Vec<u8>> = None;
    for ifd in ifds {
        if ifd.has_entry(TiffCommonTag::StripOffsets) {
            if let Some(bytes) = strip_bytes(&src, ifd) {
                track(&mut best, bytes);
            }
        }
        if ifd.has_entry(TiffCommonTag::TileOffsets) {
            if let Some(bytes) = tile_bytes(&src, ifd) {
                track(&mut best, bytes);
            }
        }
        if ifd.has_entry(TiffCommonTag::PanaOffsets) {
            if let Some(bytes) = panasonic_bytes(&src, ifd) {
                track(&mut best, bytes);
            }
        }
    }

    best
}

fn collect<'a>(ifd: &'a IFD, out: &mut Vec<&'a IFD>) {
    out.push(ifd);
    for subs in ifd.sub_ifds().values() {
        for s in subs {
            collect(s, out);
        }
    }
}

fn track(best: &mut Option<Vec<u8>>, bytes: Vec<u8>) {
    if best.as_ref().map_or(true, |b| bytes.len() > b.len()) {
        *best = Some(bytes);
    }
}

fn strip_bytes(src: &RawSource, ifd: &IFD) -> Option<Vec<u8>> {
    let (strips, cont) = ifd.strip_data(src).ok()?;
    Some(match cont {
        Some(c) => c.to_vec(),
        None => {
            let mut out = Vec::new();
            for s in strips {
                out.extend_from_slice(s);
            }
            out
        }
    })
}

fn tile_bytes(src: &RawSource, ifd: &IFD) -> Option<Vec<u8>> {
    let tiles = ifd.tile_data(src).ok()?;
    let mut out = Vec::new();
    for t in tiles {
        out.extend_from_slice(t);
    }
    Some(out)
}

fn panasonic_bytes(src: &RawSource, ifd: &IFD) -> Option<Vec<u8>> {
    let offset = ifd.get_entry(TiffCommonTag::PanaOffsets)?.value.force_usize(0);
    src.subview_until_eof(offset as u64).ok().map(|s| s.to_vec())
}
