//! MFT (Master File Table) FILE record parser.
//!
//! A FILE record is a fixed-size block (default 1024 B) holding metadata for
//! one filesystem object. After applying the **Update Sequence Array (USA)**
//! fixup, attributes start at `first_attribute_offset` and run until the
//! 0xFFFFFFFF terminator.

use crate::ntfs::attribute::{Attribute, AttributeListEntry, AttributeType, FileName};
use byteorder::{ByteOrder, LittleEndian};
use chrono::{DateTime, Utc};
use tr_core::{DataRun, Error, Result};

pub const MFT_RECORD_SIZE_DEFAULT: u32 = 1024;
const FILE_MAGIC: &[u8; 4] = b"FILE";
const FLAG_IN_USE: u16 = 0x01;
const FLAG_DIRECTORY: u16 = 0x02;

#[derive(Debug, Clone)]
pub struct MftRecord {
    bytes: Vec<u8>,
    pub used_size: u32,
    pub allocated_size: u32,
    pub flags: u16,
    pub sequence: u16,
    pub first_attribute_offset: u16,
    pub mft_record_number: Option<u64>,
    /// MFT_REF of the base record this is an extension of. Zero for base
    /// records. Used by [`Self::is_extension_record`] to skip extensions
    /// during the top-level scan.
    pub base_record_ref: u64,
    attributes: Vec<Attribute>,
}

impl MftRecord {
    /// Parse a FILE record from a fixed-size buffer.
    pub fn parse(buf: &[u8], bytes_per_sector: u16, record_size: u32) -> Result<Self> {
        if buf.len() < record_size as usize {
            return Err(Error::UnexpectedEof {
                offset: 0,
                need: record_size as usize,
                have: buf.len(),
            });
        }
        let buf = &buf[..record_size as usize];
        if &buf[..4] != FILE_MAGIC {
            // Records can also start with "BAAD" if previously corrupted by NTFS.
            // We treat anything non-FILE as uninitialised.
            return Ok(Self::uninitialised());
        }

        let usa_offset = LittleEndian::read_u16(&buf[0x04..0x06]) as usize;
        let usa_count = LittleEndian::read_u16(&buf[0x06..0x08]) as usize;
        let _lsn = LittleEndian::read_u64(&buf[0x08..0x10]);
        let sequence = LittleEndian::read_u16(&buf[0x10..0x12]);
        let _hard_links = LittleEndian::read_u16(&buf[0x12..0x14]);
        let first_attribute_offset = LittleEndian::read_u16(&buf[0x14..0x16]);
        let flags = LittleEndian::read_u16(&buf[0x16..0x18]);
        let used_size = LittleEndian::read_u32(&buf[0x18..0x1C]);
        let allocated_size = LittleEndian::read_u32(&buf[0x1C..0x20]);
        let base_record_ref = LittleEndian::read_u64(&buf[0x20..0x28]);
        // Some Windows versions store the record number at 0x2C (4 bytes).
        let record_number = if buf.len() >= 0x30 {
            Some(u64::from(LittleEndian::read_u32(&buf[0x2C..0x30])))
        } else {
            None
        };

        if used_size == 0 || used_size > record_size {
            return Err(Error::corrupt(
                "mft_record",
                0,
                format!("implausible used_size {used_size} (record_size {record_size})"),
            ));
        }

        let mut bytes = buf.to_vec();
        apply_fixup(&mut bytes, bytes_per_sector, usa_offset, usa_count)?;

        let attributes = parse_attributes(&bytes, first_attribute_offset as usize, used_size as usize)?;

        Ok(Self {
            bytes,
            used_size,
            allocated_size,
            flags,
            sequence,
            first_attribute_offset,
            mft_record_number: record_number,
            base_record_ref,
            attributes,
        })
    }

