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

    let target = ScanTarget {
        name: root.display().to_string(),
        paths: vec![root.clone()],
        volume: Volume::new(root),
    };

    let start = Instant::now();
    let shallow = shallow_scan(&target);
    let shallow_elapsed = start.elapsed();

    let start = Instant::now();
    let deep = deep_scan(&target);
    let deep_elapsed = start.elapsed();

    let start = Instant::now();
    fast_scan(&target);
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
    println!("fast (rayon, photos):     {:?}", fast_elapsed);
}
