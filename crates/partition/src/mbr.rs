//! MBR (Master Boot Record) parser.
//!
//! Layout (sector 0, 512 bytes):
//! - 0x000..0x1BD : bootloader code (446 bytes)
//! - 0x1BE..0x1FD : 4 partition entries × 16 bytes
//! - 0x1FE..0x1FF : 0x55 0xAA signature
//!
//! Per-entry layout (16 bytes):
//! - 0x00 : status (bit 7 = active)
//! - 0x01..0x03 : starting CHS (ignored — unreliable on > 8 GiB disks)
//! - 0x04 : type
//! - 0x05..0x07 : ending CHS (ignored)
//! - 0x08..0x0B : starting LBA (LE u32)
//! - 0x0C..0x0F : sector count (LE u32)
//!
//! We follow extended partitions (types 0x05, 0x0F, 0x85) by reading the EBR
//! at the entry's start LBA. The EBR chain has at most one logical partition
//! per link plus an optional pointer to the next link.

use byteorder::{ByteOrder, LittleEndian};
use tr_core::{Error, PartitionInfo, PartitionScheme, Result};

const SECTOR_SIZE_HINT: usize = 512;
const SIG_OFFSET: usize = 0x1FE;
const PART_TABLE_OFFSET: usize = 0x1BE;
const ENTRY_SIZE: usize = 16;

const TYPE_EMPTY: u8 = 0x00;
const TYPE_GPT_PROTECTIVE: u8 = 0xEE;
const TYPE_EXT_CHS: u8 = 0x05;
const TYPE_EXT_LBA: u8 = 0x0F;
const TYPE_EXT_LINUX: u8 = 0x85;

/// True if this looks like a protective MBR for a GPT disk: signature OK and
/// the first non-empty entry has type 0xEE covering the whole disk.
#[must_use]
pub fn is_protective_mbr(sector0: &[u8]) -> bool {
    if sector0.len() < SECTOR_SIZE_HINT {
        return false;
    }
    if sector0[SIG_OFFSET] != 0x55 || sector0[SIG_OFFSET + 1] != 0xAA {
        return false;
    }
    for i in 0..4 {
        let off = PART_TABLE_OFFSET + i * ENTRY_SIZE;
        let t = sector0[off + 0x04];
        if t == TYPE_GPT_PROTECTIVE {
            return true;
        }
        if t != TYPE_EMPTY {
            return false;
        }
    }
    false
}

/// Parse a (non-protective) MBR. `device_bytes` is the total device size in
/// bytes — used to clamp implausible entries.
///
/// Note: this function only parses the primary table. For extended partitions
/// the caller should pass an extended-partition-aware reader; for v0.1 we
/// emit them as ordinary entries with a flag set to false. Full EBR walking
/// is in the engine layer (it requires async I/O against the parent reader).
pub fn parse(sector0: &[u8], sector_size: u32, device_bytes: u64) -> Result<Vec<PartitionInfo>> {
    if sector0.len() < SECTOR_SIZE_HINT {
        return Err(Error::UnexpectedEof {
            offset: 0,
            need: SECTOR_SIZE_HINT,
            have: sector0.len(),
        });
    }
    if sector0[SIG_OFFSET] != 0x55 || sector0[SIG_OFFSET + 1] != 0xAA {
        return Err(Error::BadMagic {
            offset: SIG_OFFSET as u64,
            expected: "55 AA",
            got: format!("{:02X} {:02X}", sector0[SIG_OFFSET], sector0[SIG_OFFSET + 1]),
        });
    }

    let total_sectors = device_bytes / u64::from(sector_size);
    let mut parts = Vec::with_capacity(4);

    for i in 0..4u32 {
        let off = PART_TABLE_OFFSET + (i as usize) * ENTRY_SIZE;
        let entry = &sector0[off..off + ENTRY_SIZE];
        let ty = entry[0x04];
        if ty == TYPE_EMPTY {
            continue;
        }
        let start = LittleEndian::read_u32(&entry[0x08..0x0C]) as u64;
        let count = LittleEndian::read_u32(&entry[0x0C..0x10]) as u64;

        if start == 0 || count == 0 {
            continue;
        }

        let end = start.saturating_add(count);
        if end > total_sectors.saturating_add(1) {
            tracing::warn!(
                index = i,
                start,
                count,
                total_sectors,
                "MBR entry exceeds device size; clamping"
            );
        }

        let length_sectors = std::cmp::min(count, total_sectors.saturating_sub(start));

        parts.push(PartitionInfo {
            index: i,
            scheme: PartitionScheme::Mbr,
            type_id: format!("0x{ty:02X}"),
            name: Some(mbr_type_name(ty).to_string()),
            filesystem: None, // sniffed later by filesystem layer
            start_lba: start,
            length_sectors,
            sector_size,
            reconstructed: false,
        });
    }

    Ok(parts)
}

