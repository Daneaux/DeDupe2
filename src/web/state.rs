use std::path::PathBuf;

use super::settings;

/// Server state: the hierarchical scan cache is shared by every request for
/// the lifetime of the process, so repeated scans reuse what earlier passes
/// already computed.
#[derive(Clone, Default)]
pub struct AppState {
    pub cache: std::sync::Arc<std::sync::Mutex<crate::scanner::cache::ScanCache>>,
    /// Where the cache is persisted; `None` keeps everything in memory
    /// (tests and throwaway runs). Changeable from the Settings tab.
    pub cache_path: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
    /// Where the settings file lives; `None` skips writing it (tests).
    pub settings_path: Option<PathBuf>,
    /// Guards against overlapping autosaves (a save can take a while on a
    /// large cache; stacking them would starve the scans behind the lock).
    persisting: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    /// In-memory state for tests.
    pub fn in_memory() -> AppState {
        AppState::default()
    }

    /// Production state: settings (then environment, then platform default)
    /// pick the cache location, which is loaded before serving.
    pub fn persistent() -> AppState {
        let settings = settings::Settings::load();
        let path = resolve_cache_path(&settings);
        let cache = crate::scanner::cache::ScanCache::load(&path);
        AppState {
            cache: std::sync::Arc::new(std::sync::Mutex::new(cache)),
            cache_path: std::sync::Arc::new(std::sync::Mutex::new(Some(path))),
            settings_path: Some(settings::Settings::settings_path()),
            ..AppState::default()
        }
    }

    pub fn cache_path(&self) -> Option<PathBuf> {
        self.cache_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Save when there is something new to save. Cheap no-op otherwise, and
    /// never runs two saves at once.
    pub fn persist(&self) {
        use std::sync::atomic::Ordering;

        let Some(path) = self.cache_path() else {
            return;
        };
        if self.persisting.swap(true, Ordering::SeqCst) {
            return; // a save is already in flight
        }
        let result = {
            let mut cache = crate::scanner::cache::lock(&self.cache);
            if cache.dirty() {
                cache.save(&path)
            } else {
                Ok(())
            }
        };
        self.persisting.store(false, Ordering::SeqCst);
        if let Err(e) = result {
            tracing::warn!("failed to persist scan cache to {}: {e}", path.display());
        }
    }

    /// Move the cache to a new location: the current (live) cache is written
    /// there and all future saves go to the new path.
    pub fn switch_cache_path(&self, new_path: PathBuf) -> std::io::Result<()> {
        let mut cache = crate::scanner::cache::lock(&self.cache);
        cache.save(&new_path)?;
        *self
            .cache_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(new_path);
        Ok(())
    }
}

/// Resolution order: `$DEDUPE2_CACHE` (for scripts/tests) beats the settings
/// file, which beats the platform default.
fn resolve_cache_path(settings: &settings::Settings) -> PathBuf {
    if let Some(p) = std::env::var_os("DEDUPE2_CACHE") {
        return PathBuf::from(p);
    }
    settings
        .cache_path
        .clone()
        .unwrap_or_else(crate::scanner::cache::default_cache_path)
}
