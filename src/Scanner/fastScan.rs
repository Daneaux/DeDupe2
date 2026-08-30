use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::Scanner::scanner::ScanTarget;

pub const HEADER_READ_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct PhotoMetadata {
    pub path: PathBuf,
    pub file_size: u64,
    pub modified_time: i64,
    pub header_hash: u64,
}

pub fn fast_scan(target: &ScanTarget) {
    let (tx, rx) = channel::<PhotoMetadata>();

    let db_handle = std::thread::spawn(move || {
        if let Err(e) = run_database_writer(rx) {
            eprintln!("Database writer error: {e}");
        }
    });

    println!("Stage 1: Crawling directory tree...");
    let mut files = Vec::new();
    for path in &target.paths {
        files.extend(collect_photo_files(path));
    }

    println!(
        "Found {} photos. Stage 2: Saturating all CPU cores...",
        files.len()
    );

    files
        .par_iter()
        .for_each_with(tx, |channel_sender, (path, size, mod_time)| {
            if *size == 0 {
                return;
            }

            if let Some(header_hash) = read_header_hash(path) {
                let _ = channel_sender.send(PhotoMetadata {
                    path: path.clone(),
                    file_size: *size,
                    modified_time: *mod_time,
                    header_hash,
                });
            }
        });

    println!("Scan complete. Finalizing database transactions...");
    db_handle.join().unwrap();
    println!("Index fully built for {}", target.name);
}

pub fn collect_photo_files(root: &Path) -> Vec<(PathBuf, u64, i64)> {
    const PHOTO_EXTENSIONS: [&str; 7] = ["jpg", "jpeg", "png", "heic", "tiff", "cr2", "nef"];

    WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let ext = e
                .path()
                .extension()
                .and_then(|os| os.to_str())
                .unwrap_or("")
                .to_lowercase();
            PHOTO_EXTENSIONS.contains(&ext.as_str())
        })
        .map(|e| {
            let meta = e.metadata().unwrap();
            let mod_time = meta
                .modified()
                .unwrap_or(SystemTime::now())
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            (e.into_path(), meta.len(), mod_time)
        })
        .collect()
}

pub fn read_header_hash(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let mut buffer = [0u8; HEADER_READ_BYTES];
    let bytes_read = file.read(&mut buffer).unwrap_or(0);
    Some(seahash::hash(&buffer[..bytes_read]))
}

fn run_database_writer(rx: Receiver<PhotoMetadata>) -> Result<(), Box<dyn std::error::Error>> {
    for _meta in rx {}
    Ok(())
}