    fn uninitialised() -> Self {
        Self {
            bytes: Vec::new(),
            used_size: 0,
            allocated_size: 0,
            flags: 0,
            sequence: 0,
            first_attribute_offset: 0,
            mft_record_number: None,
            base_record_ref: 0,
            attributes: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_initialised(&self) -> bool {
        !self.bytes.is_empty()
    }

    #[must_use]
    pub fn is_in_use(&self) -> bool {
        self.flags & FLAG_IN_USE != 0
    }

    #[must_use]
    pub fn is_directory(&self) -> bool {
        self.flags & FLAG_DIRECTORY != 0
    }

    #[must_use]
    pub fn record_index(&self) -> Option<u64> {
        self.mft_record_number
    }

    /// `true` if this MFT record is an *extension* of another (its
    /// `base_record_ref` is non-zero). Extension records carry spillover
    /// attributes (e.g. extra $DATA chunks for fragmented files) and must not
    /// be treated as standalone files.
    #[must_use]
    pub fn is_extension_record(&self) -> bool {
        self.base_record_ref != 0
    }

    /// Decoded $ATTRIBUTE_LIST entries if this base record carries one.
    /// Returns `None` if the record has no $ATTRIBUTE_LIST or if the list is
    /// non-resident (currently unsupported — see
    /// [`Attribute::parse_attribute_list`]).
    #[must_use]
    pub fn attribute_list_entries(&self) -> Option<Vec<AttributeListEntry>> {
        for a in &self.attributes {
            if a.kind == AttributeType::AttributeList {
                return a.parse_attribute_list();
            }
        }
        None
    }

    /// Append an attribute pulled from an extension MFT record. Used by
    /// [`crate::ntfs::NtfsVolume::expand_record`] when following an
    /// `$ATTRIBUTE_LIST` chain.
    pub fn append_attribute(&mut self, attr: Attribute) {
        self.attributes.push(attr);
    }

    /// Consume the record and return its attribute vector. Used by
    /// [`crate::ntfs::NtfsVolume::expand_record`] when draining an extension
    /// record into its base.
    #[must_use]
    pub fn take_attributes(self) -> Vec<Attribute> {
        self.attributes
    }

    /// Return the "best" $FILE_NAME (Win32 namespace beats DOS).
    #[must_use]
    pub fn best_name(&self) -> Option<String> {
        self.best_filename().map(|f| f.name)
    }

    /// Return the "best" $FILE_NAME paired with its parent MFT record number.
    /// Pairing is essential for correct path reconstruction: a hardlink shows
    /// up as multiple $FILE_NAME attributes in the same record, each with a
    /// potentially different parent. Picking name and parent independently
    /// would attribute the wrong leaf to the wrong directory.
    #[must_use]
    pub fn best_filename(&self) -> Option<FileName> {
        let mut best: Option<FileName> = None;
        for a in &self.attributes {
            if a.kind != AttributeType::FileName {
                continue;
            }
            let Some(fname) = a.parse_filename_full() else {
                continue;
            };
            let beats = best
                .as_ref()
                .is_none_or(|b| ns_priority(fname.namespace) > ns_priority(b.namespace));
            if beats {
                best = Some(fname);
            }
        }
        best
    }

    #[must_use]
    pub fn modified_time(&self) -> Option<DateTime<Utc>> {
        for a in &self.attributes {
            if a.kind == AttributeType::StandardInformation {
                if let Some(t) = a.parse_std_modified() {
                    return Some(t);
                }
            }
        }
        None
    }

    /// Real (logical) file size, taken from $DATA (non-resident) or the
    /// resident value length. With $ATTRIBUTE_LIST chains the size lives in
    /// the chunk anchored at VCN 0; we prefer that one and fall back to any.
    #[must_use]
    pub fn real_size(&self) -> u64 {
        let mut anchor: Option<&Attribute> = None;
        let mut fallback: Option<&Attribute> = None;
        for a in &self.attributes {
            if a.kind == AttributeType::Data && a.is_unnamed() {
                if a.starting_vcn() == 0 {
                    anchor = Some(a);
                } else if fallback.is_none() {
                    fallback = Some(a);
                }
            }
        }
        anchor.or(fallback).map(Attribute::value_size).unwrap_or(0)
    }

    #[must_use]
    pub fn is_data_resident(&self) -> bool {
        // A file is resident iff it has exactly one unnamed $DATA and that one
        // is resident. With chained extension chunks the file is non-resident
        // by definition.
        let mut found_any = false;
        let mut all_resident = true;
        for a in &self.attributes {
            if a.kind == AttributeType::Data && a.is_unnamed() {
                found_any = true;
                if a.non_resident {
                    all_resident = false;
                }
            }
        }
        !found_any || all_resident
    }

    /// Decode all unnamed $DATA chunks into **device-absolute, sector-LBA**
    /// [`DataRun`]s ready for the recovery engine.
    ///
    /// A fragmented file may have its $DATA split across multiple non-resident
    /// attributes living in extension MFT records (see
    /// [`crate::ntfs::NtfsVolume::expand_record`]). Each chunk owns a complete
    /// runlist for its `[starting_vcn..last_vcn]` range; deltas reset per
    /// chunk so decoded LCNs are absolute. We sort chunks by `starting_vcn`
    /// and concatenate.
    ///
    /// Conversion (per run, per chunk):
    ///   1. `Attribute::run_list` → volume-relative cluster runs ([`RawRun`]).
    ///   2. `lcn * sectors_per_cluster` → volume-relative sectors.
    ///   3. `+ partition_start_lba` → device-absolute LBA.
    ///
    /// Sparse runs (`RawRun::lcn == None`) are dropped with a `tracing::warn!`
    /// — `tr_core::DataRun` cannot model sparse, so fragmented files with a
    /// sparse middle have surrounding chunks concatenated (tracked follow-up).
    ///
    /// Returns `None` if the file has no $DATA attribute. Returns
    /// `Some(empty_vec)` if $DATA is resident (use [`Self::resident_value`] /
    /// [`Self::head_hex`] to read the bytes).
    pub fn data_runs(
        &self,
        cluster_bytes: u32,
        bytes_per_sector: u16,
        partition_start_lba: u64,
    ) -> Option<Vec<DataRun>> {
        // Collect every unnamed $DATA attribute on the (possibly merged)
        // record. There must be at least one for $DATA to exist at all.
        let mut chunks: Vec<&Attribute> = self
            .attributes
            .iter()
            .filter(|a| a.kind == AttributeType::Data && a.is_unnamed())
            .collect();
        if chunks.is_empty() {
            return None;
        }
        // Resident files have a single in-base attribute and no runlist.
        if chunks.iter().all(|a| !a.non_resident) {
            return Some(Vec::new());
        }
        // Mixed resident+non-resident is malformed; ignore the resident one.
        chunks.retain(|a| a.non_resident);
        // Order by starting VCN so concatenated runs match file byte order.
        chunks.sort_by_key(|a| a.starting_vcn());

        if bytes_per_sector == 0
            || cluster_bytes == 0
            || cluster_bytes % u32::from(bytes_per_sector) != 0
        {
            tracing::warn!(
                bytes_per_sector,
                cluster_bytes,
                "data_runs: cluster_bytes not a multiple of sector size; skipping"
            );
            return Some(Vec::new());
        }
        let sectors_per_cluster = u64::from(cluster_bytes / u32::from(bytes_per_sector));

        let mut out: Vec<DataRun> = Vec::new();
        let mut sparse_dropped: u64 = 0;
        for chunk in chunks {
            let raw_runs = match chunk.run_list() {
                Ok(rs) => rs,
                Err(e) => {
                    tracing::warn!(
                        record = self.mft_record_number,
                        starting_vcn = chunk.starting_vcn(),
                        "data_runs: chunk run_list decode failed: {e}"
                    );
                    continue;
                }
            };
            for r in raw_runs {
                let length_sectors = r.length_clusters.saturating_mul(sectors_per_cluster);
                match r.lcn {
                    None => {
                        sparse_dropped = sparse_dropped.saturating_add(length_sectors);
                    }
                    Some(lcn) if lcn < 0 => {
                        tracing::warn!(lcn, "data_runs: negative LCN survived decode; skipping run");
                    }
                    Some(lcn) => {
                        let volume_sector = (lcn as u64).saturating_mul(sectors_per_cluster);
                        out.push(DataRun {
                            start_lba: partition_start_lba.saturating_add(volume_sector),
                            length_sectors,
                        });
                    }
                }
            }
        }
        if sparse_dropped > 0 {
            tracing::warn!(
                record = self.mft_record_number,
                sparse_sectors = sparse_dropped,
                "data_runs: dropped sparse extents; recovered file may be missing zero gaps"
            );
        }
        Some(out)
    }

    /// First N bytes of the resident $DATA attribute for UI sniffing. Empty
    /// if non-resident.
    #[must_use]
    pub fn head_hex(&self, max: usize) -> String {
        for a in &self.attributes {
            if a.kind == AttributeType::Data && a.is_unnamed() && !a.non_resident {
                if let Some(bytes) = a.resident_value() {
                    let n = std::cmp::min(max, bytes.len());
                    return bytes[..n].iter().map(|b| format!("{b:02X}")).collect();
                }
            }
        }
        String::new()
    }

    /// 0..=100 estimate of how likely a recovery is to succeed.
    #[must_use]
    pub fn recoverability_score(&self) -> u8 {
        if !self.is_initialised() {
            return 0;
        }
        // Resident files almost always recover cleanly.
        if self.is_data_resident() {
            return 95;
        }
        // Non-resident with a clean run list: high.
        let runs = self.attributes.iter().find_map(|a| {
            if a.kind == AttributeType::Data && a.is_unnamed() {
                Some(a.run_list())
            } else {
                None
            }
        });
        match runs {
            Some(Ok(rs)) if !rs.is_empty() => 80,
            Some(Ok(_)) => 30,
            _ => 40,
        }
    }
}

fn ns_priority(ns: u8) -> u8 {
    match ns {
        // Win32 + DOS combined
        3 => 4,
        // Win32 only
        1 => 3,
        // POSIX
        0 => 2,
        // DOS only
        2 => 1,
        _ => 0,
    }
}

/// Apply NTFS USA fixup. Each sector inside the record has its last 2 bytes
/// replaced with the **Update Sequence Number** (USN) before being written to
/// disk; the saved bytes live in the USA. This function:
/// 1. reads the USN at `usa_offset`
/// 2. for each subsequent sector inside the record, verifies that the last 2
///    bytes equal the USN, then restores the saved value.
fn apply_fixup(buf: &mut [u8], bps: u16, usa_offset: usize, usa_count: usize) -> Result<()> {
    let bps = usize::from(bps);
    if bps < 2 {
        return Err(Error::corrupt(
            "mft_fixup",
            0,
            "bytes_per_sector < 2",
        ));
    }
    if usa_offset + 2 > buf.len() || usa_count == 0 {
        return Err(Error::corrupt(
            "mft_fixup",
            usa_offset as u64,
            "USA out of bounds",
        ));
    }
    let usn = u16::from_le_bytes([buf[usa_offset], buf[usa_offset + 1]]);

    for i in 1..usa_count {
        let sector_end = i * bps;
        let usa_pos = usa_offset + i * 2;
        if sector_end < 2 || sector_end > buf.len() || usa_pos + 2 > buf.len() {
            return Err(Error::corrupt(
                "mft_fixup",
                sector_end as u64,
                "fixup runs past record",
            ));
        }
        let last_two = u16::from_le_bytes([buf[sector_end - 2], buf[sector_end - 1]]);
        if last_two != usn {
            return Err(Error::corrupt(
                "mft_fixup",
                sector_end as u64,
                format!("USN mismatch (expected {usn:#06x}, got {last_two:#06x})"),
            ));
        }
        let saved = u16::from_le_bytes([buf[usa_pos], buf[usa_pos + 1]]);
        let bytes = saved.to_le_bytes();
        buf[sector_end - 2] = bytes[0];
        buf[sector_end - 1] = bytes[1];
    }
    Ok(())
}

fn parse_attributes(buf: &[u8], start: usize, used_size: usize) -> Result<Vec<Attribute>> {
    let mut out = Vec::new();
    let mut cursor = start;
    while cursor + 4 <= used_size {
        let ty = LittleEndian::read_u32(&buf[cursor..cursor + 4]);
        if ty == 0xFFFF_FFFF {
            break;
        }
        if cursor + 8 > used_size {
            break;
        }
        let len = LittleEndian::read_u32(&buf[cursor + 4..cursor + 8]) as usize;
        if len < 24 || cursor + len > used_size {
            // Defensive — corrupt length terminates the walk.
            break;
        }
        match Attribute::parse(&buf[cursor..cursor + len]) {
            Ok(a) => out.push(a),
            Err(_) => {} // skip individual bad attribute; keep scanning
        }
        cursor += len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ntfs::attribute::AttributeType as AT;

    /// Build a minimal MFT record with $STANDARD_INFORMATION + $FILE_NAME +
    /// resident $DATA. Returns the buffer and the record_size.
    fn make_record(name: &str, data: &[u8], in_use: bool) -> (Vec<u8>, u32) {
        let record_size: u32 = 1024;
        let mut buf = vec![0u8; record_size as usize];
        // header
        buf[..4].copy_from_slice(FILE_MAGIC);
        // USA at 0x2A (typical), length 3 entries (=> 1 USN + 2 saved → 2 sectors)
        let usa_off: u16 = 0x2A;
        let usa_count: u16 = 3;
        LittleEndian::write_u16(&mut buf[0x04..0x06], usa_off);
        LittleEndian::write_u16(&mut buf[0x06..0x08], usa_count);
        LittleEndian::write_u16(&mut buf[0x10..0x12], 1); // sequence
        LittleEndian::write_u16(&mut buf[0x12..0x14], 1); // hard links
        let first_attr_off: u16 = 0x38;
        LittleEndian::write_u16(&mut buf[0x14..0x16], first_attr_off);
        let flags: u16 = if in_use { FLAG_IN_USE } else { 0 };
        LittleEndian::write_u16(&mut buf[0x16..0x18], flags);
        // used_size will be filled at the end
        LittleEndian::write_u32(&mut buf[0x1C..0x20], record_size);

        // USA: USN = 0xCAFE, two saved values = 0x1111, 0x2222
        let usn: u16 = 0xCAFE;
        LittleEndian::write_u16(&mut buf[usa_off as usize..usa_off as usize + 2], usn);
        LittleEndian::write_u16(&mut buf[usa_off as usize + 2..usa_off as usize + 4], 0x1111);
        LittleEndian::write_u16(&mut buf[usa_off as usize + 4..usa_off as usize + 6], 0x2222);

        // attribute 1: $STANDARD_INFORMATION (resident)
        let mut cursor = first_attr_off as usize;
        write_resident_attr(&mut buf, &mut cursor, AT::StandardInformation.to_u32(), &[0u8; 48]);
        // attribute 2: $FILE_NAME (resident)
        let fname = make_filename_value(name);
        write_resident_attr(&mut buf, &mut cursor, AT::FileName.to_u32(), &fname);
        // attribute 3: $DATA (resident)
        write_resident_attr(&mut buf, &mut cursor, AT::Data.to_u32(), data);
        // terminator
        LittleEndian::write_u32(&mut buf[cursor..cursor + 4], 0xFFFF_FFFF);
        cursor += 4;

        let used_size = cursor as u32;
        LittleEndian::write_u32(&mut buf[0x18..0x1C], used_size);

        // place USN at end of every sector inside the record
        let bps = 512;
        for i in 1..usa_count as usize {
            let pos = i * bps - 2;
            buf[pos] = (usn & 0xFF) as u8;
            buf[pos + 1] = (usn >> 8) as u8;
        }
        (buf, record_size)
    }

    fn write_resident_attr(buf: &mut [u8], cursor: &mut usize, ty: u32, value: &[u8]) {
        let header = 24usize;
        let value_offset = header;
        let total = header + value.len();
        let total = (total + 7) & !7; // 8-byte aligned
        LittleEndian::write_u32(&mut buf[*cursor..*cursor + 4], ty);
        LittleEndian::write_u32(&mut buf[*cursor + 4..*cursor + 8], total as u32);
        buf[*cursor + 8] = 0; // resident
        buf[*cursor + 9] = 0; // name length
        LittleEndian::write_u16(&mut buf[*cursor + 10..*cursor + 12], 0);
        LittleEndian::write_u16(&mut buf[*cursor + 12..*cursor + 14], 0); // flags
        LittleEndian::write_u16(&mut buf[*cursor + 14..*cursor + 16], 0); // attr id
        LittleEndian::write_u32(&mut buf[*cursor + 16..*cursor + 20], value.len() as u32);
        LittleEndian::write_u16(&mut buf[*cursor + 20..*cursor + 22], value_offset as u16);
        buf[*cursor + 22] = 0;
        buf[*cursor + 23] = 0;
        buf[*cursor + value_offset..*cursor + value_offset + value.len()].copy_from_slice(value);
        *cursor += total;
    }

    fn make_filename_value(name: &str) -> Vec<u8> {
        let mut v = vec![0u8; 0x42];
        // parent = 5 (root)
        LittleEndian::write_u64(&mut v[0..8], 5);
        let utf16: Vec<u16> = name.encode_utf16().collect();
        v[0x40] = utf16.len() as u8;
        v[0x41] = 1; // namespace = Win32
        for c in utf16 {
            v.push((c & 0xFF) as u8);
            v.push((c >> 8) as u8);
        }
        v
    }

    #[test]
    fn parses_record_and_finds_filename() {
        let (buf, sz) = make_record("hello.txt", b"hello world", true);
        let rec = MftRecord::parse(&buf, 512, sz).unwrap();
        assert!(rec.is_initialised());
        assert!(rec.is_in_use());
        assert!(!rec.is_directory());
        assert_eq!(rec.best_name().as_deref(), Some("hello.txt"));
        assert!(rec.is_data_resident());
        assert_eq!(rec.real_size(), b"hello world".len() as u64);
    }

    #[test]
    fn detects_deleted_record() {
        let (buf, sz) = make_record("gone.bin", b"x", false);
        let rec = MftRecord::parse(&buf, 512, sz).unwrap();
        assert!(rec.is_initialised());
        assert!(!rec.is_in_use());
    }

    #[test]
    fn rejects_bad_fixup() {
        let (mut buf, sz) = make_record("a.txt", b"x", true);
        // corrupt the USN at the end of sector 1
        buf[2 * 512 - 2] ^= 0xFF;
        let r = MftRecord::parse(&buf, 512, sz);
        assert!(matches!(r, Err(Error::Corrupt { .. })));
    }

    /// Build a record holding a *non-resident* $DATA attribute whose run list
    /// describes one extent of `length_clusters` clusters at LCN `lcn`.
    fn make_nonres_record(lcn: u64, length_clusters: u64, real_size: u64) -> (Vec<u8>, u32) {
        let record_size: u32 = 1024;
        let mut buf = vec![0u8; record_size as usize];

        buf[..4].copy_from_slice(FILE_MAGIC);
        let usa_off: u16 = 0x2A;
        let usa_count: u16 = 3;
        LittleEndian::write_u16(&mut buf[0x04..0x06], usa_off);
        LittleEndian::write_u16(&mut buf[0x06..0x08], usa_count);
        LittleEndian::write_u16(&mut buf[0x10..0x12], 1);
        LittleEndian::write_u16(&mut buf[0x12..0x14], 1);
        let first_attr_off: u16 = 0x38;
        LittleEndian::write_u16(&mut buf[0x14..0x16], first_attr_off);
        LittleEndian::write_u16(&mut buf[0x16..0x18], FLAG_IN_USE);
        LittleEndian::write_u32(&mut buf[0x1C..0x20], record_size);

        let usn: u16 = 0xCAFE;
        LittleEndian::write_u16(&mut buf[usa_off as usize..usa_off as usize + 2], usn);
        LittleEndian::write_u16(&mut buf[usa_off as usize + 2..usa_off as usize + 4], 0);
        LittleEndian::write_u16(&mut buf[usa_off as usize + 4..usa_off as usize + 6], 0);

        // $FILE_NAME (resident) so best_name() works
        let mut cursor = first_attr_off as usize;
        let fname = make_filename_value("big.bin");
        write_resident_attr(&mut buf, &mut cursor, AT::FileName.to_u32(), &fname);

        // Encode a run list: header byte 0x21 (len_bytes=1, off_bytes=2),
        // length byte, two LCN delta bytes (LE, signed), trailing 0.
        // Restrict to lcn < 0x10000 so it fits in 2 signed bytes positive.
        assert!(lcn < 0x7FFF, "test helper: keep lcn small");
        assert!(length_clusters < 0xFF, "test helper: keep length small");
        let runlist = [
            0x21,
            length_clusters as u8,
            (lcn & 0xFF) as u8,
            ((lcn >> 8) & 0xFF) as u8,
            0x00,
        ];

        // Non-resident $DATA attribute. Header is 0x40 bytes, run list follows.
        let header_size: usize = 0x40;
        let total: usize = ((header_size + runlist.len()) + 7) & !7;
        let attr_off = cursor;
        LittleEndian::write_u32(&mut buf[attr_off..attr_off + 4], AT::Data.to_u32());
        LittleEndian::write_u32(&mut buf[attr_off + 4..attr_off + 8], total as u32);
        buf[attr_off + 8] = 1; // non-resident
        buf[attr_off + 9] = 0; // name length
        LittleEndian::write_u16(&mut buf[attr_off + 10..attr_off + 12], 0); // name offset
        LittleEndian::write_u16(&mut buf[attr_off + 12..attr_off + 14], 0); // flags
        LittleEndian::write_u16(&mut buf[attr_off + 14..attr_off + 16], 0); // attr id
        LittleEndian::write_u64(&mut buf[attr_off + 16..attr_off + 24], 0); // starting VCN
        LittleEndian::write_u64(
            &mut buf[attr_off + 24..attr_off + 32],
            length_clusters.saturating_sub(1),
        );
        LittleEndian::write_u16(&mut buf[attr_off + 32..attr_off + 34], header_size as u16);
        LittleEndian::write_u16(&mut buf[attr_off + 34..attr_off + 36], 0); // compression unit
        LittleEndian::write_u32(&mut buf[attr_off + 36..attr_off + 40], 0); // padding
        LittleEndian::write_u64(&mut buf[attr_off + 40..attr_off + 48], real_size);
        LittleEndian::write_u64(&mut buf[attr_off + 48..attr_off + 56], real_size);
        LittleEndian::write_u64(&mut buf[attr_off + 56..attr_off + 64], real_size);
        buf[attr_off + header_size..attr_off + header_size + runlist.len()]
            .copy_from_slice(&runlist);
        cursor = attr_off + total;

        LittleEndian::write_u32(&mut buf[cursor..cursor + 4], 0xFFFF_FFFF);
        cursor += 4;
        let used_size = cursor as u32;
        LittleEndian::write_u32(&mut buf[0x18..0x1C], used_size);

        let bps = 512;
        for i in 1..usa_count as usize {
            let pos = i * bps - 2;
            buf[pos] = (usn & 0xFF) as u8;
            buf[pos + 1] = (usn >> 8) as u8;
        }
        (buf, record_size)
    }

    #[test]
    fn data_runs_converts_clusters_to_device_lba() {
        // 4 KiB clusters on a 512-byte-sector disk → 8 sectors per cluster.
        // Partition starts at device LBA 2_048.
        // Run: 3 clusters at LCN 100 → volume sector 800 → device LBA 2_848,
        //      length 24 sectors.
        let (buf, sz) = make_nonres_record(100, 3, 12_288);
        let rec = MftRecord::parse(&buf, 512, sz).unwrap();
        assert!(!rec.is_data_resident());
        let runs = rec.data_runs(4096, 512, 2048).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].start_lba, 2048 + 100 * 8);
        assert_eq!(runs[0].length_sectors, 3 * 8);
    }

    #[test]
    fn data_runs_returns_empty_for_resident() {
        let (buf, sz) = make_record("hi.txt", b"hi", true);
        let rec = MftRecord::parse(&buf, 512, sz).unwrap();
        assert!(rec.is_data_resident());
        let runs = rec.data_runs(4096, 512, 2048).unwrap();
        assert!(runs.is_empty());
    }

    /// Build an `Attribute` for a non-resident $DATA chunk with one extent.
    /// Used to simulate the spillover chunks that live in extension MFT
    /// records when an $ATTRIBUTE_LIST chain is present.
    fn make_nonres_data_attr(
        starting_vcn: u64,
        last_vcn: u64,
        lcn: u64,
        length_clusters: u64,
        real_size: u64,
    ) -> crate::ntfs::attribute::Attribute {
        assert!(lcn < 0x7FFF, "test helper: keep lcn small");
        assert!(length_clusters < 0xFF, "test helper: keep length small");
        let runlist = [
            0x21,
            length_clusters as u8,
            (lcn & 0xFF) as u8,
            ((lcn >> 8) & 0xFF) as u8,
            0x00,
        ];
        let header_size: usize = 0x40;
        let total: usize = ((header_size + runlist.len()) + 7) & !7;
        let mut buf = vec![0u8; total];
        LittleEndian::write_u32(&mut buf[0..4], AT::Data.to_u32());
        LittleEndian::write_u32(&mut buf[4..8], total as u32);
        buf[8] = 1; // non-resident
        LittleEndian::write_u16(&mut buf[10..12], 0); // name offset
        LittleEndian::write_u16(&mut buf[12..14], 0); // flags
        LittleEndian::write_u16(&mut buf[14..16], 0); // attr id
        LittleEndian::write_u64(&mut buf[16..24], starting_vcn);
        LittleEndian::write_u64(&mut buf[24..32], last_vcn);
        LittleEndian::write_u16(&mut buf[32..34], header_size as u16);
        LittleEndian::write_u64(&mut buf[40..48], real_size);
        LittleEndian::write_u64(&mut buf[48..56], real_size);
        LittleEndian::write_u64(&mut buf[56..64], real_size);
        buf[header_size..header_size + runlist.len()].copy_from_slice(&runlist);
        crate::ntfs::attribute::Attribute::parse(&buf).unwrap()
    }

    #[test]
    fn data_runs_concatenates_chunks_in_vcn_order() {
        // base record: VCN 0..2 at LCN 100 (3 clusters), file size 12_288
        let (buf, sz) = make_nonres_record(100, 3, 12_288);
        let mut rec = MftRecord::parse(&buf, 512, sz).unwrap();
        // simulate $ATTRIBUTE_LIST spillover: VCN 3..7 at LCN 500 (5 clusters)
        rec.append_attribute(make_nonres_data_attr(3, 7, 500, 5, 0));

        let runs = rec.data_runs(4096, 512, 2048).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].start_lba, 2048 + 100 * 8);
        assert_eq!(runs[0].length_sectors, 3 * 8);
        assert_eq!(runs[1].start_lba, 2048 + 500 * 8);
        assert_eq!(runs[1].length_sectors, 5 * 8);
        // real_size still comes from the VCN-0 chunk, not the extension's 0.
        assert_eq!(rec.real_size(), 12_288);
        assert!(!rec.is_data_resident());
    }

    #[test]
    fn data_runs_sort_chunks_by_starting_vcn() {
        // Insert chunks out of VCN order; data_runs() must sort them.
        let (buf, sz) = make_nonres_record(100, 3, 12_288);
        let mut rec = MftRecord::parse(&buf, 512, sz).unwrap();
        rec.append_attribute(make_nonres_data_attr(8, 10, 900, 3, 0));
        rec.append_attribute(make_nonres_data_attr(3, 7, 500, 5, 0));

        let runs = rec.data_runs(4096, 512, 2048).unwrap();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].start_lba, 2048 + 100 * 8); // VCN 0
        assert_eq!(runs[1].start_lba, 2048 + 500 * 8); // VCN 3
        assert_eq!(runs[2].start_lba, 2048 + 900 * 8); // VCN 8
    }
}
