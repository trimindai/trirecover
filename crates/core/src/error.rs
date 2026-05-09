//! Workspace-wide error type. Every public function returns `Result<T>`.
//!
//! Variants are deliberately fine-grained so the UI layer can render
//! actionable messages and the recovery engine can branch on cause.

use std::path::PathBuf;
use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    // ---------------- I/O & OS ----------------
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("OS error {code}: {msg}")]
    Os { code: i32, msg: String },

    #[error("permission denied — TriRecover must run as administrator to access raw disks")]
    PermissionDenied,

    #[error("device not found: {0}")]
    DeviceNotFound(String),

    #[error("device busy: {0}")]
    DeviceBusy(String),

    // ---------------- Read-only invariant ----------------
    #[error(
        "refusing to recover to the same volume as the source drive (would overwrite the data \
         being recovered) — choose a different destination"
    )]
    SameVolumeRecoveryRefused,

    #[error("attempted write through a read-only handle (this is a bug, please report it)")]
    ReadOnlyViolation,

    // ---------------- Parsing ----------------
    #[error("unexpected end of buffer at offset {offset} (need {need} bytes, have {have})")]
    UnexpectedEof {
        offset: u64,
        need: usize,
        have: usize,
    },

    #[error("bad magic at offset {offset}: expected {expected}, got {got}")]
    BadMagic {
        offset: u64,
        expected: &'static str,
        got: String,
    },

    #[error("CRC mismatch at offset {offset}: expected {expected:#010x}, got {got:#010x}")]
    BadCrc {
        offset: u64,
        expected: u32,
        got: u32,
    },

    #[error("unsupported filesystem: {0}")]
    UnsupportedFilesystem(String),

    #[error("corrupt {what} at offset {offset}: {detail}")]
    Corrupt {
        what: &'static str,
        offset: u64,
        detail: String,
    },

    // ---------------- Session / DB ----------------
    #[error("session error: {0}")]
    Session(String),

    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("database migration error: {0}")]
    DbMigrate(#[from] sqlx::migrate::MigrateError),

    // ---------------- Config ----------------
    #[error("config error: {0}")]
    Config(String),

    #[error("config path not writable: {0}")]
    ConfigPath(PathBuf),

    // ---------------- Job control ----------------
    #[error("job not found: {0}")]
    JobNotFound(String),

    #[error("job already finished: {0}")]
    JobFinished(String),

    #[error("scan cancelled")]
    Cancelled,

    // ---------------- Generic escape hatch ----------------
    #[error("internal: {0}")]
    Internal(String),
}

impl Error {
    /// Build an `Os` error from a Windows / POSIX errno-like value.
    #[must_use]
    pub fn os(code: i32, msg: impl Into<String>) -> Self {
        Self::Os {
            code,
            msg: msg.into(),
        }
    }

    /// Build a `Corrupt` error.
    #[must_use]
    pub fn corrupt(what: &'static str, offset: u64, detail: impl Into<String>) -> Self {
        Self::Corrupt {
            what,
            offset,
            detail: detail.into(),
        }
    }

    /// Build a generic internal error. Avoid in new code — prefer a typed variant.
    #[must_use]
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    /// True if the error is the kind a user can fix (permissions, busy device,
    /// missing destination). False for genuine corruption / bugs.
    #[must_use]
    pub fn is_user_actionable(&self) -> bool {
        matches!(
            self,
            Self::PermissionDenied
                | Self::DeviceBusy(_)
                | Self::DeviceNotFound(_)
                | Self::SameVolumeRecoveryRefused
                | Self::ConfigPath(_)
        )
    }
}

/// Convert any error into a stable kebab-case string for telemetry hashing.
impl Error {
    #[must_use]
    pub fn telemetry_kind(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::Os { .. } => "os",
            Self::PermissionDenied => "permission-denied",
            Self::DeviceNotFound(_) => "device-not-found",
            Self::DeviceBusy(_) => "device-busy",
            Self::SameVolumeRecoveryRefused => "same-volume-refused",
            Self::ReadOnlyViolation => "read-only-violation",
            Self::UnexpectedEof { .. } => "eof",
            Self::BadMagic { .. } => "bad-magic",
            Self::BadCrc { .. } => "bad-crc",
            Self::UnsupportedFilesystem(_) => "unsupported-fs",
            Self::Corrupt { .. } => "corrupt",
            Self::Session(_) => "session",
            Self::Db(_) => "db",
            Self::DbMigrate(_) => "db-migrate",
            Self::Config(_) => "config",
            Self::ConfigPath(_) => "config-path",
            Self::JobNotFound(_) => "job-not-found",
            Self::JobFinished(_) => "job-finished",
            Self::Cancelled => "cancelled",
            Self::Internal(_) => "internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_actionable_classification() {
        assert!(Error::PermissionDenied.is_user_actionable());
        assert!(!Error::Cancelled.is_user_actionable());
        assert!(!Error::internal("bug").is_user_actionable());
    }

    #[test]
    fn telemetry_kind_is_stable() {
        assert_eq!(Error::PermissionDenied.telemetry_kind(), "permission-denied");
        assert_eq!(Error::Cancelled.telemetry_kind(), "cancelled");
    }
}
