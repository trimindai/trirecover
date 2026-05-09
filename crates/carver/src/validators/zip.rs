//! ZIP validator. Looks for the End-of-Central-Directory (EOCD) record:
//!   `50 4B 05 06`. The EOCD is at most 65557 bytes from the end of file (its
//!   own 22 bytes + up to 64 KiB of trailing comment). We scan backwards from
//!   the end of the window for the EOCD signature and read the total length
//!   from the structure.

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct ZipValidator;

const EOCD_SIG: &[u8] = &[0x50, 0x4B, 0x05, 0x06];
const ZIP64_EOCD_LOCATOR: &[u8] = &[0x50, 0x4B, 0x06, 0x07];
const ZIP64_EOCD: &[u8] = &[0x50, 0x4B, 0x06, 0x06];
const MAX_ZIP_SIZE: u64 = 4 * 1024 * 1024 * 1024;

impl Validator for ZipValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < 30 {
            return Validation::NeedsMore;
        }
        if &w[..4] != &[0x50, 0x4B, 0x03, 0x04] {
            return Validation::Rejected;
        }
        // Locate EOCD — must be present in window for a confirmed length.
        let eocd = match locate_eocd(w) {
            Some(p) => p,
            None => return Validation::NeedsMore,
        };
        // EOCD is 22 bytes + comment_len (LE u16 at eocd+20).
        let cmt_len = match super::read_u16_le(w, eocd + 20) {
            Some(v) => v as usize,
            None => return Validation::NeedsMore,
        };
        let total = (eocd + 22 + cmt_len) as u64;
        if total > MAX_ZIP_SIZE {
            return Validation::Rejected;
        }
        if total > w.len() as u64 {
            return Validation::NeedsMore;
        }
        Validation::Confirmed {
            length: total,
            recoverability: baseline_recoverability(FileKind::Zip),
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Zip
    }
}

/// Find the EOCD record in `w`. Per spec, scan backwards from end up to
/// 65557 bytes. Returns the offset of the EOCD signature within `w`, or
/// `None` if absent.
pub(crate) fn locate_eocd(w: &[u8]) -> Option<usize> {
    let scan_from = w.len().saturating_sub(65_557 + 22);
    super::rfind_within(w, EOCD_SIG, w.len())
        .filter(|&p| p >= scan_from)
}

/// Search the window for a ZIP64 EOCD locator, which precedes the regular
/// EOCD when the archive is in ZIP64 format.
#[allow(dead_code)]
pub(crate) fn locate_zip64(w: &[u8]) -> Option<usize> {
    super::rfind_within(w, ZIP64_EOCD_LOCATOR, w.len())
        .or_else(|| super::rfind_within(w, ZIP64_EOCD, w.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_zip() -> Vec<u8> {
        let mut v = Vec::new();
        // Local file header (30 bytes minimum, no extras)
        v.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
        v.extend_from_slice(&[0x14, 0]); // version
        v.extend_from_slice(&[0, 0]); // flags
        v.extend_from_slice(&[0, 0]); // method
        v.extend_from_slice(&[0, 0, 0, 0]); // time/date
        v.extend_from_slice(&[0, 0, 0, 0]); // crc
        v.extend_from_slice(&[0, 0, 0, 0]); // compressed size
        v.extend_from_slice(&[0, 0, 0, 0]); // uncompressed size
        v.extend_from_slice(&[1, 0]); // filename len
        v.extend_from_slice(&[0, 0]); // extra len
        v.push(b'a');
        // EOCD (22 bytes, no comment)
        v.extend_from_slice(EOCD_SIG);
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        v
    }

    #[test]
    fn validates_minimal_zip() {
        let v = minimal_zip();
        match ZipValidator.validate(&v) {
            Validation::Confirmed { length, .. } => assert_eq!(length as usize, v.len()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn needs_more_without_eocd() {
        let mut v = minimal_zip();
        v.truncate(31); // chop off EOCD
        assert!(matches!(ZipValidator.validate(&v), Validation::NeedsMore));
    }
}
