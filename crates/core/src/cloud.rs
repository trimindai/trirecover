//! Cloud storage destination detection and resolution.
//!
//! TriRecover supports saving recovered files to cloud-synced folders. The
//! primary approach is **sync-folder detection**: we find the local sync
//! directory (e.g. `C:\Users\<name>\OneDrive`) and write there like any local
//! path. The cloud client handles the upload transparently.
//!
//! Supported providers:
//! - **OneDrive** (Personal + Business)
//! - **Google Drive** (Desktop app sync folder)
//! - **Dropbox**
//!
//! Direct API upload (Microsoft Graph, Google Drive API) is scaffolded as
//! [`CloudUploader`] for future implementation — see `docs/architecture.md` §12.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing;

// ---- Types ------------------------------------------------------------------

/// A cloud storage provider supported as a recovery destination.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum CloudProvider {
    OneDrivePersonal,
    OneDriveBusiness,
    GoogleDrive,
    Dropbox,
}

impl CloudProvider {
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::OneDrivePersonal => "OneDrive",
            Self::OneDriveBusiness => "OneDrive for Business",
            Self::GoogleDrive => "Google Drive",
            Self::Dropbox => "Dropbox",
        }
    }

    #[must_use]
    pub fn icon_name(self) -> &'static str {
        match self {
            Self::OneDrivePersonal | Self::OneDriveBusiness => "onedrive",
            Self::GoogleDrive => "google-drive",
            Self::Dropbox => "dropbox",
        }
    }
}

/// A detected cloud sync folder on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudDestination {
    pub provider: CloudProvider,
    /// Human-readable label (e.g. "OneDrive - John", "Dropbox").
    pub label: String,
    /// The local sync folder path.
    pub local_path: PathBuf,
    /// Free space in the local sync folder (if determinable).
    pub free_bytes: Option<u64>,
    /// Whether the sync client appears to be running.
    pub sync_active: bool,
}

impl CloudDestination {
    /// Resolve the actual write path: `<sync_folder>/TriRecover Recovery/<subfolder>`.
    #[must_use]
    pub fn recovery_dir(&self) -> PathBuf {
        self.local_path.join("TriRecover Recovery")
    }

