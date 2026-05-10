//! Shared domain types. Mirrored to TypeScript in `frontend/src/lib/types.ts`.
//! When you change a field here, update the TS copy and run
//! `scripts/check-bindings.sh`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

// ---------------- IDs ----------------

/// Stable identifier for a scan job within a process lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub Uuid);

impl JobId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Stable identifier for a persisted scan session (= one SQLite file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------- Drive ----------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DriveInfo {
    /// OS path (e.g. `\\.\PhysicalDrive0` on Windows, `/dev/sda` on Linux).
    pub path: String,
    /// Vendor-supplied model string.
    pub model: String,
    /// Serial number if the OS exposed one (otherwise empty).
    pub serial: String,
    /// Total size in bytes.
    pub size_bytes: u64,
    /// Logical sector size (almost always 512 or 4096).
    pub sector_size: u32,
    /// Removable / fixed / virtual.
    pub kind: DriveKind,
    /// Bus the drive sits on.
    pub bus: DriveBus,
    /// True if SMART data is available for this drive.
    pub smart_available: bool,
    /// True if any volume on the drive is currently mounted (informational).
    pub has_mounted_volumes: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DriveKind {
    Hdd,
    Ssd,
    Nvme,
    UsbFlash,
    SdCard,
    External,
    Virtual,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DriveBus {
    Sata,
    Nvme,
    Usb,
    Sd,
    Scsi,
    Virtual,
    #[default]
    Unknown,
}

impl DriveKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hdd => "Hdd",
            Self::Ssd => "Ssd",
            Self::Nvme => "Nvme",
            Self::UsbFlash => "UsbFlash",
            Self::SdCard => "SdCard",
            Self::External => "External",
            Self::Virtual => "Virtual",
            Self::Unknown => "Unknown",
        }
    }
}

impl DriveBus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sata => "Sata",
            Self::Nvme => "Nvme",
            Self::Usb => "Usb",
            Self::Sd => "Sd",
            Self::Scsi => "Scsi",
            Self::Virtual => "Virtual",
            Self::Unknown => "Unknown",
        }
    }
}

// ---------------- SMART ----------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartReport {
    pub drive_path: String,
    pub overall: SmartHealth,
    /// Temperature in degrees Celsius if reported.
    pub temperature_c: Option<i16>,
    /// Power-on hours if reported.
    pub power_on_hours: Option<u64>,
    /// Reallocated sector count, if reported. High values are bad news.
    pub reallocated_sectors: Option<u64>,
    /// Pending sectors awaiting reallocation, if reported.
    pub pending_sectors: Option<u64>,
    /// SSD-only: lifetime media wear indicator (0..=100, 100 = new).
    pub wear_leveling_remaining: Option<u8>,
    /// Vendor-specific raw attributes (id, name, value, worst, raw).
    pub raw_attributes: Vec<SmartAttribute>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SmartHealth {
    Ok,
    Caution,
    Failing,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartAttribute {
    pub id: u8,
    pub name: String,
    pub value: u8,
    pub worst: u8,
    pub threshold: u8,
    pub raw: u64,
}

// ---------------- Partitions ----------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartitionInfo {
    pub index: u32,
    pub scheme: PartitionScheme,
    /// Type GUID (GPT) or single-byte type (MBR), normalized to a string.
    pub type_id: String,
    /// Friendly name if the scheme provides one (GPT only).
    pub name: Option<String>,
    /// Filesystem detected by sniffing the first sectors. None if unknown.
    pub filesystem: Option<String>,
    /// First sector LBA on the parent disk.
    pub start_lba: u64,
    /// Length in sectors.
    pub length_sectors: u64,
    /// Sector size of the parent disk (carried for convenience).
    pub sector_size: u32,
    /// True if this partition was inferred (not in the on-disk table).
    pub reconstructed: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PartitionScheme {
    Mbr,
    Gpt,
    /// No partition table detected — whole disk is treated as a single volume.
    Raw,
}

// ---------------- Recovery / scan ----------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryStrategy {
    /// Filesystem metadata only — fast, finds recently deleted files.
    Quick,
    /// Quick + carve unallocated regions for known signatures.
    Deep,
    /// Pure carving across the entire device (no FS interpretation).
    Raw,
    /// Reconstruct missing partitions then quick-scan each.
    Partition,
    /// Treat the volume as if it had been reformatted; trust no metadata.
    Formatted,
    /// Best-effort scan of a corrupted FS — repair journals in memory.
    CorruptedFs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRequest {
    pub drive_path: String,
    pub strategy: RecoveryStrategy,
    /// Filter to specific filesystem partitions (empty = all).
    pub partitions: Vec<u32>,
    /// Filter to specific file kinds (empty = all known kinds).
    pub file_kinds: Vec<FileKind>,
    /// Minimum carve size in bytes (filters tiny false positives).
    pub min_carve_bytes: u64,
    /// Optional resume from a saved session.
    pub resume_session: Option<SessionId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
    pub job_id: JobId,
    pub state: JobState,
    pub sectors_scanned: u64,
    pub sectors_total: u64,
    pub files_found: u64,
    pub bytes_recoverable: u64,
    pub eta_secs: Option<u64>,
    pub current_phase: String,
    pub bad_sectors_skipped: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Finished,
    Failed,
    Cancelled,
}

// ---------------- Files ----------------

/// A file located by the scanner — either via filesystem metadata or by carving.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub size_bytes: u64,
    pub modified: Option<DateTime<Utc>>,
    pub source: FileSource,
    /// 0..=100 estimated probability the recovered file will open correctly.
    pub recoverability: u8,
    /// First 16 bytes of the file as hex, for quick sniffing in the UI.
    pub head_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "via", rename_all = "kebab-case")]
