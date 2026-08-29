use rayon::prelude::*;
use std::fs::File;
use std::io::Read;
use std::sync::mpsc::channel;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

pub fn fastScan() {
    let target_volume = "/Volumes/UltraFastNVMe"; // Use "D:\\" on Windows
    let db_output = "photo_index.db";

    // 1. Create a cross-thread message pipeline to the database thread
    let (tx, rx) = channel::<PhotoMetadata>();

    // 2. Spin up our dedicated database thread in the background
    let db_handle = std::thread::spawn(move || {
        if let Err(e) = run_database_writer(rx, db_output) {
            eprintln!("Database writer error: {}", e);
        }
    });

    println!("Stage 1: Crawling directory tree...");
    
    // Rapidly collect paths. This keeps disk operations purely sequential initially.
    let photo_extensions = ["jpg", "jpeg", "png", "heic", "tiff", "cr2", "nef"];
    let files: Vec<(std::path::PathBuf, u64, i64)> = WalkDir::new(target_volume)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let ext = e.path().extension().and_then(|os| os.to_str()).unwrap_or("").to_lowercase();
            photo_extensions.contains(&ext.as_str())
        })
        .map(|e| {
            let meta = e.metadata().unwrap();
            let mod_time = meta.modified()
                .unwrap_or(SystemTime::now())
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            (e.into_path(), meta.len(), mod_time)
        })
        .collect();

    println!("Found {} photos. Stage 2: Saturating all CPU cores...", files.len());

    // 3. Parallel worker loop running across all cores simultaneously
    files.par_iter().for_each_with(tx, |channel_sender, (path, size, mod_time)| {
        if *size == 0 { return; }

        // Open the file handler and extract the lightweight header buffer
        if let Ok(mut file) = File::open(path) {
            let mut buffer = [0; 16384]; // 16 KB optimization block
            let bytes_read = file.read(&mut buffer).unwrap_or(0);

            // Compute an ultra-fast non-cryptographic hash over the header
            let header_hash = seahash::hash(&buffer[..bytes_read]);

            // Ship the completed metadata packet down to the database worker
            let _ = channel_sender.send(PhotoMetadata {
                path: path.clone(),
                file_size: *size,
                modified_time: *mod_time,
                header_hash,
            });
        }
    });

    // Dropping our original senders signals to the database thread that processing is complete
    println!("Scan complete. Finalizing database transactions...");
    db_handle.join().unwrap();
    println!("Index fully built in {}", db_output);
}