    /// Ensure the recovery subdirectory exists.
    pub fn ensure_recovery_dir(&self) -> crate::Result<PathBuf> {
        let dir = self.recovery_dir();
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

// ---- Detection --------------------------------------------------------------

/// Detect all available cloud sync folders on this machine.
/// Returns an empty vec if none are found (never errors — this is best-effort).
#[must_use]
pub fn detect_all() -> Vec<CloudDestination> {
    let mut out = Vec::new();
    out.extend(detect_onedrive());
    out.extend(detect_google_drive());
    out.extend(detect_dropbox());
    out
}

/// Detect OneDrive sync folders.
///
/// Windows detection strategy (in priority order):
/// 1. Registry: `HKCU\Software\Microsoft\OneDrive\Accounts\*\UserFolder`
/// 2. Environment: `%OneDrive%`, `%OneDriveConsumer%`, `%OneDriveCommercial%`
/// 3. Known path: `%USERPROFILE%\OneDrive`
///
/// Linux (dev fallback): `$HOME/OneDrive` (onedrive-fuse, rclone mount).
#[must_use]
pub fn detect_onedrive() -> Vec<CloudDestination> {
    let mut found = Vec::new();

    // Strategy 1: Windows registry (most reliable)
    #[cfg(windows)]
    {
        found.extend(detect_onedrive_registry());
    }

    // Strategy 2: environment variables
    for (var, provider) in [
        ("OneDrive", CloudProvider::OneDrivePersonal),
        ("OneDriveConsumer", CloudProvider::OneDrivePersonal),
        ("OneDriveCommercial", CloudProvider::OneDriveBusiness),
    ] {
        if let Ok(p) = std::env::var(var) {
            let path = PathBuf::from(&p);
            if path.is_dir() && !already_found(&found, &path) {
                found.push(make_destination(provider, path, None));
            }
        }
    }

    // Strategy 3: well-known paths
    if let Some(home) = home_dir() {
        for name in ["OneDrive", "OneDrive - Personal"] {
            let path = home.join(name);
            if path.is_dir() && !already_found(&found, &path) {
                found.push(make_destination(
                    CloudProvider::OneDrivePersonal,
                    path,
                    None,
                ));
            }
        }
    }

    found
}

/// Detect Google Drive Desktop sync folder.
///
/// Windows: Registry `HKCU\Software\Google\DriveFS\PerAccountPreferences`
///          or default mount `G:\My Drive` / `%USERPROFILE%\Google Drive`.
/// macOS: `$HOME/Google Drive/My Drive`
/// Linux: `$HOME/google-drive` (rclone) or `$HOME/Google Drive`.
#[must_use]
pub fn detect_google_drive() -> Vec<CloudDestination> {
    let mut found = Vec::new();

    if let Some(home) = home_dir() {
        for name in ["Google Drive", "google-drive"] {
            let path = home.join(name);
            if path.is_dir() && !already_found(&found, &path) {
                found.push(make_destination(
                    CloudProvider::GoogleDrive,
                    path,
                    None,
                ));
            }
            // Google Drive Desktop creates "My Drive" inside the root
            let my_drive = home.join(name).join("My Drive");
            if my_drive.is_dir() && !already_found(&found, &my_drive) {
                found.push(make_destination(
                    CloudProvider::GoogleDrive,
                    my_drive,
                    None,
                ));
            }
        }
    }

    // Windows: check drive letters for Google Drive virtual drive
    #[cfg(windows)]
    {
        for letter in b'D'..=b'Z' {
            let path = PathBuf::from(format!("{}:\\My Drive", letter as char));
            if path.is_dir() && !already_found(&found, &path) {
                found.push(make_destination(
                    CloudProvider::GoogleDrive,
                    path,
                    None,
                ));
                break; // usually only one
            }
        }
    }

    found
}

/// Detect Dropbox sync folder.
///
/// Windows: `%APPDATA%\Dropbox\info.json` → `personal.path` / `business.path`
///          or `%USERPROFILE%\Dropbox`.
/// Linux/macOS: `$HOME/Dropbox` or `$HOME/.dropbox/info.json`.
#[must_use]
pub fn detect_dropbox() -> Vec<CloudDestination> {
    let mut found = Vec::new();

    // Try info.json first (most reliable)
    let info_paths = dropbox_info_json_paths();
    for info_path in &info_paths {
        if let Ok(contents) = std::fs::read_to_string(info_path) {
            if let Ok(info) = serde_json::from_str::<serde_json::Value>(&contents) {
                for key in ["personal", "business"] {
                    if let Some(path_str) = info
                        .get(key)
                        .and_then(|v| v.get("path"))
                        .and_then(|v| v.as_str())
                    {
                        let path = PathBuf::from(path_str);
                        if path.is_dir() && !already_found(&found, &path) {
                            found.push(make_destination(
                                CloudProvider::Dropbox,
                                path,
                                Some(format!("Dropbox ({key})")),
                            ));
                        }
                    }
                }
            }
        }
    }

    // Fallback: well-known path
    if let Some(home) = home_dir() {
        let path = home.join("Dropbox");
        if path.is_dir() && !already_found(&found, &path) {
            found.push(make_destination(CloudProvider::Dropbox, path, None));
        }
    }

    found
}

// ---- Windows registry detection (OneDrive) ----------------------------------

#[cfg(windows)]
fn detect_onedrive_registry() -> Vec<CloudDestination> {
    use ::windows::Win32::System::Registry::*;
    let mut found = Vec::new();

    let accounts_key = r"Software\Microsoft\OneDrive\Accounts";
    // Enumerate subkeys: "Personal", "Business1", etc.
    let hkey = match open_reg_key(HKEY_CURRENT_USER, accounts_key) {
        Some(h) => h,
        None => return found,
    };

    for subkey_name in enum_reg_subkeys(hkey) {
        let sub_path = format!("{accounts_key}\\{subkey_name}");
        if let Some(sub) = open_reg_key(HKEY_CURRENT_USER, &sub_path) {
            if let Some(folder) = read_reg_string(sub, "UserFolder") {
                let path = PathBuf::from(&folder);
                if path.is_dir() {
                    let provider = if subkey_name.eq_ignore_ascii_case("personal") {
                        CloudProvider::OneDrivePersonal
                    } else {
                        CloudProvider::OneDriveBusiness
                    };
                    let label_name = read_reg_string(sub, "DisplayName")
                        .unwrap_or_else(|| subkey_name.clone());
                    found.push(make_destination(
                        provider,
                        path,
                        Some(format!("{} - {}", provider.display_name(), label_name)),
                    ));
                }
            }
            close_reg_key(sub);
        }
    }
    close_reg_key(hkey);
    found
}

#[cfg(windows)]
fn open_reg_key(base: ::windows::Win32::System::Registry::HKEY, path: &str) -> Option<::windows::Win32::System::Registry::HKEY> {
    use ::windows::core::PCSTR;
    use std::ffi::CString;
    let cs = CString::new(path).ok()?;
    let mut key = ::windows::Win32::System::Registry::HKEY::default();
    let status = unsafe {
        ::windows::Win32::System::Registry::RegOpenKeyExA(
            base,
            PCSTR::from_raw(cs.as_ptr() as *const u8),
            0,
            ::windows::Win32::System::Registry::KEY_READ,
            &mut key,
        )
    };
    if status.is_ok() { Some(key) } else { None }
}

#[cfg(windows)]
fn close_reg_key(key: ::windows::Win32::System::Registry::HKEY) {
    unsafe { let _ = ::windows::Win32::System::Registry::RegCloseKey(key); }
}

#[cfg(windows)]
fn read_reg_string(key: ::windows::Win32::System::Registry::HKEY, name: &str) -> Option<String> {
    use ::windows::core::PCSTR;
    use std::ffi::CString;
    let cs = CString::new(name).ok()?;
    let mut buf = vec![0u8; 1024];
    let mut len = buf.len() as u32;
    let mut kind = 0u32;
    let status = unsafe {
        ::windows::Win32::System::Registry::RegQueryValueExA(
            key,
            PCSTR::from_raw(cs.as_ptr() as *const u8),
            None,
            Some(&mut kind),
            Some(buf.as_mut_ptr()),
            Some(&mut len),
        )
    };
    if status.is_ok() && kind == 1 { // REG_SZ
        let s = String::from_utf8_lossy(&buf[..len as usize]);
        Some(s.trim_end_matches('\0').to_string())
    } else {
        None
    }
}

#[cfg(windows)]
fn enum_reg_subkeys(key: ::windows::Win32::System::Registry::HKEY) -> Vec<String> {
    let mut names = Vec::new();
    for i in 0..32u32 {
        let mut buf = vec![0u8; 256];
        let mut len = buf.len() as u32;
        let status = unsafe {
            ::windows::Win32::System::Registry::RegEnumKeyExA(
                key,
                i,
                ::windows::core::PSTR::from_raw(buf.as_mut_ptr()),
                &mut len,
                None,
                ::windows::core::PSTR::null(),
                None,
                None,
            )
        };
        if status.is_err() {
            break;
        }
        let name = String::from_utf8_lossy(&buf[..len as usize]).to_string();
        names.push(name);
    }
    names
}

// ---- Helpers ----------------------------------------------------------------

fn make_destination(
    provider: CloudProvider,
    local_path: PathBuf,
    label_override: Option<String>,
) -> CloudDestination {
    let label = label_override.unwrap_or_else(|| provider.display_name().to_string());
    let free_bytes = free_space(&local_path);
    let sync_active = is_sync_running(provider);

    tracing::debug!(
        provider = provider.display_name(),
        path = %local_path.display(),
        free_bytes,
        sync_active,
        "detected cloud destination"
    );

    CloudDestination {
        provider,
        label,
        local_path,
        free_bytes,
        sync_active,
    }
}

fn already_found(list: &[CloudDestination], path: &Path) -> bool {
    list.iter().any(|d| d.local_path == path)
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn dropbox_info_json_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            paths.push(PathBuf::from(&appdata).join("Dropbox").join("info.json"));
        }
        if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
            paths.push(PathBuf::from(&localappdata).join("Dropbox").join("info.json"));
        }
    }
    if let Some(home) = home_dir() {
        paths.push(home.join(".dropbox").join("info.json"));
    }
    paths
}

