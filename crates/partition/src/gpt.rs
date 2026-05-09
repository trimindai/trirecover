//! GPT (GUID Partition Table) parser.
//!
//! Layout:
//! - LBA 0      : protective MBR (handled by `mbr::is_protective_mbr`)
//! - LBA 1      : primary GPT header (92 bytes + zero pad to sector size)
//! - LBA 2..    : partition entry array (size from header)
//! - LBA -1     : backup GPT header
//! - LBA -33    : backup partition entry array
//!
//! Header layout (offsets within the header buffer):
//! - 0x00..0x08 : "EFI PART"
//! - 0x08..0x0C : revision (LE u32)
//! - 0x0C..0x10 : header size in bytes (LE u32, usually 92)
//! - 0x10..0x14 : header CRC32
//! - 0x14..0x18 : reserved (must be zero)
//! - 0x18..0x20 : current LBA
//! - 0x20..0x28 : backup LBA
//! - 0x28..0x30 : first usable LBA
//! - 0x30..0x38 : last usable LBA
//! - 0x38..0x48 : disk GUID (16 bytes, mixed-endian per UEFI)
//! - 0x48..0x50 : partition entry array start LBA
//! - 0x50..0x54 : number of entries
//! - 0x54..0x58 : size of each entry
//! - 0x58..0x5C : partition entry array CRC32
//!
//! Entry layout (typically 128 bytes):
//! - 0x00..0x10 : type GUID
//! - 0x10..0x20 : unique GUID
//! - 0x20..0x28 : starting LBA
//! - 0x28..0x30 : ending LBA (inclusive)
//! - 0x30..0x38 : attributes
//! - 0x38..0x72 : name (UTF-16LE, 36 chars)

use byteorder::{ByteOrder, LittleEndian};
use crc::{Crc, CRC_32_ISO_HDLC};
use tr_core::{Error, PartitionInfo, PartitionScheme, Result};
use uuid::Uuid;

const SIGNATURE: &[u8; 8] = b"EFI PART";
const HEADER_SIZE_FIXED: u32 = 92;
const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

#[derive(Debug, Clone)]
pub struct GptHeader {
    pub revision: u32,
    pub header_size: u32,
    pub current_lba: u64,
    pub backup_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub disk_guid: Uuid,
    pub entry_array_lba: u64,
    pub num_entries: u32,
    pub entry_size: u32,
    pub entry_array_crc32: u32,
}

/// Parse a GPT table given a buffer that contains LBA 0..=N (where N is large
/// enough to hold the header at LBA 1 plus the entry array starting at the
/// LBA the header points to).
pub fn parse(buffer: &[u8], sector_size: u32) -> Result<Vec<PartitionInfo>> {
    let ssz = sector_size as usize;
    if ssz == 0 || buffer.len() < ssz * 2 {
        return Err(Error::UnexpectedEof {
            offset: 0,
            need: ssz * 2,
            have: buffer.len(),
        });
    }
    let header_buf = &buffer[ssz..ssz * 2];
    let header = parse_header(header_buf, 1, ssz as u64)?;

    // Find the entry array within the buffer if possible.
    let array_start = (header.entry_array_lba as usize)
        .checked_mul(ssz)
        .ok_or_else(|| Error::corrupt("gpt", 0, "entry array LBA overflow"))?;
    let array_len = (header.num_entries as usize)
        .checked_mul(header.entry_size as usize)
        .ok_or_else(|| Error::corrupt("gpt", 0, "entry array size overflow"))?;

    if array_start + array_len > buffer.len() {
        return Err(Error::UnexpectedEof {
            offset: array_start as u64,
            need: array_len,
            have: buffer.len().saturating_sub(array_start),
        });
    }
    let array = &buffer[array_start..array_start + array_len];

    // Validate entry array CRC32.
    let actual = CRC32.checksum(array);
    if actual != header.entry_array_crc32 {
        return Err(Error::BadCrc {
            offset: array_start as u64,
            expected: header.entry_array_crc32,
            got: actual,
        });
    }

    let mut parts = Vec::new();
    for i in 0..header.num_entries {
        let off = (i as usize) * (header.entry_size as usize);
        let entry = &array[off..off + header.entry_size as usize];
        if let Some(p) = parse_entry(i, entry, sector_size)? {
            parts.push(p);
        }
    }
    Ok(parts)
}

