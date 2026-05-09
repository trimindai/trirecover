//! Adobe PSD validator. The header has signature `8BPS` and contains four
//! variable-length sections, each prefixed with a 32-bit big-endian length:
//!   1. File header (26 bytes, fixed)
//!   2. Color mode data (length at offset 26)
//!   3. Image resources (length at section start)
//!   4. Layer & mask info (length at section start)
//!   5. Image data (to EOF)
//!
//! Without parsing image data we can compute every section length but the
//! last; for the last section we need the actual file size. The image data
//! section's length is `width * height * channels * (depth/8)` for raw mode,
//! but PSD also supports RLE/zip — too complex for the carver. Instead we
//! return the offset *just after* the layer-and-mask section as the minimum
//! plausible end, with reduced recoverability.

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct PsdValidator;

const HEADER_LEN: usize = 26;
const MAX_PSD: u64 = 2 * 1024 * 1024 * 1024;

impl Validator for PsdValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < HEADER_LEN + 4 {
            return Validation::NeedsMore;
        }
        if &w[..4] != b"8BPS" {
            return Validation::Rejected;
        }
        let version = match super::read_u16_be(w, 4) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        if version != 1 && version != 2 {
            return Validation::Rejected;
        }
        // Color mode data
        let cm_len = match super::read_u32_be(w, HEADER_LEN) {
            Some(v) => v as usize,
            None => return Validation::NeedsMore,
        };
        let resources_off = HEADER_LEN + 4 + cm_len;
        if resources_off + 4 > w.len() {
            return Validation::NeedsMore;
        }
        let res_len = match super::read_u32_be(w, resources_off) {
            Some(v) => v as usize,
            None => return Validation::NeedsMore,
        };
        let lm_off = resources_off + 4 + res_len;
        if lm_off + 4 > w.len() {
            return Validation::NeedsMore;
        }
        // PSB (version 2) uses 64-bit length here; PSD (version 1) uses 32-bit.
        let (lm_len_bytes, lm_len) = if version == 2 {
            if lm_off + 8 > w.len() {
                return Validation::NeedsMore;
            }
            (
                8usize,
                match super::read_u64_be(w, lm_off) {
                    Some(v) => v,
                    None => return Validation::NeedsMore,
                },
            )
        } else {
            (
                4usize,
                u64::from(match super::read_u32_be(w, lm_off) {
                    Some(v) => v,
                    None => return Validation::NeedsMore,
                }),
            )
        };
        let image_data_start = lm_off as u64 + lm_len_bytes as u64 + lm_len;
        // We can't determine the image-data length without decoding; use the
        // window length as an upper bound when smaller than max_size.
        let total = if (image_data_start as usize) < w.len() {
            // we have at least image data start; assume the rest of the
            // window is the image data section
            w.len() as u64
        } else {
            image_data_start
        };
        if total > MAX_PSD {
            return Validation::Rejected;
        }
        Validation::Confirmed {
            length: total,
            recoverability: baseline_recoverability(FileKind::Psd) - 5,
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Psd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_magic() {
        let v = vec![0u8; 64];
        assert!(matches!(PsdValidator.validate(&v), Validation::Rejected));
    }
}
