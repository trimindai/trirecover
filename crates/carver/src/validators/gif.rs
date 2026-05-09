//! GIF validator. GIF87a/89a layout: header (6) + LSD (7) + optional GCT,
//! then a stream of blocks. The trailer byte is `0x3B`. We walk blocks and
//! return the offset just after the trailer.

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct GifValidator;

impl Validator for GifValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < 13 {
            return Validation::NeedsMore;
        }
        if !(&w[0..6] == b"GIF87a" || &w[0..6] == b"GIF89a") {
            return Validation::Rejected;
        }
        // Logical Screen Descriptor at 6..13. Packed byte at offset 10.
        let packed = w[10];
        let gct_present = packed & 0x80 != 0;
        let gct_size = if gct_present {
            3 * (1u32 << ((packed & 0x07) + 1))
        } else {
            0
        };
        let mut i = 13usize + gct_size as usize;
        let mut blocks = 0u32;
        loop {
            if i >= w.len() {
                return Validation::NeedsMore;
            }
            blocks += 1;
            if blocks > 1_000_000 {
                return Validation::Rejected;
            }
            let intro = w[i];
            i += 1;
            match intro {
                0x3B => {
                    // Trailer
                    return Validation::Confirmed {
                        length: i as u64,
                        recoverability: baseline_recoverability(FileKind::Gif),
                    };
                }
                0x21 => {
                    // Extension: label byte + sub-blocks
                    if i >= w.len() {
                        return Validation::NeedsMore;
                    }
                    i += 1; // label
                    match skip_sub_blocks(w, i) {
                        Some(end) => i = end,
                        None => return Validation::NeedsMore,
                    }
                }
                0x2C => {
                    // Image descriptor (9 bytes), optional LCT, then LZW min code size
                    // + sub-blocks.
                    if i + 9 > w.len() {
                        return Validation::NeedsMore;
                    }
                    let img_packed = w[i + 8];
                    i += 9;
                    let lct_present = img_packed & 0x80 != 0;
                    if lct_present {
                        let lct_size = 3 * (1u32 << ((img_packed & 0x07) + 1));
                        i = i.checked_add(lct_size as usize).unwrap_or(usize::MAX);
                    }
                    if i >= w.len() {
                        return Validation::NeedsMore;
                    }
                    i += 1; // LZW minimum code size
                    match skip_sub_blocks(w, i) {
                        Some(end) => i = end,
                        None => return Validation::NeedsMore,
                    }
                }
                _ => return Validation::Rejected,
            }
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Gif
    }
}

fn skip_sub_blocks(w: &[u8], mut i: usize) -> Option<usize> {
    loop {
        if i >= w.len() {
            return None;
        }
        let n = w[i] as usize;
        i = i.checked_add(1)?.checked_add(n)?;
        if n == 0 {
            return Some(i);
        }
        if i > w.len() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_minimal_gif89a() {
        // header
        let mut v = b"GIF89a".to_vec();
        // LSD: w=1, h=1, packed=0 (no GCT), bg=0, aspect=0
        v.extend_from_slice(&[1, 0, 1, 0, 0, 0, 0]);
        // Trailer
        v.push(0x3B);
        let val = GifValidator;
        match val.validate(&v) {
            Validation::Confirmed { length, .. } => assert_eq!(length as usize, v.len()),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_header() {
        let v = GifValidator;
        let buf = vec![0u8; 32];
        assert!(matches!(v.validate(&buf), Validation::Rejected));
    }
}
