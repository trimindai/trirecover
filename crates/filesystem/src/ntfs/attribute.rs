//! NTFS attribute parser (`$STANDARD_INFORMATION`, `$FILE_NAME`, `$DATA`,
//! plus a generic catch-all). Resident and non-resident layouts are both
//! handled.

use crate::ntfs::runlist::{self, RawRun};
use byteorder::{ByteOrder, LittleEndian};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use tr_core::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum AttributeType {
    StandardInformation = 0x10,
    AttributeList = 0x20,
    FileName = 0x30,
    ObjectId = 0x40,
    SecurityDescriptor = 0x50,
    VolumeName = 0x60,
    VolumeInformation = 0x70,
    Data = 0x80,
    IndexRoot = 0x90,
    IndexAllocation = 0xA0,
    Bitmap = 0xB0,
    ReparsePoint = 0xC0,
    EaInformation = 0xD0,
    Ea = 0xE0,
    LoggedUtilityStream = 0x100,
    Other(u32),
}

impl AttributeType {
    /// Reverse of `from_u32` — needed because the `Other(u32)` variant means
    /// the enum is not field-less and so `as u32` is rejected by the compiler.
    #[must_use]
    pub fn to_u32(self) -> u32 {
        match self {
            Self::StandardInformation => 0x10,
            Self::AttributeList => 0x20,
            Self::FileName => 0x30,
            Self::ObjectId => 0x40,
            Self::SecurityDescriptor => 0x50,
            Self::VolumeName => 0x60,
            Self::VolumeInformation => 0x70,
            Self::Data => 0x80,
            Self::IndexRoot => 0x90,
            Self::IndexAllocation => 0xA0,
            Self::Bitmap => 0xB0,
            Self::ReparsePoint => 0xC0,
            Self::EaInformation => 0xD0,
            Self::Ea => 0xE0,
            Self::LoggedUtilityStream => 0x100,
            Self::Other(v) => v,
        }
    }

