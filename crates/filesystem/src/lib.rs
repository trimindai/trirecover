//! `tr-filesystem` — read-only NTFS / FAT32 / exFAT parsers.
//!
//! Each volume implementation exposes:
//! - `open(reader, partition_lba)` — sniff and validate the boot sector
//! - `iter_files()` — async stream of `FileRecord`s, including deleted ones

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc, clippy::too_many_lines)]

pub mod exfat;
pub mod fat;
pub mod ntfs;

use tr_core::{PartitionInfo, Result};
use tr_storage::SectorReaderExt;

/// Detected filesystem identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsKind {
    Ntfs,
    Fat32,
    ExFat,
    Unknown,
}

impl FsKind {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Ntfs => "ntfs",
            Self::Fat32 => "fat32",
            Self::ExFat => "exfat",
            Self::Unknown => "unknown",
        }
    }
}

/// Sniff the filesystem of a partition by reading the boot sector.
pub async fn detect(
    reader: &dyn tr_storage::SectorReader,
    partition: &PartitionInfo,
) -> Result<FsKind> {
    let bs = reader.read_lba(partition.start_lba).await?;
    if bs.len() < 512 {
        return Ok(FsKind::Unknown);
    }
    if &bs[3..11] == b"NTFS    " {
        return Ok(FsKind::Ntfs);
    }
    if &bs[3..11] == b"EXFAT   " {
        return Ok(FsKind::ExFat);
    }
    // FAT32 detection: bytes_per_sector valid + sectors_per_cluster power of 2 +
    //                  fs_type at offset 0x52 contains "FAT32"
    if bs.len() >= 0x5A && &bs[0x52..0x5A] == b"FAT32   " {
        return Ok(FsKind::Fat32);
    }
    Ok(FsKind::Unknown)
}
