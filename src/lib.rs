#[allow(non_snake_case)]
pub mod Scanner;
pub mod exif;
pub mod filemover;
pub mod image_reader;
pub mod web;

/// Progress callback shared by every long-running operation:
/// `progress(done, total)`. Called from worker threads.
pub type Progress<'a> = &'a (dyn Fn(usize, usize) + Send + Sync);
