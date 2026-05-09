//! User-visible configuration. Persisted to `<APPDATA>/TriRecover/config.json`.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// UI theme preference.
    pub theme: Theme,
    /// UI language code (ISO 639-1, e.g. "en", "ar").
    pub language: String,
    /// Telemetry opt-in. Off by default.
    pub telemetry_enabled: bool,
    /// Auto-update channel.
    pub update_channel: UpdateChannel,
    /// Where recovered files default to.
    pub default_destination: Option<PathBuf>,
    /// Maximum simultaneous worker threads for carving (0 = auto).
    pub worker_threads: u16,
    /// How often (ms) the engine flushes progress to disk.
    pub progress_flush_interval_ms: u64,
    /// Maximum candidate buffer size when carving.
    pub carve_window_bytes: u64,
    /// If true, the engine retries bad sectors with shrinking I/O size.
    pub bad_sector_retry: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            language: "en".into(),
            telemetry_enabled: false,
            update_channel: UpdateChannel::Stable,
            default_destination: None,
            worker_threads: 0,
            progress_flush_interval_ms: 5_000,
            carve_window_bytes: 64 * 1024 * 1024,
            bad_sector_retry: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    Dark,
    Light,
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateChannel {
    Stable,
    Beta,
    None,
}

impl Config {
    /// Resolve the on-disk config path. Creates parent directories on demand.
    pub fn resolve_path() -> Result<PathBuf> {
        let base = appdata_dir().ok_or_else(|| {
            Error::Config("could not resolve APPDATA / config base directory".into())
        })?;
        let dir = base.join(crate::PRODUCT_NAME);
        fs::create_dir_all(&dir).map_err(|_| Error::ConfigPath(dir.clone()))?;
        Ok(dir.join("config.json"))
    }

    pub fn load_or_default() -> Self {
        match Self::resolve_path() {
            Ok(path) if path.exists() => match fs::read_to_string(&path) {
                Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
                Err(_) => Self::default(),
            },
            _ => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::resolve_path()?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("serialize: {e}")))?;
        // atomic-ish: write to .tmp then rename
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Number of worker threads to actually use, expanding 0 → CPU count.
    #[must_use]
    pub fn effective_workers(&self) -> usize {
        if self.worker_threads == 0 {
            std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(4)
        } else {
            self.worker_threads as usize
        }
    }
}

/// Resolve the platform-specific user data directory.
fn appdata_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
}

/// Returns the directory under which scan-session SQLite files live.
pub fn sessions_dir() -> Result<PathBuf> {
    let base = appdata_dir()
        .ok_or_else(|| Error::Config("no APPDATA-equivalent directory".into()))?
        .join(crate::PRODUCT_NAME)
        .join("sessions");
    fs::create_dir_all(&base).map_err(|_| Error::ConfigPath(base.clone()))?;
    Ok(base)
}

/// Returns the directory where rolling log files are written.
pub fn logs_dir() -> Result<PathBuf> {
    let base = appdata_dir()
        .ok_or_else(|| Error::Config("no APPDATA-equivalent directory".into()))?
        .join(crate::PRODUCT_NAME)
        .join("logs");
    fs::create_dir_all(&base).map_err(|_| Error::ConfigPath(base.clone()))?;
    Ok(base)
}

/// Test helper: load/save configs to a sandbox directory.
pub fn save_to(path: &Path, c: &Config) -> Result<()> {
    let json = serde_json::to_string_pretty(c)
        .map_err(|e| Error::Config(format!("serialize: {e}")))?;
    fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip() {
        let d = tempdir().unwrap();
        let p = d.path().join("config.json");
        let c = Config {
            theme: Theme::Light,
            language: "ar".into(),
            telemetry_enabled: true,
            ..Config::default()
        };
        save_to(&p, &c).unwrap();
        let s = fs::read_to_string(&p).unwrap();
        let r: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(r.language, "ar");
        assert_eq!(r.theme, Theme::Light);
        assert!(r.telemetry_enabled);
    }

    #[test]
    fn effective_workers_nonzero() {
        let c = Config::default();
        assert!(c.effective_workers() >= 1);
    }
}
