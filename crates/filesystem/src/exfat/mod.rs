//! exFAT — boot region + directory entry parser.
//!
//! **Status: scaffold.** The boot region parser below is complete and unit-
//! tested; the directory iterator handles the most common entry types
//! (FileDirectory `0x85`, StreamExtension `0xC0`, FileName `0xC1`) but does
//! not yet walk the FAT table for files that have the `NoFatChain` flag
//! cleared. See `docs/architecture.md` §12 for the full roadmap.

use byteorder::{ByteOrder, LittleEndian};
use tr_core::{Error, Result};

#[derive(Debug, Clone)]
pub struct ExFatBoot {
    pub partition_offset_sectors: u64,
    pub volume_length_sectors: u64,
    pub fat_offset_sectors: u32,
    pub fat_length_sectors: u32,
    pub cluster_heap_offset_sectors: u32,
    pub cluster_count: u32,
    pub root_cluster: u32,
    pub bytes_per_sector_shift: u8,
    pub sectors_per_cluster_shift: u8,
    pub num_fats: u8,
}

impl ExFatBoot {
    pub fn parse(buf: &[u8], at_lba: u64) -> Result<Self> {
        if buf.len() < 512 {
            return Err(Error::UnexpectedEof {
                offset: at_lba * 512,
                need: 512,
                have: buf.len(),
            });
        }
        if &buf[3..11] != b"EXFAT   " {
            return Err(Error::BadMagic {
                offset: at_lba * 512 + 3,
                expected: "EXFAT   ",
                got: format!("{:?}", &buf[3..11]),
            });
        }
        Ok(Self {
            partition_offset_sectors: LittleEndian::read_u64(&buf[0x40..0x48]),
            volume_length_sectors: LittleEndian::read_u64(&buf[0x48..0x50]),
            fat_offset_sectors: LittleEndian::read_u32(&buf[0x50..0x54]),
            fat_length_sectors: LittleEndian::read_u32(&buf[0x54..0x58]),
            cluster_heap_offset_sectors: LittleEndian::read_u32(&buf[0x58..0x5C]),
            cluster_count: LittleEndian::read_u32(&buf[0x5C..0x60]),
            root_cluster: LittleEndian::read_u32(&buf[0x60..0x64]),
            bytes_per_sector_shift: buf[0x6C],
            sectors_per_cluster_shift: buf[0x6D],
            num_fats: buf[0x6E],
        })
    }

    #[must_use]
    pub fn bytes_per_sector(&self) -> u32 {
        1u32 << self.bytes_per_sector_shift
    }

    #[must_use]
    pub fn sectors_per_cluster(&self) -> u32 {
        1u32 << self.sectors_per_cluster_shift
    }
}

#[derive(Debug, Clone)]
pub enum ExFatDirEntry {
    File {
        in_use: bool,
        attributes: u16,
    },
    StreamExtension {
        first_cluster: u32,
        valid_data_length: u64,
        data_length: u64,
        no_fat_chain: bool,
    },
    FileName {
        chunk: String,
    },
    Other(u8),
    EndOfDirectory,
}

/// Parse a single 32-byte exFAT directory entry.
#[must_use]
pub fn parse_entry(e: &[u8; 32]) -> ExFatDirEntry {
    if e[0] == 0x00 {
        return ExFatDirEntry::EndOfDirectory;
    }
    let in_use = e[0] & 0x80 != 0;
    let ty = e[0] & 0x7F;
    match ty {
        0x05 => ExFatDirEntry::File {
            in_use,
            attributes: LittleEndian::read_u16(&e[4..6]),
        },
        0x40 => {
            let flags = e[1];
            let no_fat_chain = flags & 0x02 != 0;
            ExFatDirEntry::StreamExtension {
                first_cluster: LittleEndian::read_u32(&e[20..24]),
                valid_data_length: LittleEndian::read_u64(&e[8..16]),
                data_length: LittleEndian::read_u64(&e[24..32]),
                no_fat_chain,
            }
        }
        0x41 => {
            let mut s = String::with_capacity(15);
            for i in 0..15 {
                let p = 2 + i * 2;
                let c = LittleEndian::read_u16(&e[p..p + 2]);
                if c == 0 {
                    break;
                }
                if let Some(ch) = char::from_u32(u32::from(c)) {
                    s.push(ch);
                }
            }
            ExFatDirEntry::FileName { chunk: s }
        }
        other => ExFatDirEntry::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_boot() {
        let mut b = vec![0u8; 512];
        b[3..11].copy_from_slice(b"EXFAT   ");
        b[0x6C] = 9; // 512-byte sectors
        b[0x6D] = 3; // 8 sectors per cluster
        b[0x6E] = 2;
        let p = ExFatBoot::parse(&b, 0).unwrap();
        assert_eq!(p.bytes_per_sector(), 512);
        assert_eq!(p.sectors_per_cluster(), 8);
        assert_eq!(p.num_fats, 2);
    }

    #[test]
    fn rejects_bad_magic() {
        let b = vec![0u8; 512];
        assert!(matches!(ExFatBoot::parse(&b, 0), Err(Error::BadMagic { .. })));
    }

    #[test]
    fn parses_filename_chunk() {
        let mut e = [0u8; 32];
        e[0] = 0xC1;
        for (i, c) in "abc".encode_utf16().enumerate() {
            e[2 + i * 2] = (c & 0xFF) as u8;
            e[2 + i * 2 + 1] = (c >> 8) as u8;
        }
        if let ExFatDirEntry::FileName { chunk } = parse_entry(&e) {
            assert_eq!(chunk, "abc");
        } else {
            panic!();
        }
    }
}
