//! Per-layer reads and writes (shallow, full bytes/content, pixels,
//! perceptual hashes, dates), and the `*_cached` helpers that compute a
//! layer once and store it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::exif::CreationDate;
use crate::image_reader::{self, ImageData, ImageReaderError, Phash, PixelData};

use super::*;

impl ScanCache {
    /// 64kb encoded-image-data hash (with decoded flag), cached.
    pub fn shallow_hash(&mut self, path: &Path, n: usize) -> (u64, bool) {
        if let Some(v) = self.shallow_hash_lookup(path) {
            return v;
        }
        let v = image_reader::hash_image_data_status(path, n);
        self.shallow_hash_store(path, v);
        v
    }

    /// Non-computing shallow lookup (still applies the stat-based reset).
    pub fn shallow_hash_lookup(&mut self, path: &Path) -> Option<(u64, bool)> {
        self.with_entry(path, |e| e.shallow, |_e, _v| {}, || None)
    }

    pub fn shallow_hash_store(&mut self, path: &Path, value: (u64, bool)) {
        self.store_layer(
            path,
            value,
            |entry| &mut entry.shallow,
            |entry, v| entry.shallow = Some(v),
        );
    }

    /// Whole-file byte hash, cached on success.
    pub fn full_bytes_hash(&mut self, path: &Path) -> Result<u64, ImageReaderError> {
        if let Some(v) = self.full_bytes_lookup(path) {
            return Ok(v);
        }
        let v = image_reader::hash_all_bytes(path)?;
        self.full_bytes_store(path, v);
        Ok(v)
    }

    pub fn full_bytes_lookup(&mut self, path: &Path) -> Option<u64> {
        self.with_entry(path, |e| e.full_bytes, |_e, _v| {}, || None)
    }

    pub fn full_bytes_store(&mut self, path: &Path, value: u64) {
        self.store_layer(
            path,
            value,
            |entry| &mut entry.full_bytes,
            |entry, v| entry.full_bytes = Some(v),
        );
    }

    /// Full encoded-image-data hash (deep scan step 2), cached on success.
    pub fn full_content_hash(&mut self, path: &Path) -> Result<u64, ImageReaderError> {
        if let Some(v) = self.full_content_lookup(path) {
            return Ok(v);
        }
        let v = image_reader::hash_image_data_all(path)?;
        self.full_content_store(path, v);
        Ok(v)
    }

    pub fn full_content_lookup(&mut self, path: &Path) -> Option<u64> {
        self.with_entry(path, |e| e.full_content, |_e, _v| {}, || None)
    }

    pub fn full_content_store(&mut self, path: &Path, value: u64) {
        self.store_layer(
            path,
            value,
            |entry| &mut entry.full_content,
            |entry, v| entry.full_content = Some(v),
        );
    }

    /// Decoded-pixel key (deep scan step 3). Decode failures are not cached.
    pub fn pixel_key(&mut self, path: &Path) -> Option<PixelKey> {
        if let Some(v) = self.pixel_lookup(path) {
            return Some(v);
        }
        let v = pixel_key_of(path)?;
        self.pixel_store(path, v);
        Some(v)
    }

    pub fn pixel_lookup(&mut self, path: &Path) -> Option<PixelKey> {
        self.with_entry(path, |e| e.pixel, |_e, _v| {}, || None)
    }

    pub fn pixel_store(&mut self, path: &Path, value: PixelKey) {
        self.store_layer(
            path,
            value,
            |entry| &mut entry.pixel,
            |entry, v| entry.pixel = Some(v),
        );
    }

    /// Perceptual hash, cached on success.
    pub fn phash(&mut self, path: &Path) -> Option<Phash> {
        if let Some(v) = self.phash_lookup(path) {
            return Some(v);
        }
        let v = image_reader::phash(path)?;
        self.phash_store(path, v);
        Some(v)
    }

    pub fn phash_lookup(&mut self, path: &Path) -> Option<Phash> {
        self.with_entry(path, |e| e.phash, |_e, _v| {}, || None)
    }

    pub fn phash_store(&mut self, path: &Path, value: Phash) {
        self.store_layer(
            path,
            value,
            |entry| &mut entry.phash,
            |entry, v| entry.phash = Some(v),
        );
    }

    /// Mean chroma, cached on success.
    pub fn chroma(&mut self, path: &Path) -> Option<u8> {
        if let Some(v) = self.chroma_lookup(path) {
            return Some(v);
        }
        let image = image_reader::read_image(path).ok()?;
        let v = image_reader::mean_chroma(&image)?;
        self.chroma_store(path, v);
        Some(v)
    }

