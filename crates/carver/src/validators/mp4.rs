//! MP4 / MOV validator. ISO/BMFF box layout: each box is `[size:u32 BE][type:4]
//! [data...]`. If `size==1`, an extended 64-bit size follows. If `size==0`,
//! the box extends to EOF. We walk top-level boxes and sum their sizes.
//!
//! Reported kind is MP4 by default; if the `ftyp` major brand is `qt  ` we
//! could promote to MOV — left to the caller for now (the catalog already
//! exposes both magics).

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct Mp4Validator;

const MAX_MP4_SIZE: u64 = 16 * 1024 * 1024 * 1024;
const MAX_BOXES: u32 = 1_000_000;

impl Validator for Mp4Validator {
    fn validate(&self, w: &[u8]) -> Validation {
        // Window starts at the candidate (i.e., at the size word of the FIRST box).
        if w.len() < 8 {
            return Validation::NeedsMore;
        }
        if &w[4..8] != b"ftyp" {
            return Validation::Rejected;
        }
        let mut i: u64 = 0;
        let mut boxes = 0u32;
        loop {
            boxes += 1;
            if boxes > MAX_BOXES {
                return Validation::Rejected;
            }
            // Need 8 bytes of header
            let i_us = match usize::try_from(i) {
                Ok(v) => v,
                Err(_) => return Validation::Rejected,
            };
            if i_us + 8 > w.len() {
                return Validation::NeedsMore;
            }
            let size32 = match super::read_u32_be(w, i_us) {
                Some(v) => v,
                None => return Validation::NeedsMore,
            };
            let typ = &w[i_us + 4..i_us + 8];
            if !typ.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
                return Validation::Rejected;
            }
            let box_size: u64 = match size32 {
                1 => {
                    if i_us + 16 > w.len() {
                        return Validation::NeedsMore;
                    }
                    let s64 = match super::read_u64_be(w, i_us + 8) {
                        Some(v) => v,
                        None => return Validation::NeedsMore,
                    };
                    if s64 < 16 {
                        return Validation::Rejected;
                    }
                    s64
                }
                0 => {
                    // Extends to EOF — we cannot determine length without
                    // knowing total media size. Treat the box as covering
                    // the entire window and confirm with that length.
                    let end = w.len() as u64;
                    if end <= i {
                        return Validation::Rejected;
                    }
                    return Validation::Confirmed {
                        length: end,
                        recoverability: baseline_recoverability(FileKind::Mp4) - 10,
                    };
                }
                s if s < 8 => return Validation::Rejected,
                s => s as u64,
            };
            let next = match i.checked_add(box_size) {
                Some(v) => v,
                None => return Validation::Rejected,
            };
            if next > MAX_MP4_SIZE {
                return Validation::Rejected;
            }
            i = next;
            if (i as usize) >= w.len() {
                // We consumed every byte we have. If the buffer is shorter
                // than max_size, the file may legitimately end here OR there
                // may be more boxes beyond the window. Without container-tail
                // markers, treat reaching exactly-the-window as confirmed.
                if i as usize == w.len() {
                    return Validation::Confirmed {
                        length: i,
                        recoverability: baseline_recoverability(FileKind::Mp4),
                    };
                }
                return Validation::NeedsMore;
            }
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Mp4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_mp4(boxes: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut v = Vec::new();
        for (typ, data) in boxes {
            let size = (8 + data.len()) as u32;
            v.extend_from_slice(&size.to_be_bytes());
            v.extend_from_slice(*typ);
            v.extend_from_slice(data);
        }
        v
    }

    #[test]
    fn validates_simple_mp4() {
        let v = build_mp4(&[
            (b"ftyp", b"isom\x00\x00\x02\x00mp41"),
            (b"mdat", &[0u8; 32][..]),
        ]);
        match Mp4Validator.validate(&v) {
            Validation::Confirmed { length, .. } => assert_eq!(length as usize, v.len()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_without_ftyp() {
        let v = build_mp4(&[(b"mdat", &[0u8; 16][..])]);
        assert!(matches!(Mp4Validator.validate(&v), Validation::Rejected));
    }
}
