//! RAR validator (RAR4 + RAR5). Both formats are walks of length-prefixed
//! records; the exact format of the size field differs.
//!
//! For RAR4 (`Rar!\x1a\x07\x00`): each block has a 7-byte header followed by
//! optional add-size / data. We walk blocks until we hit the end-of-archive
//! block (HEAD_TYPE = 0x7B) and return the offset just after.
//!
//! For RAR5 (`Rar!\x1a\x07\x01\x00`): records use a vint-encoded size; we
//! walk until we see the end-of-archive header (header type 5).

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct RarValidator;

const RAR4_SIG: &[u8] = &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const RAR5_SIG: &[u8] = &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00];
const MAX_RAR_SIZE: u64 = 4 * 1024 * 1024 * 1024;
const MAX_BLOCKS: u32 = 5_000_000;

impl Validator for RarValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < 8 {
            return Validation::NeedsMore;
        }
        if w.starts_with(RAR5_SIG) {
            validate_rar5(w)
        } else if w.starts_with(RAR4_SIG) {
            validate_rar4(w)
        } else {
            Validation::Rejected
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Rar
    }
}

fn validate_rar4(w: &[u8]) -> Validation {
    // Skip 7-byte signature.
    let mut i = RAR4_SIG.len();
    let mut blocks = 0u32;
    loop {
        blocks += 1;
        if blocks > MAX_BLOCKS {
            return Validation::Rejected;
        }
        if i + 7 > w.len() {
            return Validation::NeedsMore;
        }
        let head_type = w[i + 2];
        let head_flags = match super::read_u16_le(w, i + 3) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        let head_size = match super::read_u16_le(w, i + 5) {
            Some(v) => v as u64,
            None => return Validation::NeedsMore,
        };
        if head_size < 7 {
            return Validation::Rejected;
        }
        let add_size: u64 = if head_flags & 0x8000 != 0 {
            if i + 11 > w.len() {
                return Validation::NeedsMore;
            }
            match super::read_u32_le(w, i + 7) {
                Some(v) => u64::from(v),
                None => return Validation::NeedsMore,
            }
        } else {
            0
        };
        let block_total = head_size + add_size;
        let next = (i as u64).checked_add(block_total).unwrap_or(u64::MAX);
        if next > MAX_RAR_SIZE {
            return Validation::Rejected;
        }
        if head_type == 0x7B {
            // EOA marker
            return Validation::Confirmed {
                length: next,
                recoverability: baseline_recoverability(FileKind::Rar),
            };
        }
        if next as usize > w.len() {
            return Validation::NeedsMore;
        }
        i = next as usize;
    }
}

fn validate_rar5(w: &[u8]) -> Validation {
    let mut i = RAR5_SIG.len();
    let mut blocks = 0u32;
    loop {
        blocks += 1;
        if blocks > MAX_BLOCKS {
            return Validation::Rejected;
        }
        // Each header: 4-byte CRC + vint header_size + vint header_type + flags ...
        if i + 4 > w.len() {
            return Validation::NeedsMore;
        }
        let after_crc = i + 4;
        let (hdr_size, vlen1) = match read_vint(w, after_crc) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        let hdr_field_off = after_crc + vlen1;
        let (hdr_type, vlen2) = match read_vint(w, hdr_field_off) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        let _ = vlen2;
        let header_end = hdr_field_off as u64 + hdr_size;
        // Read flags vint to know if data section follows
        let (flags, flen) = match read_vint(w, hdr_field_off + vlen2) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        let _ = flen;
        let data_size: u64 = if flags & 0x02 != 0 {
            // Data area present — its size follows the flags vint
            let off = hdr_field_off + vlen2 + flen;
            match read_vint(w, off) {
                Some((v, _)) => v,
                None => return Validation::NeedsMore,
            }
        } else {
            0
        };
        let block_end = header_end.saturating_add(data_size);
        if block_end > MAX_RAR_SIZE {
            return Validation::Rejected;
        }
        if hdr_type == 5 {
            // End-of-archive
            return Validation::Confirmed {
                length: block_end,
                recoverability: baseline_recoverability(FileKind::Rar),
            };
        }
        if block_end as usize > w.len() {
            return Validation::NeedsMore;
        }
        i = block_end as usize;
    }
}

/// RAR5 vint: each byte's top bit indicates "more bytes follow", remaining 7
/// bits are the value (LSB first). Up to 10 bytes.
fn read_vint(buf: &[u8], off: usize) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    let mut k = 0usize;
    while k < 10 {
        if off + k >= buf.len() {
            return None;
        }
        let b = buf[off + k];
        value |= u64::from(b & 0x7F) << shift;
        k += 1;
        if b & 0x80 == 0 {
            return Some((value, k));
        }
        shift += 7;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage() {
        let v = vec![0u8; 64];
        assert!(matches!(RarValidator.validate(&v), Validation::Rejected));
    }

    #[test]
    fn needs_more_with_only_signature() {
        let mut v = RAR4_SIG.to_vec();
        v.extend_from_slice(&[0u8; 6]); // partial header
        assert!(matches!(RarValidator.validate(&v), Validation::NeedsMore));
    }
}
