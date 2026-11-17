use std::fmt;

use super::safe_fs::TransferError;

#[derive(Debug)]
pub enum FileMoverError {
    Io(std::io::Error),
    Transfer(TransferError),
    Other(String),
}

impl fmt::Display for FileMoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Transfer(e) => write!(f, "{e}"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for FileMoverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Transfer(e) => Some(e),
            Self::Other(_) => None,
        }
    }
}

impl From<std::io::Error> for FileMoverError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<TransferError> for FileMoverError {
    fn from(e: TransferError) -> Self {
        Self::Transfer(e)
    }
}

impl From<walkdir::Error> for FileMoverError {
    fn from(e: walkdir::Error) -> Self {
        Self::Other(e.to_string())
    }
}

impl From<crate::image_reader::ImageReaderError> for FileMoverError {
    fn from(e: crate::image_reader::ImageReaderError) -> Self {
        Self::Other(e.to_string())
    }
}
