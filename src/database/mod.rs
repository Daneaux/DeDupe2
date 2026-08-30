#![allow(dead_code, unused_variables)]

use std::fmt;
use std::path::{Path, PathBuf};

use crate::volumes::{FileType, Volume};

#[derive(Debug)]
pub enum DbError {
    Open(String),
    Write(String),
    Query(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Open(msg) => write!(f, "failed to open database: {msg}"),
            DbError::Write(msg) => write!(f, "failed to write to database: {msg}"),
            DbError::Query(msg) => write!(f, "database query failed: {msg}"),
        }
    }
}

impl std::error::Error for DbError {}

#[derive(Debug)]
pub struct Database;

impl Database {
    pub fn open(path: &Path) -> Result<Self, DbError> {
        todo!("open database at {}", path.display())
    }

    pub fn insert_file(
        &self,
        path: &Path,
        size: u64,
        modified: i64,
        file_type: &FileType,
        hash: &str,
    ) -> Result<(), DbError> {
        todo!("insert file {}", path.display())
    }

    pub fn remove_file(&self, path: &Path) -> Result<(), DbError> {
        todo!("remove file {}", path.display())
    }

    pub fn find_by_hash(&self, hash: &str) -> Result<Vec<PathBuf>, DbError> {
        todo!("find files with hash {hash}")
    }

    pub fn find_duplicates(&self) -> Result<Vec<Vec<PathBuf>>, DbError> {
        todo!("find duplicate file groups")
    }

    pub fn add_volume(&self, volume: &Volume) -> Result<(), DbError> {
        todo!("store volume {}", volume.path.display())
    }

    pub fn remove_volume(&self, path: &Path) -> Result<(), DbError> {
        todo!("remove volume {}", path.display())
    }

    pub fn list_volumes(&self) -> Result<Vec<Volume>, DbError> {
        todo!("list volumes")
    }

    pub fn add_file_type(&self, ext: &str) -> Result<(), DbError> {
        todo!("store file type {ext}")
    }

    pub fn remove_file_type(&self, ext: &str) -> Result<(), DbError> {
        todo!("remove file type {ext}")
    }

    pub fn list_file_types(&self) -> Result<Vec<FileType>, DbError> {
        todo!("list file types")
    }

    pub fn clear(&self) -> Result<(), DbError> {
        todo!("clear database")
    }
}
