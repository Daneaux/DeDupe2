use rawler::formats::ciff::{CiffIFD, CiffTag};
use rawler::rawsource::RawSource;

pub fn raw_data(data: &[u8]) -> Option<Vec<u8>> {
    let src = RawSource::new_from_slice(data);
    let ciff = CiffIFD::new_file(&src).ok()?;

    let sensor = ciff.find_entry(CiffTag::SensorInfo)?;
    let width = sensor.get_usize(1);
    let height = sensor.get_usize(2);

    let offset = 540 + width * height / 4;
    let raw = src.subview_until_eof(offset as u64).ok()?;

    let end = raw
        .windows(2)
        .position(|w| w == [0xFF, 0xD9])
        .map(|p| p + 2)
        .unwrap_or(raw.len());

    Some(raw[..end].to_vec())
}