    pub fn chroma_lookup(&mut self, path: &Path) -> Option<u8> {
        self.with_entry(path, |e| e.chroma, |_e, _v| {}, || None)
    }

    pub fn chroma_store(&mut self, path: &Path, value: u8) {
        self.store_layer(
            path,
            value,
            |entry| &mut entry.chroma,
            |entry, v| entry.chroma = Some(v),
        );
    }

    pub fn dhash_lookup(&mut self, path: &Path) -> Option<u64> {
        self.with_entry(path, |e| e.dhash, |_e, _v| {}, || None)
    }

    pub fn dhash_store(&mut self, path: &Path, value: u64) {
        self.store_layer(
            path,
            value,
            |entry| &mut entry.dhash,
            |entry, v| entry.dhash = Some(v),
        );
    }

    /// Cached creation date (a cached `Unknown` is a real answer, so date
    /// presence itself is the cache hit).
    pub fn creation_date(&mut self, path: &Path) -> Option<CreationDate> {
        self.with_entry(path, |e| e.date.clone(), |_e, _v| {}, || None)
    }

    /// Store a value into one layer, counting a first-time store as computed.
    pub(super) fn store_layer<T: Clone>(
        &mut self,
        path: &Path,
        value: T,
        layer: impl FnOnce(&mut FileEntry) -> &mut Option<T>,
        set: impl FnOnce(&mut FileEntry, T),
    ) {
        // Never (re-)create an entry for a file that no longer exists.
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(_) => return,
        };
        let size = meta.len();
        let mtime = meta.modified().ok();
        let dev = device_of(&meta);
        let Some((name, dirs)) = self.key_components(path, dev) else {
            return;
        };
        self.dirty = true;
        let node = match Self::node_comps_mut(&mut self.root, &dirs) {
            Some(node) => node,
            None => return,
        };
        let entry = match node.files.entry(name) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                self.stats.files += 1;
                self.stats.computed += 1;
                slot.insert(FileEntry {
                    size,
                    mtime,
                    dev,
                    ..Default::default()
                })
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                if layer(slot.get_mut()).is_none() {
                    self.stats.computed += 1;
                }
                slot.into_mut()
            }
        };
        set(entry, value);
    }

    pub fn store_creation_date(&mut self, path: &Path, date: CreationDate) {
        self.store_layer(
            path,
            date,
            |entry| &mut entry.date,
            |entry, date| entry.date = Some(date),
        );
    }
}

pub(super) fn pixel_key_of(path: &Path) -> Option<PixelKey> {
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        image_reader::read_image(path)
    }));
    let image: ImageData = match decoded {
        Ok(Ok(image)) => image,
        _ => return None,
    };
    let hash = match &image.data {
        PixelData::U8(bytes) => seahash::hash(bytes),
        PixelData::U16(words) => {
            let mut hasher = seahash::SeaHasher::new();
            use std::hash::Hasher;
            for chunk in words.chunks(4096) {
                for &word in chunk {
                    hasher.write_u16(word);
                }
            }
            hasher.finish()
        }
    };
    Some(PixelKey {
        width: image.width,
        height: image.height,
        cpp: image.cpp,
        hash,
    })
}

