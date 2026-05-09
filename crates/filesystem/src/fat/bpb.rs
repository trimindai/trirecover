//! FAT32 BIOS Parameter Block.

use byteorder::{ByteOrder, LittleEndian};
use tr_core::{Error, Result};

#[derive(Debug, Clone)]
pub struct Fat32Bpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub total_sectors: u32,
    pub fat_size_sectors: u32,
    pub root_cluster: u32,
    pub fs_info_sector: u16,
    pub backup_boot_sector: u16,
}

impl Fat32Bpb {
    pub fn parse(buf: &[u8], at_lba: u64) -> Result<Self> {
        if buf.len() < 512 {
            return Err(Error::UnexpectedEof {
                offset: at_lba * 512,
                need: 512,
                have: buf.len(),
            });
        }
        if buf[0x1FE] != 0x55 || buf[0x1FF] != 0xAA {
            return Err(Error::BadMagic {
                offset: at_lba * 512 + 0x1FE,
                expected: "55 AA",
                got: format!("{:02X} {:02X}", buf[0x1FE], buf[0x1FF]),
            });
        }
        if &buf[0x52..0x5A] != b"FAT32   " {
            return Err(Error::BadMagic {
                offset: at_lba * 512 + 0x52,
                expected: "FAT32   ",
                got: format!("{:?}", &buf[0x52..0x5A]),
            });
        }
        let bps = LittleEndian::read_u16(&buf[0x0B..0x0D]);
        let spc = buf[0x0D];
        if bps == 0 || (bps & (bps - 1)) != 0 {
            return Err(Error::corrupt("fat32_bpb", at_lba * 512, "bad bytes_per_sector"));
        }
        if spc == 0 || (spc & (spc - 1)) != 0 {
            return Err(Error::corrupt(
                "fat32_bpb",
                at_lba * 512,
                "sectors_per_cluster not power of two",
            ));
        }
        let reserved = LittleEndian::read_u16(&buf[0x0E..0x10]);
        let num_fats = buf[0x10];
        let total_sectors = LittleEndian::read_u32(&buf[0x20..0x24]);
        let fat_size = LittleEndian::read_u32(&buf[0x24..0x28]);
        let root_cluster = LittleEndian::read_u32(&buf[0x2C..0x30]);
        let fs_info = LittleEndian::read_u16(&buf[0x30..0x32]);
        let backup = LittleEndian::read_u16(&buf[0x32..0x34]);

        if num_fats == 0 || fat_size == 0 || root_cluster < 2 {
            return Err(Error::corrupt(
                "fat32_bpb",
                at_lba * 512,
                "invalid FAT layout fields",
            ));
        }

        Ok(Self {
            bytes_per_sector: bps,
            sectors_per_cluster: spc,
            reserved_sectors: reserved,
            num_fats,
            total_sectors,
            fat_size_sectors: fat_size,
            root_cluster,
            fs_info_sector: fs_info,
            backup_boot_sector: backup,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bpb() -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        LittleEndian::write_u16(&mut b[0x0B..0x0D], 512);
        b[0x0D] = 8;
        LittleEndian::write_u16(&mut b[0x0E..0x10], 32);
        b[0x10] = 2;
        LittleEndian::write_u32(&mut b[0x20..0x24], 1_000_000);
        LittleEndian::write_u32(&mut b[0x24..0x28], 1024);
        LittleEndian::write_u32(&mut b[0x2C..0x30], 2);
        LittleEndian::write_u16(&mut b[0x30..0x32], 1);
        LittleEndian::write_u16(&mut b[0x32..0x34], 6);
        b[0x52..0x5A].copy_from_slice(b"FAT32   ");
        b[0x1FE] = 0x55;
        b[0x1FF] = 0xAA;
        b
    }

    #[test]
    fn parses_valid() {
        let b = Fat32Bpb::parse(&make_bpb(), 0).unwrap();
        assert_eq!(b.bytes_per_sector, 512);
        assert_eq!(b.sectors_per_cluster, 8);
        assert_eq!(b.fat_size_sectors, 1024);
        assert_eq!(b.root_cluster, 2);
    }

    #[test]
    fn rejects_missing_fat32_id() {
        let mut b = make_bpb();
        b[0x52] = b'X';
        assert!(matches!(Fat32Bpb::parse(&b, 0), Err(Error::BadMagic { .. })));
    }
}
