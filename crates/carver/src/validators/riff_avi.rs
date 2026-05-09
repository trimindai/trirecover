//! RIFF (AVI / WAV) validator. RIFF layout:
//!   `R I F F  [size:LE u32]  [form-type:4]  [chunks...]`
//! Total file length = `size + 8`.

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct RiffAviValidator;

const MAX_AVI_SIZE: u64 = 16 * 1024 * 1024 * 1024;

impl Validator for RiffAviValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < 12 {
            return Validation::NeedsMore;
        }
        if &w[..4] != b"RIFF" || &w[8..12] != b"AVI " {
            return Validation::Rejected;
        }
        let payload = match super::read_u32_le(w, 4) {
            Some(v) => u64::from(v),
            None => return Validation::NeedsMore,
        };
        let total = payload.saturating_add(8);
        if total > MAX_AVI_SIZE || total < 12 {
            return Validation::Rejected;
        }
        // Cannot verify trailing bytes without the whole file; allow short
        // window — confirm based on header math, similar to BMP.
        Validation::Confirmed {
            length: total,
            recoverability: baseline_recoverability(FileKind::Avi),
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Avi
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_small_avi() {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&100u32.to_le_bytes());
        v.extend_from_slice(b"AVI ");
        match RiffAviValidator.validate(&v) {
            Validation::Confirmed { length, .. } => assert_eq!(length, 108),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_wav() {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&100u32.to_le_bytes());
        v.extend_from_slice(b"WAVE");
        assert!(matches!(
            RiffAviValidator.validate(&v),
            Validation::Rejected
        ));
    }
}