pub fn parse_header(buf: &[u8], lba: u64, sector_size: u64) -> Result<GptHeader> {
    if buf.len() < HEADER_SIZE_FIXED as usize {
        return Err(Error::UnexpectedEof {
            offset: lba * sector_size,
            need: HEADER_SIZE_FIXED as usize,
            have: buf.len(),
        });
    }
    if &buf[..8] != SIGNATURE {
        return Err(Error::BadMagic {
            offset: lba * sector_size,
            expected: "EFI PART",
            got: format!("{:?}", &buf[..8]),
        });
    }
    let revision = LittleEndian::read_u32(&buf[0x08..0x0C]);
    let header_size = LittleEndian::read_u32(&buf[0x0C..0x10]);
    let stored_crc = LittleEndian::read_u32(&buf[0x10..0x14]);
    let reserved = LittleEndian::read_u32(&buf[0x14..0x18]);
    if reserved != 0 {
        return Err(Error::corrupt(
            "gpt_header",
            lba * sector_size,
            "reserved field nonzero",
        ));
    }
    if header_size < HEADER_SIZE_FIXED || header_size as usize > buf.len() {
        return Err(Error::corrupt(
            "gpt_header",
            lba * sector_size,
            format!("implausible header size {header_size}"),
        ));
    }

    // CRC excludes itself: zero out 0x10..0x14 in a copy
    let mut tmp = buf[..header_size as usize].to_vec();
    tmp[0x10..0x14].fill(0);
    let actual = CRC32.checksum(&tmp);
    if actual != stored_crc {
        return Err(Error::BadCrc {
            offset: lba * sector_size + 0x10,
            expected: stored_crc,
            got: actual,
        });
    }

    let current_lba = LittleEndian::read_u64(&buf[0x18..0x20]);
    let backup_lba = LittleEndian::read_u64(&buf[0x20..0x28]);
    let first_usable = LittleEndian::read_u64(&buf[0x28..0x30]);
    let last_usable = LittleEndian::read_u64(&buf[0x30..0x38]);
    let disk_guid = read_guid(&buf[0x38..0x48]);
    let entry_lba = LittleEndian::read_u64(&buf[0x48..0x50]);
    let num_entries = LittleEndian::read_u32(&buf[0x50..0x54]);
    let entry_size = LittleEndian::read_u32(&buf[0x54..0x58]);
    let entry_crc = LittleEndian::read_u32(&buf[0x58..0x5C]);

    if entry_size < 128 || entry_size > 4096 || (entry_size & (entry_size - 1)) != 0 {
        return Err(Error::corrupt(
            "gpt_header",
            lba * sector_size,
            format!("entry_size {entry_size} not a power of two ≥128"),
        ));
    }
    if num_entries > 1024 {
        return Err(Error::corrupt(
            "gpt_header",
            lba * sector_size,
            format!("num_entries {num_entries} exceeds sane limit"),
        ));
    }

    Ok(GptHeader {
        revision,
        header_size,
        current_lba,
        backup_lba,
        first_usable_lba: first_usable,
        last_usable_lba: last_usable,
        disk_guid,
        entry_array_lba: entry_lba,
        num_entries,
        entry_size,
        entry_array_crc32: entry_crc,
    })
}

