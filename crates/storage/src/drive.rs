//! Drive abstraction: a `Drive` is an enumerated device. A `DriveHandle` is an
//! opened read-only handle backed by a [`SectorReader`].

use crate::SectorReader;
use std::sync::Arc;
use tr_core::DriveInfo;

/// An enumerated drive — produced by `enumerate_drives` and consumed by
/// `open_drive`.
#[derive(Debug, Clone)]
pub struct Drive {
    pub info: DriveInfo,
}

impl Drive {
    #[must_use]
    pub fn new(info: DriveInfo) -> Self {
        Self { info }
    }
}

/// An opened, read-only handle.
#[derive(Debug, Clone)]
pub struct DriveHandle {
    pub info: DriveInfo,
    pub reader: Arc<dyn SectorReader>,
}

impl DriveHandle {
    #[must_use]
    pub fn new(info: DriveInfo, reader: Arc<dyn SectorReader>) -> Self {
        Self { info, reader }
    }

    /// Return the inner reader for sharing with workers.
    #[must_use]
    pub fn reader(&self) -> Arc<dyn SectorReader> {
        Arc::clone(&self.reader)
    }
}