/// Shallow-hash many files through one shared cache. The lock is only held
/// for the lookup and the store — the actual file hashing runs outside it, so
/// the pass stays parallel instead of serializing behind the cache.
pub fn shallow_hashes_cached(
    cache: &Mutex<ScanCache>,
    paths: &[PathBuf],
    n: usize,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<(u64, bool)> {
    let total = paths.len();
    let done = AtomicUsize::new(0);
    paths
        .par_iter()
        .map(|path| {
            let (value, _reused) = shallow_hash_cached(cache, path, n);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            value
        })
        .collect()
}

/// One shallow hash through the cache. The value is computed outside the
/// lock; `reused` reports whether it came from the cache.
pub fn shallow_hash_cached(cache: &Mutex<ScanCache>, path: &Path, n: usize) -> ((u64, bool), bool) {
    // NOTE: the lookup guard must be dropped before the store lock — a
    // `match lock(...)` scrutinee guard would live through the whole match
    // and deadlock the second lock.
    let cached = { lock(cache).shallow_hash_lookup(path) };
    match cached {
        Some(value) => (value, true),
        None => {
            let value = image_reader::hash_image_data_status(path, n);
            lock(cache).shallow_hash_store(path, value);
            (value, false)
        }
    }
}

/// Whole-file byte hash through the cache, computed outside the lock.
pub fn full_bytes_cached(cache: &Mutex<ScanCache>, path: &Path) -> Result<u64, ImageReaderError> {
    if let Some(value) = lock(cache).full_bytes_lookup(path) {
        return Ok(value);
    }
    let value = image_reader::hash_all_bytes(path)?;
    lock(cache).full_bytes_store(path, value);
    Ok(value)
}

/// Full encoded-image-data hash through the cache, computed outside the lock.
pub fn full_content_cached(cache: &Mutex<ScanCache>, path: &Path) -> Result<u64, ImageReaderError> {
    if let Some(value) = lock(cache).full_content_lookup(path) {
        return Ok(value);
    }
    let value = image_reader::hash_image_data_all(path)?;
    lock(cache).full_content_store(path, value);
    Ok(value)
}

/// Perceptual hash + chroma through the cache. Phash and chroma both need a
/// decode, so missing pieces are filled with a single decode pass (the old
/// phash value survives if only the chroma is new).
pub fn perceptual_cached(
    cache: &Mutex<ScanCache>,
    path: &Path,
) -> Option<(Phash, Option<u8>, Option<u64>)> {
    let (phash, chroma, dhash) = {
        let mut cache = lock(cache);
        (
            cache.phash_lookup(path),
            cache.chroma_lookup(path),
            cache.dhash_lookup(path),
        )
    };
    if let (Some(phash), Some(chroma), Some(dhash)) = (phash, chroma, dhash) {
        return Some((phash, Some(chroma), Some(dhash)));
    }

    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        image_reader::read_image(path)
    }));
    let image = match decoded {
        Ok(Ok(image)) => image,
        _ => return phash.map(|phash| (phash, chroma, dhash)),
    };
    let phash = match phash {
        Some(phash) => phash,
        None => {
            let phash = image_reader::phash_of(&image)?;
            lock(cache).phash_store(path, phash);
            phash
        }
    };
    let chroma = match chroma {
        Some(chroma) => Some(chroma),
        None => match image_reader::mean_chroma(&image) {
            Some(chroma) => {
                lock(cache).chroma_store(path, chroma);
                Some(chroma)
            }
            None => None,
        },
    };
    let dhash = match dhash {
        Some(dhash) => Some(dhash),
        None => match image_reader::gradient_hash_of(&image) {
            Some(dhash) => {
                lock(cache).dhash_store(path, dhash);
                Some(dhash)
            }
            None => None,
        },
    };
    Some((phash, chroma, dhash))
}

/// Perceptual hash through the cache, computed outside the lock.
pub fn cached_phash(cache: &Mutex<ScanCache>, path: &Path) -> Option<Phash> {
    if let Some(value) = lock(cache).phash_lookup(path) {
        return Some(value);
    }
    let value = image_reader::phash(path)?;
    lock(cache).phash_store(path, value);
    Some(value)
}

/// Decoded pixel key through the cache, computed outside the lock.
pub fn pixel_cached(cache: &Mutex<ScanCache>, path: &Path) -> Option<PixelKey> {
    if let Some(value) = lock(cache).pixel_lookup(path) {
        return Some(value);
    }
    let value = pixel_key_of(path)?;
    lock(cache).pixel_store(path, value);
    Some(value)
}

/// Date resolution through the cache: cached dates are served directly; the
/// misses go through the normal batched funnel (in-process readers → one
/// exiftool run → folder proxy) and are stored as they come back.
pub fn creation_dates_cached(
    cache: &Mutex<ScanCache>,
    paths: &[PathBuf],
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Vec<CreationDate> {
    let total = paths.len();
    let mut out: Vec<Option<CreationDate>> = Vec::with_capacity(total);
    let mut misses: Vec<(usize, PathBuf)> = Vec::new();
    {
        let mut cache = lock(cache);
        for (i, path) in paths.iter().enumerate() {
            match cache.creation_date(path) {
                Some(date) => {
                    out.push(Some(date));
                    progress(i + 1, total);
                }
                None => {
                    out.push(None);
                    misses.push((i, path.clone()));
                }
            }
        }
    }

    if !misses.is_empty() {
        let miss_paths: Vec<PathBuf> = misses.iter().map(|(_, p)| p.clone()).collect();
        let hits = total - misses.len();
        let computed = crate::exif::creation_dates_batch(&miss_paths, &|done, _total| {
            progress(hits + done, total);
        });
        let mut cache = lock(cache);
        for ((i, path), date) in misses.iter().zip(computed) {
            out[*i] = Some(date.clone());
            cache.store_creation_date(path, date);
        }
    }

    out.into_iter()
        .map(|date| date.unwrap_or(CreationDate::Unknown))
        .collect()
}