fn parse_entry(index: u32, entry: &[u8], sector_size: u32) -> Result<Option<PartitionInfo>> {
    if entry.len() < 128 {
        return Err(Error::UnexpectedEof {
            offset: 0,
            need: 128,
            have: entry.len(),
        });
    }
    let type_guid = read_guid(&entry[0x00..0x10]);
    if type_guid.is_nil() {
        return Ok(None); // empty slot
    }
    let _unique_guid = read_guid(&entry[0x10..0x20]);
    let start = LittleEndian::read_u64(&entry[0x20..0x28]);
    let end_inclusive = LittleEndian::read_u64(&entry[0x28..0x30]);
    let attrs = LittleEndian::read_u64(&entry[0x30..0x38]);
    let _ = attrs;

    let length = end_inclusive
        .checked_sub(start)
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| Error::corrupt("gpt_entry", 0, "end < start"))?;

    let name = read_utf16_name(&entry[0x38..0x72]);

    Ok(Some(PartitionInfo {
        index,
        scheme: PartitionScheme::Gpt,
        type_id: type_guid.to_string(),
        name: if name.is_empty() { None } else { Some(name) },
        filesystem: gpt_type_to_fs_hint(&type_guid).map(str::to_string),
        start_lba: start,
        length_sectors: length,
        sector_size,
        reconstructed: false,
    }))
}

/// GPT GUIDs are stored mixed-endian: first three groups little-endian,
/// last two groups big-endian. UEFI section 5.3.3.
fn read_guid(bytes: &[u8]) -> Uuid {
    debug_assert!(bytes.len() >= 16);
    let d1 = LittleEndian::read_u32(&bytes[0..4]);
    let d2 = LittleEndian::read_u16(&bytes[4..6]);
    let d3 = LittleEndian::read_u16(&bytes[6..8]);
    let d4: [u8; 8] = bytes[8..16].try_into().unwrap_or_default();
    Uuid::from_fields(d1, d2, d3, &d4)
}

fn read_utf16_name(bytes: &[u8]) -> String {
    let mut chars = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let v = u16::from_le_bytes([chunk[0], chunk[1]]);
        if v == 0 {
            break;
        }
        chars.push(v);
    }
    String::from_utf16_lossy(&chars)
}

