//! `tr-storage` — read-only raw sector I/O.
//!
//! This crate is the only place in the workspace where `unsafe` is permitted,
//! and it is restricted to the FFI shims in `windows.rs` / `linux.rs`. Every
//! handle opened here is `O_RDONLY` / `GENERIC_READ` only. There is no public
//! API to obtain a writable handle.
//!
//! See `docs/architecture.md` §5 (read-only invariant).

#![deny(missing_debug_implementations)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod drive;
pub mod sector;
pub mod smart;

#[cfg(windows)]
mod windows;
#[cfg(unix)]
mod linux;
mod fixture;

#[cfg(windows)]
pub use crate::windows::{enumerate_drives, open_drive};
#[cfg(unix)]
pub use crate::linux::{enumerate_drives, open_drive};

pub use drive::{Drive, DriveHandle};
pub use fixture::FixtureReader;
pub use sector::{ReadOptions, SectorReader, SectorReaderExt};
pub use smart::SmartProvider;
