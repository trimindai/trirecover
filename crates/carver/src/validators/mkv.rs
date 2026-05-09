//! MKV (Matroska / WebM) validator. EBML structure is `[ID:vint][size:vint]
//! [data]`. The top-level Segment element has size that, summed with the
//! header bytes, gives the file length.

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct MkvValidator;

const MAX_MKV_SIZE: u64 = 16 * 1024 * 1024 * 1024;
// EBML magic
const EBML_HEADER: &[u8] = &[0x1A, 0x45, 0xDF, 0xA3];
// Segment element ID = 0x18538067 (4 bytes)
const SEGMENT_ID: &[u8] = &[0x18, 0x53, 0x80, 0x67];

impl Validator for MkvValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < 8 || &w[..4] != EBML_HEADER {
            return Validation::Rejected;
        }
        // Read the EBML header element's size
        let mut i = 4usize;
        let (ebml_data_size, vint_len) = match read_vint_size(w, i) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        i += vint_len;
        let after_ebml = match (ebml_data_size as u64).checked_add(i as u64) {
            Some(v) => v as usize,
            None => return Validation::Rejected,
        };
        if after_ebml + 5 > w.len() {
            return Validation::NeedsMore;
        }
        // Expect Segment element next
        if &w[after_ebml..after_ebml + 4] != SEGMENT_ID {
            return Validation::Rejected;
        }
        let seg_size_off = after_ebml + 4;
        let (seg_size, seg_vint_len) = match read_vint_size(w, seg_size_off) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        let header_end = seg_size_off + seg_vint_len;
        // "unknown size" — a vint of all 1-bits in the value range — means
        // segment extends to EOF. We can't determine length precisely; reject
        // for safety in carving (would mis-size adjacent files).
        if seg_size == u64::MAX {
            return Validation::Rejected;
        }
        let total = match (header_end as u64).checked_add(seg_size) {
            Some(v) => v,
            None => return Validation::Rejected,
        };
        if total > MAX_MKV_SIZE {
            return Validation::Rejected;
        }
        if total > w.len() as u64 {
            return Validation::NeedsMore;
        }
        Validation::Confirmed {
            length: total,
            recoverability: baseline_recoverability(FileKind::Mkv),
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Mkv
    }
}

/// Decode an EBML variable-length integer at `off` in size form (the leading
/// bit indicates byte-count). Returns `(value, byte_count)`. The all-ones
/// pattern is reported as `u64::MAX` to signal "unknown size".
fn read_vint_size(buf: &[u8], off: usize) -> Option<(u64, usize)> {
    if off >= buf.len() {
        return None;
    }
    let first = buf[off];
    if first == 0 {
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    if off + len > buf.len() {
        return None;
    }
    // mask off the length-marker bit
    let mut value: u64 = u64::from(first & (0x7F >> (len - 1)));
    let mut all_ones = (first & (0x7F >> (len - 1))) == (0x7F >> (len - 1));
    for k in 1..len {
        let b = buf[off + k];
        value = (value << 8) | u64::from(b);
        if b != 0xFF {
            all_ones = false;
        }
    }
    if all_ones {
        return Some((u64::MAX, len));
    }
    Some((value, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vint_one_byte(value: u8) -> u8 {
        // 1-byte vint with marker bit set: 0x80 | value (value <= 0x7F)
        0x80 | (value & 0x7F)
    }

    #[test]
    fn validates_minimal_mkv() {
        let mut v = Vec::new();
        v.extend_from_slice(EBML_HEADER);
        // EBML data size = 4 (one byte vint), data 4 bytes of zeros
        v.push(vint_one_byte(4));
        v.extend_from_slice(&[0, 0, 0, 0]);
        // Segment element ID
        v.extend_from_slice(SEGMENT_ID);
        // Segment size vint = 8, data 8 bytes
        v.push(vint_one_byte(8));
        v.extend_from_slice(&[0u8; 8]);
        match MkvValidator.validate(&v) {
            Validation::Confirmed { length, .. } => assert_eq!(length as usize, v.len()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_size_segment() {
        let mut v = Vec::new();
        v.extend_from_slice(EBML_HEADER);
        v.push(vint_one_byte(0)); // EBML data size = 0
        v.extend_from_slice(SEGMENT_ID);
        v.push(0xFF); // 1-byte all-ones = unknown
        assert!(matches!(MkvValidator.validate(&v), Validation::Rejected));
    }
}
