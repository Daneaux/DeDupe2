use std::fmt;

#[derive(Debug)]
pub enum VolumeError {
    NotFound(String),
    AlreadyExists(String),
}

impl fmt::Display for VolumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VolumeError::NotFound(what) => write!(f, "not found: {what}"),
            VolumeError::AlreadyExists(what) => write!(f, "already exists: {what}"),
        }
    }
}

impl std::error::Error for VolumeError {}