/// Query free disk space on the volume containing `path`.
fn free_space(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = OsString::from(path.as_os_str())
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut free: u64 = 0;
        let ok = unsafe {
            ::windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                ::windows::core::PCWSTR(wide.as_ptr()),
                None,
                None,
                Some(&mut free),
            )
        };
        if ok.is_ok() {
            Some(free)
        } else {
            None
        }
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        let cs = CString::new(path.to_string_lossy().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let r = unsafe { libc::statvfs(cs.as_ptr(), &mut stat) };
        if r == 0 {
            Some(stat.f_bavail as u64 * stat.f_frsize as u64)
        } else {
            None
        }
    }
}

/// Best-effort check: is the sync client running?
fn is_sync_running(provider: CloudProvider) -> bool {
    #[cfg(windows)]
    {
        // Quick heuristic: check if the process is running via tasklist.
        // A proper implementation would use CreateToolhelp32Snapshot.
        let process_name = match provider {
            CloudProvider::OneDrivePersonal | CloudProvider::OneDriveBusiness => "OneDrive.exe",
            CloudProvider::GoogleDrive => "GoogleDriveFS.exe",
            CloudProvider::Dropbox => "Dropbox.exe",
        };
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {process_name}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(process_name))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        // Dev-only: check process list
        let process_hint = match provider {
            CloudProvider::OneDrivePersonal | CloudProvider::OneDriveBusiness => "onedrive",
            CloudProvider::GoogleDrive => "google-drive",
            CloudProvider::Dropbox => "dropbox",
        };
        std::process::Command::new("pgrep")
            .args(["-f", process_hint])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

// ---- Future: Direct API upload scaffold -------------------------------------

/// Trait for direct cloud upload (Microsoft Graph, Google Drive API).
/// Not yet implemented — save goes through the local sync folder for v0.1.
/// See `docs/architecture.md` §12.
pub trait CloudUploader: Send + Sync + std::fmt::Debug {
    /// Upload a single file to the cloud destination.
    /// `remote_path` is relative to the cloud root (e.g. "TriRecover Recovery/photo.jpg").
    fn upload(
        &self,
        local_file: &Path,
        remote_path: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::Result<()>> + Send + '_>>;

    /// Check remaining cloud quota in bytes.
    fn remaining_quota(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::Result<u64>> + Send + '_>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_all_does_not_panic() {
        // May return empty on a dev machine without cloud clients.
        let dests = detect_all();
        for d in &dests {
            assert!(!d.label.is_empty());
            assert!(d.local_path.as_os_str().len() > 0);
        }
    }

    #[test]
    fn provider_display_names() {
        assert_eq!(CloudProvider::OneDrivePersonal.display_name(), "OneDrive");
        assert_eq!(
            CloudProvider::OneDriveBusiness.display_name(),
            "OneDrive for Business"
        );
        assert_eq!(CloudProvider::GoogleDrive.display_name(), "Google Drive");
        assert_eq!(CloudProvider::Dropbox.display_name(), "Dropbox");
    }

    #[test]
    fn recovery_dir_is_inside_sync_folder() {
        let d = CloudDestination {
            provider: CloudProvider::OneDrivePersonal,
            label: "OneDrive".into(),
            local_path: PathBuf::from("/home/test/OneDrive"),
            free_bytes: None,
            sync_active: false,
        };
        assert!(d.recovery_dir().starts_with("/home/test/OneDrive"));
        assert!(d.recovery_dir().ends_with("TriRecover Recovery"));
    }

    #[test]
    fn already_found_deduplicates() {
        let d = CloudDestination {
            provider: CloudProvider::Dropbox,
            label: "x".into(),
            local_path: PathBuf::from("/a/b"),
            free_bytes: None,
            sync_active: false,
        };
        assert!(already_found(&[d.clone()], Path::new("/a/b")));
        assert!(!already_found(&[d], Path::new("/a/c")));
    }
}