pub enum FileSource {
    /// Located via NTFS MFT / FAT directory / exFAT directory.
    Filesystem {
        partition_index: u32,
        record_id: u64,
        is_deleted: bool,
        is_resident: bool,
        runs: Vec<DataRun>,
    },
    /// Located via signature carving in unallocated space.
    Carved {
        offset_bytes: u64,
        length_bytes: u64,
        signature: String,
    },
}

/// A `(start_lba, length_sectors)` pair within the parent device.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataRun {
    pub start_lba: u64,
    pub length_sectors: u64,
}

#[derive(Debug, Clone)]
pub struct CarvedFile {
    pub kind: FileKind,
    pub offset_bytes: u64,
    pub length_bytes: u64,
    pub signature: &'static str,
    pub recoverability: u8,
}

// ---------------- File kinds ----------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum FileKind {
    Jpg,
    Png,
    Gif,
    Bmp,
    Tiff,
    Mp4,
    Mov,
    Mkv,
    Avi,
    Pdf,
    Docx,
    Xlsx,
    Pptx,
    Zip,
    Rar,
    SevenZ,
    Psd,
    Ai,
    Txt,
    Csv,
    Sql,
    Other,
}

impl FileKind {
    /// Human-readable label (no heap allocation).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jpg => "Jpg",
            Self::Png => "Png",
            Self::Gif => "Gif",
            Self::Bmp => "Bmp",
            Self::Tiff => "Tiff",
            Self::Mp4 => "Mp4",
            Self::Mov => "Mov",
            Self::Mkv => "Mkv",
            Self::Avi => "Avi",
            Self::Pdf => "Pdf",
            Self::Docx => "Docx",
            Self::Xlsx => "Xlsx",
            Self::Pptx => "Pptx",
            Self::Zip => "Zip",
            Self::Rar => "Rar",
            Self::SevenZ => "SevenZ",
            Self::Psd => "Psd",
            Self::Ai => "Ai",
            Self::Txt => "Txt",
            Self::Csv => "Csv",
            Self::Sql => "Sql",
            Self::Other => "Other",
        }
    }

    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Mkv => "mkv",
            Self::Avi => "avi",
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
            Self::Zip => "zip",
            Self::Rar => "rar",
            Self::SevenZ => "7z",
            Self::Psd => "psd",
            Self::Ai => "ai",
            Self::Txt => "txt",
            Self::Csv => "csv",
            Self::Sql => "sql",
            Self::Other => "bin",
        }
    }

    #[must_use]
    pub fn category(self) -> FileCategory {
        match self {
            Self::Jpg | Self::Png | Self::Gif | Self::Bmp | Self::Tiff | Self::Psd | Self::Ai => {
                FileCategory::Image
            }
            Self::Mp4 | Self::Mov | Self::Mkv | Self::Avi => FileCategory::Video,
            Self::Pdf => FileCategory::Document,
            Self::Docx | Self::Xlsx | Self::Pptx => FileCategory::Office,
            Self::Zip | Self::Rar | Self::SevenZ => FileCategory::Archive,
            Self::Txt | Self::Csv | Self::Sql => FileCategory::Text,
            Self::Other => FileCategory::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum FileCategory {
    Image,
    Video,
    Document,
    Office,
    Archive,
    Text,
    Other,
}

// ---------------- Recovery output ----------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverRequest {
    pub session_id: SessionId,
    pub file_ids: Vec<u64>,
    pub destination: PathBuf,
    /// If true, preserve original directory layout under `destination`.
    pub preserve_paths: bool,
    /// If true, run per-file integrity validation after writing.
    pub verify: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverReport {
    pub recovered: u64,
    pub failed: u64,
    pub bytes_written: u64,
    pub destination: PathBuf,
    pub failures: Vec<RecoverFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverFailure {
    pub file_id: u64,
    pub name: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_kind_extension_round_trips() {
        for k in [
            FileKind::Jpg,
            FileKind::Png,
            FileKind::SevenZ,
            FileKind::Other,
        ] {
            assert!(!k.extension().is_empty());
        }
    }

    #[test]
    fn category_classifies_archives() {
        assert_eq!(FileKind::Zip.category(), FileCategory::Archive);
        assert_eq!(FileKind::SevenZ.category(), FileCategory::Archive);
        assert_eq!(FileKind::Rar.category(), FileCategory::Archive);
    }

    #[test]
    fn job_id_uniqueness() {
        let a = JobId::new();
        let b = JobId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn round_trip_serde_drive_info() {
        let d = DriveInfo {
            path: r"\\.\PhysicalDrive0".into(),
            model: "Samsung SSD 980".into(),
            serial: "S1234".into(),
            size_bytes: 1_000_204_886_016,
            sector_size: 512,
            kind: DriveKind::Nvme,
            bus: DriveBus::Nvme,
            smart_available: true,
            has_mounted_volumes: true,
        };
        let j = serde_json::to_string(&d).unwrap();
        let r: DriveInfo = serde_json::from_str(&j).unwrap();
        assert_eq!(d, r);
    }
}