/// Map well-known type GUIDs to filesystem hints. The list is intentionally
/// small — full filesystem detection happens in `tr-filesystem` by sniffing
/// the boot sector.
fn gpt_type_to_fs_hint(g: &Uuid) -> Option<&'static str> {
    let s = g.to_string().to_lowercase();
    match s.as_str() {
        "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7" => Some("ntfs-or-fat"), // Windows basic data
        "c12a7328-f81f-11d2-ba4b-00a0c93ec93b" => Some("fat32"),       // EFI System
        "0fc63daf-8483-4772-8e79-3d69d8477de4" => Some("ext4-or-other"), // Linux fs data
        "21686148-6449-6e6f-744e-656564454649" => Some("biosboot"),
        "a2a0d0eb-e5b9-3344-87c0-68b6b72699c7" => Some("ntfs-or-fat"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal but valid GPT (one partition) for round-trip tests.
    fn build_fixture() -> (Vec<u8>, u32) {
        let ssz: usize = 512;
        let mut buf = vec![0u8; ssz * 34];

        // Protective MBR — not strictly required for `parse_header`, but
        // useful for top-level integration.
        buf[0x1FE] = 0x55;
        buf[0x1FF] = 0xAA;
        buf[0x1BE + 4] = 0xEE;

        // Partition entry array at LBA 2 — one NTFS-typed partition.
        let entry_lba: u64 = 2;
        let num_entries: u32 = 128;
        let entry_size: u32 = 128;
        let array_off = (entry_lba as usize) * ssz;
        let array_len = (num_entries as usize) * (entry_size as usize);

        // First entry: Microsoft Basic Data
        let type_guid_bytes: [u8; 16] = [
            0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26,
            0x99, 0xC7,
        ];
        buf[array_off..array_off + 16].copy_from_slice(&type_guid_bytes);
        let unique = [0x11u8; 16];
        buf[array_off + 16..array_off + 32].copy_from_slice(&unique);
        LittleEndian::write_u64(&mut buf[array_off + 32..array_off + 40], 2048);
        LittleEndian::write_u64(&mut buf[array_off + 40..array_off + 48], 102_047);
        // Name: "Data" UTF-16LE
        let name = "Data".encode_utf16().collect::<Vec<u16>>();
        for (i, c) in name.iter().enumerate() {
            buf[array_off + 56 + i * 2..array_off + 56 + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }

        let entry_array_crc = CRC32.checksum(&buf[array_off..array_off + array_len]);

        // Header at LBA 1
        let h = ssz; // header offset
        buf[h..h + 8].copy_from_slice(SIGNATURE);
        LittleEndian::write_u32(&mut buf[h + 0x08..h + 0x0C], 0x00010000);
        LittleEndian::write_u32(&mut buf[h + 0x0C..h + 0x10], HEADER_SIZE_FIXED);
        // 0x10..0x14 = header CRC, set to 0 then computed
        LittleEndian::write_u32(&mut buf[h + 0x10..h + 0x14], 0);
        LittleEndian::write_u32(&mut buf[h + 0x14..h + 0x18], 0);
        LittleEndian::write_u64(&mut buf[h + 0x18..h + 0x20], 1);
        LittleEndian::write_u64(&mut buf[h + 0x20..h + 0x28], 200_000);
        LittleEndian::write_u64(&mut buf[h + 0x28..h + 0x30], 34);
        LittleEndian::write_u64(&mut buf[h + 0x30..h + 0x38], 199_966);
        // disk_guid arbitrary
        for i in 0..16 {
            buf[h + 0x38 + i] = i as u8;
        }
        LittleEndian::write_u64(&mut buf[h + 0x48..h + 0x50], entry_lba);
        LittleEndian::write_u32(&mut buf[h + 0x50..h + 0x54], num_entries);
        LittleEndian::write_u32(&mut buf[h + 0x54..h + 0x58], entry_size);
        LittleEndian::write_u32(&mut buf[h + 0x58..h + 0x5C], entry_array_crc);

        // header CRC
        let header_crc = CRC32.checksum(&buf[h..h + HEADER_SIZE_FIXED as usize]);
        LittleEndian::write_u32(&mut buf[h + 0x10..h + 0x14], header_crc);

        (buf, ssz as u32)
    }

    #[test]
    fn parses_header_round_trip() {
        let (buf, ssz) = build_fixture();
        let h = parse_header(&buf[ssz as usize..ssz as usize * 2], 1, u64::from(ssz)).unwrap();
        assert_eq!(h.revision, 0x00010000);
        assert_eq!(h.entry_array_lba, 2);
        assert_eq!(h.num_entries, 128);
        assert_eq!(h.entry_size, 128);
    }

    #[test]
    fn parses_partition() {
        let (buf, ssz) = build_fixture();
        let parts = parse(&buf, ssz).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].start_lba, 2048);
        assert_eq!(parts[0].length_sectors, 100_000);
        assert_eq!(parts[0].name.as_deref(), Some("Data"));
        assert_eq!(parts[0].scheme, PartitionScheme::Gpt);
    }

    #[test]
    fn rejects_bad_header_crc() {
        let (mut buf, ssz) = build_fixture();
        // flip a byte in the header
        buf[ssz as usize + 0x20] ^= 0xFF;
        let r = parse_header(&buf[ssz as usize..ssz as usize * 2], 1, u64::from(ssz));
        assert!(matches!(r, Err(Error::BadCrc { .. })));
    }

    #[test]
    fn rejects_bad_signature() {
        let (mut buf, ssz) = build_fixture();
        buf[ssz as usize] = 0;
        let r = parse_header(&buf[ssz as usize..ssz as usize * 2], 1, u64::from(ssz));
        assert!(matches!(r, Err(Error::BadMagic { .. })));
    }
}
