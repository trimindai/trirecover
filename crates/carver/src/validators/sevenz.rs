//! 7z validator. The 32-byte 7z header has the layout:
//!   `00..06 magic (37 7A BC AF 27 1C)`
//!   `06..07 archive version major / minor`
//!   `08..0C start-header CRC32 (LE)`
//!   `0C..14 next-header offset (LE u64) — relative to byte 32`
//!   `14..1C next-header size (LE u64)`
//!   `1C..20 next-header CRC32 (LE)`
//!
//! Total file length = 32 + next_header_offset + next_header_size.

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct SevenZValidator;

const SEVENZ_MAGIC: &[u8] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
const HEADER_LEN: u64 = 32;
const MAX_7Z: u64 = 4 * 1024 * 1024 * 1024;

impl Validator for SevenZValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < HEADER_LEN as usize {
            return Validation::NeedsMore;
        }
        if &w[..6] != SEVENZ_MAGIC {
            return Validation::Rejected;
        }
        let nh_off = match super::read_u64_le(w, 12) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        let nh_size = match super::read_u64_le(w, 20) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        let total = HEADER_LEN
            .checked_add(nh_off)
            .and_then(|v| v.checked_add(nh_size));
        let total = match total {
            Some(t) => t,
            None => return Validation::Rejected,
        };
        if total > MAX_7Z || total < HEADER_LEN {
            return Validation::Rejected;
        }
        Validation::Confirmed {
            length: total,
            recoverability: baseline_recoverability(FileKind::SevenZ),
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::SevenZ
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_minimal_7z() {
        let mut v = SEVENZ_MAGIC.to_vec();
        v.extend_from_slice(&[0, 4]); // version
        v.extend_from_slice(&[0, 0, 0, 0]); // start crc
        v.extend_from_slice(&100u64.to_le_bytes()); // nh offset
        v.extend_from_slice(&50u64.to_le_bytes()); // nh size
        v.extend_from_slice(&[0, 0, 0, 0]); // nh crc
        match SevenZValidator.validate(&v) {
            Validation::Confirmed { length, .. } => assert_eq!(length, 32 + 100 + 50),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_bad_magic() {
        let v = vec![0u8; 32];
        assert!(matches!(
            SevenZValidator.validate(&v),
            Validation::Rejected
        ));
    }
}
