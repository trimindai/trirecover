//! NTFS boot sector (BIOS Parameter Block + NTFS extension).
//!
//! References: Microsoft NTFS specification (publicly documented), and the
//! linux-ntfs project's wiki (https://flatcap.github.io/linux-ntfs/ntfs/).

use byteorder::{ByteOrder, LittleEndian};
use tr_core::{Error, Result};

#[derive(Debug, Clone)]
pub struct NtfsBoot {
    /// Logical sector size (almost always 512 or 4096).
    pub bytes_per_sector: u16,
    /// Sectors per cluster.
    pub sectors_per_cluster: u8,
    /// Total sectors on the volume.
    pub total_sectors: u64,
    /// LCN (cluster offset within the volume) of the $MFT.
    pub mft_lcn: u64,
    /// LCN of the $MFTMirr (mirror used for recovery).
    pub mftmirr_lcn: u64,
    /// Encoded MFT record size:
    /// - positive value n  : n clusters per record
    /// - negative value n  : 2^|n| bytes per record
    pub mft_record_size_encoded: i8,
    /// Encoded index buffer size, same convention as the MFT field.
    pub index_buffer_size_encoded: i8,
    /// Volume serial number.
    pub volume_serial: u64,
}

impl NtfsBoot {
    pub fn parse(buf: &[u8], at_lba: u64) -> Result<Self> {
        if buf.len() < 512 {
            return Err(Error::UnexpectedEof {
                offset: at_lba * 512,
                need: 512,
                have: buf.len(),
            });
        }
        if &buf[3..11] != b"NTFS    " {
            return Err(Error::BadMagic {
                offset: at_lba * 512 + 3,
                expected: "NTFS    ",
                got: format!("{:?}", &buf[3..11]),
            });
        }
        let bps = LittleEndian::read_u16(&buf[0x0B..0x0D]);
        if bps == 0 || bps > 8192 || (bps & (bps - 1)) != 0 {
            return Err(Error::corrupt(
                "ntfs_boot",
                at_lba * 512 + 0x0B,
                format!("implausible bytes_per_sector {bps}"),
            ));
        }
        let spc = buf[0x0D];
        if spc == 0 {
            return Err(Error::corrupt(
                "ntfs_boot",
                at_lba * 512 + 0x0D,
                "sectors_per_cluster = 0",
            ));
        }
        // NTFS-specific fields start at 0x28
        let total_sectors = LittleEndian::read_u64(&buf[0x28..0x30]);
        let mft_lcn = LittleEndian::read_u64(&buf[0x30..0x38]);
        let mftmirr_lcn = LittleEndian::read_u64(&buf[0x38..0x40]);
        let mft_record_size_encoded = buf[0x40] as i8;
        let index_buffer_size_encoded = buf[0x44] as i8;
        let volume_serial = LittleEndian::read_u64(&buf[0x48..0x50]);

        if buf[0x1FE] != 0x55 || buf[0x1FF] != 0xAA {
            return Err(Error::BadMagic {
                offset: at_lba * 512 + 0x1FE,
                expected: "55 AA",
                got: format!("{:02X} {:02X}", buf[0x1FE], buf[0x1FF]),
            });
        }

        Ok(Self {
            bytes_per_sector: bps,
            sectors_per_cluster: spc,
            total_sectors,
            mft_lcn,
            mftmirr_lcn,
            mft_record_size_encoded,
            index_buffer_size_encoded,
            volume_serial,
        })
    }

    /// Bytes per cluster = bytes_per_sector × sectors_per_cluster.
    #[must_use]
    pub fn cluster_bytes(&self) -> u32 {
        u32::from(self.bytes_per_sector) * u32::from(self.sectors_per_cluster)
    }

    /// Decoded MFT record size in bytes.
    #[must_use]
    pub fn mft_record_bytes(&self) -> u32 {
        decode_size(self.mft_record_size_encoded, self.cluster_bytes())
    }

    /// Decoded index buffer size in bytes.
    #[must_use]
    pub fn index_buffer_bytes(&self) -> u32 {
        decode_size(self.index_buffer_size_encoded, self.cluster_bytes())
    }
}

fn decode_size(encoded: i8, cluster_bytes: u32) -> u32 {
    if encoded >= 0 {
        u32::from(encoded as u8) * cluster_bytes
    } else {
        // 2^|encoded|
        let exp = (-i32::from(encoded)) as u32;
        if exp >= 32 {
            return 0;
        }
        1u32 << exp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_boot() -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[0..3].copy_from_slice(&[0xEB, 0x52, 0x90]);
        b[3..11].copy_from_slice(b"NTFS    ");
        LittleEndian::write_u16(&mut b[0x0B..0x0D], 512);
        b[0x0D] = 8; // 8 sectors per cluster -> 4 KiB
        LittleEndian::write_u64(&mut b[0x28..0x30], 200_000);
        LittleEndian::write_u64(&mut b[0x30..0x38], 4);
        LittleEndian::write_u64(&mut b[0x38..0x40], 100);
        b[0x40] = 0xF6_u8 as u8; // -10 → 1024 bytes (default)
        b[0x44] = 0x01;
        LittleEndian::write_u64(&mut b[0x48..0x50], 0xDEAD_BEEF_CAFE_BABE);
        b[0x1FE] = 0x55;
        b[0x1FF] = 0xAA;
        b
    }

    #[test]
    fn parses_typical_boot() {
        let b = NtfsBoot::parse(&make_boot(), 0).unwrap();
        assert_eq!(b.bytes_per_sector, 512);
        assert_eq!(b.sectors_per_cluster, 8);
        assert_eq!(b.cluster_bytes(), 4096);
        assert_eq!(b.mft_lcn, 4);
        assert_eq!(b.mft_record_bytes(), 1024);
        assert_eq!(b.index_buffer_bytes(), 4096);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut b = make_boot();
        b[3] = b'X';
        let r = NtfsBoot::parse(&b, 0);
        assert!(matches!(r, Err(Error::BadMagic { .. })));
    }

    #[test]
    fn rejects_implausible_sector_size() {
        let mut b = make_boot();
        LittleEndian::write_u16(&mut b[0x0B..0x0D], 333); // not power of two
        let r = NtfsBoot::parse(&b, 0);
        assert!(matches!(r, Err(Error::Corrupt { .. })));
    }
}
