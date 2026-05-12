//! `tr-core` — foundational types, errors, configuration, logging, and the
//! scan-session SQLite store shared by every other crate.
//!
//! No other workspace crate may depend on a higher layer; everything imports
//! from here downward. See `docs/architecture.md`.

#![deny(unsafe_code)]
#![deny(missing_debug_implementations)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

#[allow(unsafe_code)]
pub mod cloud;
pub mod config;
pub mod db;
pub mod error;
pub mod logging;
pub mod types;

pub use error::{Error, Result};
pub use cloud::{CloudDestination, CloudProvider};
pub use types::{
    CarvedFile, DataRun, DriveBus, DriveInfo, DriveKind, FileCategory, FileKind, FileRecord,
    FileSource, JobId, JobState, PartitionInfo, PartitionScheme, RecoverDestination,
    RecoverFailure, RecoverReport, RecoverRequest, RecoveryStrategy, ScanProgress, ScanRequest,
    SessionId, SmartAttribute, SmartHealth, SmartReport,
};

/// Crate version, exposed for telemetry and the about-box.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Product display name. Centralized to keep branding consistent.
pub const PRODUCT_NAME: &str = "TriRecover";

/// Reverse-DNS app id — used by Tauri, the installer, and the updater.
pub const APP_ID: &str = "tech.trimind.trirecover";
