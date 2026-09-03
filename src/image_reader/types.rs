use std::fmt;

#[derive(Debug, Clone)]
pub enum PixelData {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

#[derive(Debug, Clone)]
pub struct ImageData {
    pub width: usize,
    pub height: usize,
    pub cpp: usize,
    pub data: PixelData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadLimit {
    All,
    First(usize),
}

#[derive(Debug)]
pub struct ImageReaderError(String);

impl ImageReaderError {
    pub(crate) fn new(msg: impl Into<String>) -> Self {
        ImageReaderError(msg.into())
    }
}

impl fmt::Display for ImageReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to read image: {}", self.0)
    }
}

impl std::error::Error for ImageReaderError {}
