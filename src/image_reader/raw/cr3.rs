use super::super::isobmff::extract_primary_image_data;

pub fn raw_data(data: &[u8]) -> Option<Vec<u8>> {
    extract_primary_image_data(data)
}
