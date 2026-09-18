//! Application settings: a tiny JSON file remembered between runs. Only the
//! scan-cache location lives here (the cache itself is persisted separately
//! and can be large, this file must stay trivial).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    /// Where the scan cache is persisted. `None` uses the platform default.
    pub cache_path: Option<PathBuf>,
}

impl Settings {
    /// `$DEDUPE2_SETTINGS`, else the platform config directory.
    pub fn settings_path() -> PathBuf {
        if let Some(p) = std::env::var_os("DEDUPE2_SETTINGS") {
            return PathBuf::from(p);
        }
        let Some(home) = std::env::var_os("HOME") else {
            return std::env::temp_dir().join("dedupe2-settings.json");
        };
        let base = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            base.join("Library/Application Support/dedupe2/settings.json")
        } else {
            base.join(".config/dedupe2/settings.json")
        }
    }

    pub fn load() -> Settings {
        Self::load_from(&Self::settings_path())
    }

    pub fn load_from(path: &Path) -> Settings {
        match std::fs::read(path) {
            Ok(data) => serde_json::from_slice(&data).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::settings_path())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, path)
    }
}
