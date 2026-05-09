//! `tr-partition` — MBR and GPT parsers.
//!
//! Pure functions over `&[u8]` slices; the higher-level entry point reads
//! the relevant LBAs through a `SectorReader` and validates them.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod gpt;
pub mod mbr;

use tr_core::{PartitionInfo, PartitionScheme, Result};
use tr_storage::{SectorReader, SectorReaderExt};

/// Read the partition table from a device. Tries GPT first (because a GPT
/// disk has a protective MBR with type 0xEE that would otherwise fool MBR
/// parsing), falls back to MBR, and finally to a single `Raw` partition
/// covering the whole device.
pub async fn read_table(reader: &dyn SectorReader) -> Result<Vec<PartitionInfo>> {
    let ssz = reader.sector_size();
    if ssz == 0 || ssz > 65_536 {
        return Err(tr_core::Error::corrupt(
            "sector_size",
            0,
            format!("implausible sector size {ssz}"),
        ));
    }

    // Read first 33 sectors — covers MBR (1) + GPT header (1) + GPT entries
    // (typically 32 sectors of 128 entries × 128 bytes when sector_size=512).
    let header = reader.read_lba_run(0, 34).await?;

    if mbr::is_protective_mbr(&header[..ssz as usize]) {
        match gpt::parse(&header, ssz) {
            Ok(parts) => return Ok(parts),
            Err(e) => {
                tracing::warn!("GPT parse failed despite protective MBR: {e}; falling back");
            }
        }
    }

    if let Ok(parts) = mbr::parse(&header[..ssz as usize], ssz, reader.size_bytes()) {
        if !parts.is_empty() {
            return Ok(parts);
        }
    }

    // No table — treat the whole device as one raw partition so callers can
    // still attempt RAW carving.
    let length_sectors = reader.size_bytes() / u64::from(ssz);
    Ok(vec![PartitionInfo {
        index: 0,
        scheme: PartitionScheme::Raw,
        type_id: "raw".into(),
        name: None,
        filesystem: None,
        start_lba: 0,
        length_sectors,
        sector_size: ssz,
        reconstructed: false,
    }])
}