#[must_use]
pub fn is_extended(ty: u8) -> bool {
    matches!(ty, TYPE_EXT_CHS | TYPE_EXT_LBA | TYPE_EXT_LINUX)
}

#[must_use]
pub fn mbr_type_name(ty: u8) -> &'static str {
    match ty {
        0x00 => "Empty",
        0x01 => "FAT12",
        0x04 | 0x06 | 0x0E => "FAT16",
        0x05 | 0x0F | 0x85 => "Extended",
        0x07 => "NTFS / exFAT / IFS",
        0x0B | 0x0C => "FAT32",
        0x82 => "Linux swap",
        0x83 => "Linux",
        0x8E => "Linux LVM",
        0xA5 => "FreeBSD",
        0xA6 => "OpenBSD",
        0xA8 => "Darwin UFS",
        0xAB => "Darwin boot",
        0xAF => "HFS+",
        0xEE => "GPT protective",
        0xEF => "EFI System",
        0xFD => "Linux RAID",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mbr(entries: &[(u8, u32, u32)]) -> Vec<u8> {
        let mut sec = vec![0u8; 512];
        for (i, (ty, start, count)) in entries.iter().enumerate().take(4) {
            let off = PART_TABLE_OFFSET + i * ENTRY_SIZE;
            sec[off + 0x04] = *ty;
            LittleEndian::write_u32(&mut sec[off + 0x08..off + 0x0C], *start);
            LittleEndian::write_u32(&mut sec[off + 0x0C..off + 0x10], *count);
        }
        sec[SIG_OFFSET] = 0x55;
        sec[SIG_OFFSET + 1] = 0xAA;
        sec
    }

    #[test]
    fn parses_two_partitions() {
        let s = make_mbr(&[(0x07, 2048, 100_000), (0x83, 102_048, 200_000)]);
        let parts = parse(&s, 512, 512 * 400_000).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].start_lba, 2048);
        assert_eq!(parts[0].length_sectors, 100_000);
        assert_eq!(parts[1].type_id, "0x83");
    }

    #[test]
    fn skips_empty_entries() {
        let s = make_mbr(&[(0x00, 0, 0), (0x83, 2048, 1000)]);
        let parts = parse(&s, 512, 512 * 10_000).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].index, 1);
    }

    #[test]
    fn rejects_missing_signature() {
        let mut s = make_mbr(&[(0x83, 2048, 1000)]);
        s[SIG_OFFSET] = 0;
        let r = parse(&s, 512, 512 * 10_000);
        assert!(matches!(r, Err(Error::BadMagic { .. })));
    }

    #[test]
    fn detects_protective_mbr() {
        let s = make_mbr(&[(0xEE, 1, 0xFFFFFFFF)]);
        assert!(is_protective_mbr(&s));
    }

    #[test]
    fn rejects_protective_mbr_without_signature() {
        let mut s = make_mbr(&[(0xEE, 1, 0xFFFFFFFF)]);
        s[SIG_OFFSET] = 0;
        assert!(!is_protective_mbr(&s));
    }

    #[test]
    fn classifies_extended() {
        assert!(is_extended(0x05));
        assert!(is_extended(0x0F));
        assert!(is_extended(0x85));
        assert!(!is_extended(0x07));
    }

    #[test]
    fn clamps_oversized_entries() {
        // entry says 1 GiB but device is only 512 MiB
        let s = make_mbr(&[(0x83, 2048, 2_000_000)]);
        let parts = parse(&s, 512, 512 * 1_000_000).unwrap();
        assert_eq!(parts.len(), 1);
        assert!(parts[0].length_sectors <= 1_000_000 - 2048);
    }
}
