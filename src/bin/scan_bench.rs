use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use dedupe2::Scanner::fastScan::fast_scan;
use dedupe2::Scanner::scanner::{deep_scan, shallow_scan, ScanTarget};
use dedupe2::volumes::Volume;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = match args.get(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: scan_bench <directory>");
            std::process::exit(2);
        }
    };
    let root = PathBuf::from(path);

    let extensions: Vec<String> = [
        "jpg", "jpeg", "png", "gif", "heic", "tiff", "cr2", "nef", "webp", "bmp",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let target = ScanTarget {
        name: root.display().to_string(),
        paths: vec![root.clone()],
        volume: Volume::new(root),
        extensions,
    };

    {  // prime
    let start = Instant::now();
    let _shallow = shallow_scan(&target);
    let _shallow_elapsed = start.elapsed();

    let start = Instant::now();
    let _deep = deep_scan(&target);
    let _deep_elapsed = start.elapsed();

    let start = Instant::now();
    let _fast = fast_scan(&target);
    let _fast_elapsed = start.elapsed();
    }

    let start = Instant::now();
    let shallow = shallow_scan(&target);
    let shallow_elapsed = start.elapsed();

    let start = Instant::now();
    let deep = deep_scan(&target);
    let deep_elapsed = start.elapsed();

    let start = Instant::now();
    let fast = fast_scan(&target);
    let fast_elapsed = start.elapsed();



    println!("target: {}", target.name);
    println!(
        "shallow (header, 64 KB):  {} files in {:?}",
        shallow.files.len(),
        shallow_elapsed
    );
    println!(
        "deep (full file):         {} files in {:?}",
        deep.files.len(),
        deep_elapsed
    );
    println!(
        "fast (rayon):            {} files in {:?}",
        fast.files.len(),
        fast_elapsed
    );

    let fast_by_path: HashMap<String, u64> = fast
        .files
        .iter()
        .map(|f| (f.path.display().to_string(), f.hash))
        .collect();
    let shallow_paths: HashSet<String> = shallow
        .files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();

    let mut verified = 0usize;
    let mut problems = Vec::new();

    for f in &shallow.files {
        let key = f.path.display().to_string();
        match fast_by_path.get(&key) {
            Some(hash) if *hash == f.hash => verified += 1,
            Some(_) => problems.push(format!("hash differs: {key}")),
            None => problems.push(format!("missing in fast scan: {key}")),
        }
    }

    for f in &fast.files {
        let key = f.path.display().to_string();
        if !shallow_paths.contains(&key) {
            problems.push(format!("missing in shallow scan: {key}"));
        }
    }

    if problems.is_empty() {
        println!("verification: fast scan == shallow scan ({verified} files)");
    } else {
        println!(
            "verification: {} problem(s) across {} shallow files",
            problems.len(),
            shallow.files.len()
        );
        for p in &problems {
            println!("  {p}");
        }
    }
}
