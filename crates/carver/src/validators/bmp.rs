//! BMP validator. The total file size is in the header at offset 2 (LE u32).
//! We sanity-check that the data offset (offset 10) and DIB header size are
//! plausible.

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct BmpValidator;

impl Validator for BmpValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < 26 {
            return Validation::NeedsMore;
        }
        if &w[..2] != b"BM" {
            return Validation::Rejected;
        }
        let size = match super::read_u32_le(w, 2) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        let data_off = match super::read_u32_le(w, 10) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        let dib_size = match super::read_u32_le(w, 14) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        if !(12..=256).contains(&dib_size) {
            return Validation::Rejected;
        }
        if (data_off as usize) < 14 + dib_size as usize {
            return Validation::Rejected;
        }
        if (size as u64) < (data_off as u64) {
            return Validation::Rejected;
        }
        if size as u64 > 1024 * 1024 * 1024 {
            return Validation::Rejected;
        }
        Validation::Confirmed {
            length: size as u64,
            recoverability: baseline_recoverability(FileKind::Bmp),
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Bmp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_minimal_bmp() {
        let mut v = b"BM".to_vec();
        v.extend_from_slice(&100u32.to_le_bytes()); // total size
        v.extend_from_slice(&[0, 0, 0, 0]); // reserved
        v.extend_from_slice(&54u32.to_le_bytes()); // data offset
        v.extend_from_slice(&40u32.to_le_bytes()); // DIB header size
        v.extend_from_slice(&[0u8; 100 - 18]);
        let val = BmpValidator;
        match val.validate(&v) {
            Validation::Confirmed { length, .. } => assert_eq!(length, 100),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_bad_signature() {
        let v = vec![0u8; 64];
        assert!(matches!(BmpValidator.validate(&v), Validation::Rejected));
    }
}