    fn from_u32(v: u32) -> Self {
        match v {
            0x10 => Self::StandardInformation,
            0x20 => Self::AttributeList,
            0x30 => Self::FileName,
            0x40 => Self::ObjectId,
            0x50 => Self::SecurityDescriptor,
            0x60 => Self::VolumeName,
            0x70 => Self::VolumeInformation,
            0x80 => Self::Data,
            0x90 => Self::IndexRoot,
            0xA0 => Self::IndexAllocation,
            0xB0 => Self::Bitmap,
            0xC0 => Self::ReparsePoint,
            0xD0 => Self::EaInformation,
            0xE0 => Self::Ea,
            0x100 => Self::LoggedUtilityStream,
            other => Self::Other(other),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub kind: AttributeType,
    pub name: String,
    pub flags: u16,
    pub attribute_id: u16,
    pub non_resident: bool,
    pub raw: Vec<u8>,
    /// Resident layout fields (None if non-resident).
    pub resident: Option<ResidentLayout>,
    /// Non-resident layout fields (None if resident).
    pub non_resident_layout: Option<NonResidentLayout>,
}

#[derive(Debug, Clone)]
pub struct ResidentLayout {
    pub value_length: u32,
    pub value_offset: u16,
}

/// One decoded `$FILE_NAME` value. A single MFT record can have multiple of
/// these (one per namespace, plus extras for hardlinks pointing at different
/// parents) — see [`crate::ntfs::mft::MftRecord::best_filename`].
#[derive(Debug, Clone)]
pub struct FileName {
    /// Parent directory's MFT record number (low 48 bits of the MFT_REF).
    pub parent_record: u64,
    /// 0=POSIX, 1=Win32, 2=DOS, 3=Win32+DOS combined.
    pub namespace: u8,
    /// UTF-16-decoded leaf name.
    pub name: String,
}

/// One decoded `$ATTRIBUTE_LIST` entry. A base MFT record uses an
/// $ATTRIBUTE_LIST when its own attributes will not fit (typically heavy
/// fragmentation or many alternate data streams). Each entry tells us where
/// to find one of the file's actual attributes — usually in an "extension"
/// MFT record whose number lives in [`Self::mft_record`].
///
/// Layout per entry (variable length, min 0x1A bytes):
/// - 0x00 u32 attribute type
/// - 0x04 u16 record length (whole entry incl. trailing UTF-16 name)
/// - 0x06 u8  name length (in UTF-16 chars)
/// - 0x07 u8  name offset (within the entry)
/// - 0x08 u64 starting VCN (for non-resident attrs; 0 otherwise)
/// - 0x10 u64 MFT_REF — low 48 bits = record number, high 16 bits = sequence
/// - 0x18 u16 attribute id
/// - 0x1A    UTF-16 name (if any)
#[derive(Debug, Clone)]
pub struct AttributeListEntry {
    pub kind: AttributeType,
    pub name: String,
    pub starting_vcn: u64,
    /// MFT record holding the actual attribute (low 48 bits of MFT_REF).
    pub mft_record: u64,
    pub mft_sequence: u16,
    pub attribute_id: u16,
}

#[derive(Debug, Clone)]
pub struct NonResidentLayout {
    pub starting_vcn: u64,
    pub last_vcn: u64,
    pub run_list_offset: u16,
    pub allocated_size: u64,
    pub real_size: u64,
    pub initialised_size: u64,
}

impl Attribute {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < 16 {
            return Err(Error::UnexpectedEof {
                offset: 0,
                need: 16,
                have: buf.len(),
            });
        }
        let ty = LittleEndian::read_u32(&buf[0..4]);
        let kind = AttributeType::from_u32(ty);
        let length = LittleEndian::read_u32(&buf[4..8]) as usize;
        if length < 16 || length > buf.len() {
            return Err(Error::corrupt(
                "ntfs_attr",
                0,
                format!("invalid attribute length {length}"),
            ));
        }
        let non_resident = buf[8] != 0;
        let name_len = buf[9] as usize;
        let name_off = LittleEndian::read_u16(&buf[10..12]) as usize;
        let flags = LittleEndian::read_u16(&buf[12..14]);
        let attribute_id = LittleEndian::read_u16(&buf[14..16]);

        let name = if name_len == 0 {
            String::new()
        } else if name_off + name_len * 2 > buf.len() {
            String::new()
        } else {
            let mut codes = Vec::with_capacity(name_len);
            for i in 0..name_len {
                codes.push(LittleEndian::read_u16(&buf[name_off + i * 2..name_off + i * 2 + 2]));
            }
            String::from_utf16_lossy(&codes)
        };

        let (resident, non_resident_layout) = if non_resident {
            if buf.len() < 64 {
                return Err(Error::UnexpectedEof {
                    offset: 0,
                    need: 64,
                    have: buf.len(),
                });
            }
            let starting_vcn = LittleEndian::read_u64(&buf[16..24]);
            let last_vcn = LittleEndian::read_u64(&buf[24..32]);
            let run_list_offset = LittleEndian::read_u16(&buf[32..34]);
            let allocated_size = LittleEndian::read_u64(&buf[40..48]);
            let real_size = LittleEndian::read_u64(&buf[48..56]);
            let initialised_size = LittleEndian::read_u64(&buf[56..64]);
            (
                None,
                Some(NonResidentLayout {
                    starting_vcn,
                    last_vcn,
                    run_list_offset,
                    allocated_size,
                    real_size,
                    initialised_size,
                }),
            )
        } else {
            if buf.len() < 24 {
                return Err(Error::UnexpectedEof {
                    offset: 0,
                    need: 24,
                    have: buf.len(),
                });
            }
            let value_length = LittleEndian::read_u32(&buf[16..20]);
            let value_offset = LittleEndian::read_u16(&buf[20..22]);
            (
                Some(ResidentLayout {
                    value_length,
                    value_offset,
                }),
                None,
            )
        };

        Ok(Self {
            kind,
            name,
            flags,
            attribute_id,
            non_resident,
            raw: buf.to_vec(),
            resident,
            non_resident_layout,
        })
    }

    #[must_use]
    pub fn is_unnamed(&self) -> bool {
        self.name.is_empty()
    }

    #[must_use]
    pub fn value_size(&self) -> u64 {
        if let Some(ref nr) = self.non_resident_layout {
            return nr.real_size;
        }
        if let Some(ref r) = self.resident {
            return u64::from(r.value_length);
        }
        0
    }

    #[must_use]
    pub fn resident_value(&self) -> Option<&[u8]> {
        let r = self.resident.as_ref()?;
        let off = r.value_offset as usize;
        let end = off
            .checked_add(r.value_length as usize)
            .filter(|e| *e <= self.raw.len())?;
        Some(&self.raw[off..end])
    }

    /// Decode the run list of a non-resident attribute into volume-relative,
    /// **cluster-measured** [`RawRun`]s. Sparse runs are kept as `lcn: None`
    /// so callers can preserve VCN alignment.
    ///
    /// Translation to sector-absolute, device-LBA `tr_core::DataRun`s is the
    /// caller's responsibility (see [`crate::ntfs::mft::MftRecord::data_runs`]).
    pub fn run_list(&self) -> Result<Vec<RawRun>> {
        let nr = self
            .non_resident_layout
            .as_ref()
            .ok_or_else(|| Error::corrupt("ntfs_attr", 0, "run_list on resident attr"))?;
        let off = nr.run_list_offset as usize;
        if off >= self.raw.len() {
            return Ok(Vec::new());
        }
        runlist::decode(&self.raw[off..])
    }

    /// $FILE_NAME parsing — legacy helper returning just `(namespace, name)`.
    /// New code should call [`Self::parse_filename_full`] which also yields
    /// the parent MFT record number needed for path reconstruction.
    #[must_use]
    pub fn parse_filename(&self) -> Option<(u8, String)> {
        let f = self.parse_filename_full()?;
        Some((f.namespace, f.name))
    }

    /// $FILE_NAME parsing — full record. The first 8 bytes of the attribute
    /// value are an `MFT_REF` to the parent directory: low 48 bits are the
    /// record number, high 16 bits are the sequence number (we discard the
    /// sequence — path reconstruction only needs the record number).
    #[must_use]
    pub fn parse_filename_full(&self) -> Option<FileName> {
        let v = self.resident_value()?;
        if v.len() < 0x42 {
            return None;
        }
        let parent_ref = LittleEndian::read_u64(&v[0..8]);
        let parent_record = parent_ref & 0x0000_FFFF_FFFF_FFFF;
        let name_len = v[0x40] as usize;
        let namespace = v[0x41];
        let name_bytes_end = 0x42usize.checked_add(name_len * 2)?;
        if name_bytes_end > v.len() {
            return None;
        }
        let mut codes = Vec::with_capacity(name_len);
        for i in 0..name_len {
            codes.push(LittleEndian::read_u16(&v[0x42 + i * 2..0x42 + i * 2 + 2]));
        }
        Some(FileName {
            parent_record,
            namespace,
            name: String::from_utf16_lossy(&codes),
        })
    }

    /// $ATTRIBUTE_LIST (0x20) → list of attribute references that may live in
    /// extension MFT records. Returns `None` if this attribute is not an
    /// $ATTRIBUTE_LIST or is non-resident (non-resident lists are extremely
    /// rare; supporting them would require fetching the list payload via the
    /// run-list, which we don't do at this layer).
    #[must_use]
    pub fn parse_attribute_list(&self) -> Option<Vec<AttributeListEntry>> {
        if self.kind != AttributeType::AttributeList {
            return None;
        }
        if self.non_resident {
            tracing::warn!("non-resident $ATTRIBUTE_LIST not supported; fragmented file may be incomplete");
            return None;
        }
        let v = self.resident_value()?;
        let mut out = Vec::new();
        let mut cursor = 0usize;
        // Defensive cap — a real $ATTRIBUTE_LIST is at most a few hundred
        // entries; anything past this is corrupt.
        const MAX_ENTRIES: usize = 4096;
        while cursor + 0x1A <= v.len() && out.len() < MAX_ENTRIES {
            let ty = LittleEndian::read_u32(&v[cursor..cursor + 4]);
            let rec_len = LittleEndian::read_u16(&v[cursor + 4..cursor + 6]) as usize;
            if rec_len < 0x1A || cursor.checked_add(rec_len).is_none_or(|e| e > v.len()) {
                break;
            }
            let name_len = v[cursor + 6] as usize;
            let name_off = v[cursor + 7] as usize;
            let starting_vcn = LittleEndian::read_u64(&v[cursor + 8..cursor + 0x10]);
            let mft_ref = LittleEndian::read_u64(&v[cursor + 0x10..cursor + 0x18]);
            let attr_id = LittleEndian::read_u16(&v[cursor + 0x18..cursor + 0x1A]);
            let name = if name_len > 0
                && name_off
                    .checked_add(name_len * 2)
                    .is_some_and(|e| e <= rec_len)
            {
                let mut codes = Vec::with_capacity(name_len);
                for i in 0..name_len {
                    let p = cursor + name_off + i * 2;
                    codes.push(LittleEndian::read_u16(&v[p..p + 2]));
                }
                String::from_utf16_lossy(&codes)
            } else {
                String::new()
            };
            out.push(AttributeListEntry {
                kind: AttributeType::from_u32(ty),
                name,
                starting_vcn,
                mft_record: mft_ref & 0x0000_FFFF_FFFF_FFFF,
                mft_sequence: ((mft_ref >> 48) & 0xFFFF) as u16,
                attribute_id: attr_id,
            });
            cursor += rec_len;
        }
        Some(out)
    }

    /// Starting VCN of a non-resident attribute (0 if resident or missing).
    #[must_use]
    pub fn starting_vcn(&self) -> u64 {
        self.non_resident_layout
            .as_ref()
            .map_or(0, |nr| nr.starting_vcn)
    }

    /// $STANDARD_INFORMATION → modified time.
    #[must_use]
    pub fn parse_std_modified(&self) -> Option<DateTime<Utc>> {
        let v = self.resident_value()?;
        if v.len() < 16 {
            return None;
        }
        let modified = LittleEndian::read_u64(&v[8..16]);
        filetime_to_utc(modified)
    }
}

/// Convert a Windows FILETIME (100-ns ticks since 1601-01-01 UTC) to chrono.
fn filetime_to_utc(ft: u64) -> Option<DateTime<Utc>> {
    // 11644473600 seconds between 1601-01-01 and 1970-01-01
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?
        .and_hms_opt(0, 0, 0)?
        .and_utc();
    Some(epoch + Duration::seconds(secs) + Duration::nanoseconds(i64::from(nanos)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_resident(ty: u32, value: &[u8]) -> Vec<u8> {
        let header = 24usize;
        let total = ((header + value.len()) + 7) & !7;
        let mut buf = vec![0u8; total];
        LittleEndian::write_u32(&mut buf[0..4], ty);
        LittleEndian::write_u32(&mut buf[4..8], total as u32);
        buf[8] = 0;
        LittleEndian::write_u32(&mut buf[16..20], value.len() as u32);
        LittleEndian::write_u16(&mut buf[20..22], header as u16);
        buf[header..header + value.len()].copy_from_slice(value);
        buf
    }

    #[test]
    fn parses_resident_data() {
        let buf = build_resident(AttributeType::Data.to_u32(), b"hello");
        let a = Attribute::parse(&buf).unwrap();
        assert_eq!(a.kind, AttributeType::Data);
        assert!(!a.non_resident);
        assert_eq!(a.resident_value(), Some(&b"hello"[..]));
        assert_eq!(a.value_size(), 5);
    }

    #[test]
    fn parses_filename_value() {
        let mut v = vec![0u8; 0x42];
        v[0x40] = 4;
        v[0x41] = 1;
        for (i, c) in "test".encode_utf16().enumerate() {
            v.push((c & 0xFF) as u8);
            v.push((c >> 8) as u8);
            let _ = i;
        }
        let buf = build_resident(AttributeType::FileName.to_u32(), &v);
        let a = Attribute::parse(&buf).unwrap();
        let (ns, name) = a.parse_filename().unwrap();
        assert_eq!(ns, 1);
        assert_eq!(name, "test");
    }

    #[test]
    fn rejects_truncated_attr() {
        let buf = vec![0u8; 8];
        let r = Attribute::parse(&buf);
        assert!(r.is_err());
    }

    /// Build one $ATTRIBUTE_LIST entry value.
    fn build_alist_entry(
        ty: u32,
        starting_vcn: u64,
        mft_record: u64,
        mft_sequence: u16,
        attribute_id: u16,
    ) -> Vec<u8> {
        let mut e = vec![0u8; 0x1A];
        LittleEndian::write_u32(&mut e[0..4], ty);
        LittleEndian::write_u16(&mut e[4..6], 0x1A);
        e[6] = 0; // name length
        e[7] = 0x1A; // name offset (no name, points at end)
        LittleEndian::write_u64(&mut e[8..16], starting_vcn);
        let mft_ref = (u64::from(mft_sequence) << 48) | (mft_record & 0x0000_FFFF_FFFF_FFFF);
        LittleEndian::write_u64(&mut e[16..24], mft_ref);
        LittleEndian::write_u16(&mut e[24..26], attribute_id);
        e
    }

    #[test]
    fn parses_attribute_list_two_entries() {
        let mut value = Vec::new();
        // entry 1: $DATA chunk, starting VCN 0, lives in record 100
        value.extend(build_alist_entry(0x80, 0, 100, 1, 5));
        // entry 2: $DATA chunk, starting VCN 200, lives in record 250
        value.extend(build_alist_entry(0x80, 200, 250, 1, 6));

        let buf = build_resident(AttributeType::AttributeList.to_u32(), &value);
        let a = Attribute::parse(&buf).unwrap();
        let entries = a.parse_attribute_list().expect("returns Some");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, AttributeType::Data);
        assert_eq!(entries[0].starting_vcn, 0);
        assert_eq!(entries[0].mft_record, 100);
        assert_eq!(entries[1].starting_vcn, 200);
        assert_eq!(entries[1].mft_record, 250);
    }

    #[test]
    fn parse_attribute_list_returns_none_on_wrong_kind() {
        let buf = build_resident(AttributeType::Data.to_u32(), b"x");
        let a = Attribute::parse(&buf).unwrap();
        assert!(a.parse_attribute_list().is_none());
    }

    #[test]
    fn parse_attribute_list_handles_truncated_entry() {
        // record_len claims 0x40 but value is only 0x1A bytes — must not panic.
        let mut value = vec![0u8; 0x1A];
        LittleEndian::write_u32(&mut value[0..4], 0x80);
        LittleEndian::write_u16(&mut value[4..6], 0x40);
        let buf = build_resident(AttributeType::AttributeList.to_u32(), &value);
        let a = Attribute::parse(&buf).unwrap();
        let entries = a.parse_attribute_list().unwrap();
        assert!(entries.is_empty());
    }
}
