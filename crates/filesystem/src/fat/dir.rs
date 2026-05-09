//! FAT32 directory entry parser, including LFN reassembly.
//!
//! Directory entries are 32 bytes each. A long filename is stored across one
//! or more LFN entries (attribute 0x0F) that immediately precede the short
//! 8.3 entry, in reverse order. Deleted entries have first byte 0xE5.

use byteorder::{ByteOrder, LittleEndian};

const ENTRY_SIZE: usize = 32;
const ATTR_LFN: u8 = 0x0F;
const ATTR_VOLUME: u8 = 0x08;
const ATTR_DIR: u8 = 0x10;
const DELETED_MARKER: u8 = 0xE5;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub size_bytes: u32,
    pub first_cluster: u32,
    pub is_deleted: bool,
}

#[derive(Debug, Clone)]
pub struct SubdirEntry {
    pub name: String,
    pub first_cluster: u32,
}

#[derive(Debug, Clone)]
pub enum FatDirEntry {
    File(FileEntry),
    Subdir(SubdirEntry),
    Volume(String),
    DeletedRaw(DirEntryRaw),
    EndOfDirectory,
}

#[derive(Debug, Clone)]
pub struct DirEntryRaw {
    pub bytes: [u8; ENTRY_SIZE],
}

#[derive(Debug)]
pub struct DirIter<'a> {
    buf: &'a [u8],
    cursor: usize,
    pending_lfn: Vec<[u16; 13]>,
    pending_lfn_seq: Vec<u8>,
}

impl<'a> DirIter<'a> {
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            cursor: 0,
            pending_lfn: Vec::new(),
            pending_lfn_seq: Vec::new(),
        }
    }

    pub fn next_entry(&mut self) -> Option<FatDirEntry> {
        loop {
            if self.cursor + ENTRY_SIZE > self.buf.len() {
                return None;
            }
            let e = &self.buf[self.cursor..self.cursor + ENTRY_SIZE];
            self.cursor += ENTRY_SIZE;

            if e[0] == 0x00 {
                return Some(FatDirEntry::EndOfDirectory);
            }

            let attr = e[0x0B];

            if attr == ATTR_LFN {
                // LFN entry — accumulate. If first byte is 0xE5, the SFN entry
                // is deleted; we still try to recover the long name.
                let seq_raw = e[0x00];
                let mut chars = [0u16; 13];
                let positions = [
                    (0x01, 5),
                    (0x0E, 6),
                    (0x1C, 2),
                ];
                let mut idx = 0;
                for (off, count) in positions {
                    for i in 0..count {
                        let p = off + i * 2;
                        chars[idx] = LittleEndian::read_u16(&e[p..p + 2]);
                        idx += 1;
                    }
                }
                self.pending_lfn.push(chars);
                self.pending_lfn_seq.push(seq_raw);
                continue;
            }

            // SFN (8.3) entry.
            let is_deleted = e[0] == DELETED_MARKER;
            let first_cluster_hi = u32::from(LittleEndian::read_u16(&e[0x14..0x16]));
            let first_cluster_lo = u32::from(LittleEndian::read_u16(&e[0x1A..0x1C]));
            let first_cluster = (first_cluster_hi << 16) | first_cluster_lo;
            let size_bytes = LittleEndian::read_u32(&e[0x1C..0x20]);

            let name = self.consume_name(e, is_deleted);
            self.pending_lfn.clear();
            self.pending_lfn_seq.clear();

            if attr & ATTR_VOLUME != 0 && attr & ATTR_DIR == 0 {
                return Some(FatDirEntry::Volume(name));
            }
            if attr & ATTR_DIR != 0 {
                return Some(FatDirEntry::Subdir(SubdirEntry {
                    name,
                    first_cluster,
                }));
            }
            return Some(FatDirEntry::File(FileEntry {
                name,
                size_bytes,
                first_cluster,
                is_deleted,
            }));
        }
    }

    /// Reconstruct the filename from any pending LFN entries; otherwise fall
    /// back to the 8.3 short name.
    fn consume_name(&self, sfn: &[u8], is_deleted: bool) -> String {
        if !self.pending_lfn.is_empty() {
            // pending entries are in reverse order
            let mut name = String::new();
            for chars in self.pending_lfn.iter().rev() {
                for &c in chars {
                    if c == 0 || c == 0xFFFF {
                        return name;
                    }
                    name.push(char::from_u32(u32::from(c)).unwrap_or('?'));
                }
            }
            return name;
        }
        // Recover deleted SFN by replacing 0xE5 with '_' so the user can rename
        let first = if is_deleted { b'_' } else { sfn[0] };
        let mut base = String::new();
        let bytes: Vec<u8> = std::iter::once(first).chain(sfn[1..8].iter().copied()).collect();
        for b in bytes {
            if b == b' ' {
                break;
            }
            base.push(b as char);
        }
        let mut ext = String::new();
        for b in &sfn[8..11] {
            if *b == b' ' {
                break;
            }
            ext.push(*b as char);
        }
        if ext.is_empty() {
            base
        } else {
            format!("{base}.{ext}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sfn(name: &[u8; 11], attr: u8, size: u32, cluster: u32, deleted: bool) -> [u8; 32] {
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(name);
        if deleted {
            e[0] = DELETED_MARKER;
        }
        e[0x0B] = attr;
        LittleEndian::write_u16(&mut e[0x14..0x16], (cluster >> 16) as u16);
        LittleEndian::write_u16(&mut e[0x1A..0x1C], (cluster & 0xFFFF) as u16);
        LittleEndian::write_u32(&mut e[0x1C..0x20], size);
        e
    }

    #[test]
    fn parses_simple_sfn() {
        let e = make_sfn(b"HELLO   TXT", 0x20, 11, 5, false);
        let iter_buf = e.to_vec();
        let mut it = DirIter::new(&iter_buf);
        let entry = it.next_entry().unwrap();
        if let FatDirEntry::File(f) = entry {
            assert_eq!(f.name, "HELLO.TXT");
            assert_eq!(f.size_bytes, 11);
            assert_eq!(f.first_cluster, 5);
            assert!(!f.is_deleted);
        } else {
            panic!("not a file entry");
        }
    }

    #[test]
    fn detects_deleted_entry() {
        let e = make_sfn(b"GONE    BIN", 0x20, 5, 9, true);
        let buf = e.to_vec();
        let mut it = DirIter::new(&buf);
        let entry = it.next_entry().unwrap();
        if let FatDirEntry::File(f) = entry {
            assert!(f.is_deleted);
            assert!(f.name.starts_with('_'));
        } else {
            panic!();
        }
    }

    #[test]
    fn end_of_dir_signal() {
        let buf = vec![0u8; 32];
        let mut it = DirIter::new(&buf);
        assert!(matches!(it.next_entry(), Some(FatDirEntry::EndOfDirectory)));
    }
}
