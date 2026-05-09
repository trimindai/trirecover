//! PNG validator. PNG layout: 8-byte signature, then a sequence of chunks
//! `[length:u32][type:4][data:length][crc:u32]`. The IEND chunk terminates the
//! image. We walk chunks, optionally verifying CRC32 for the IHDR chunk only
//! (full-file CRC verification is too expensive in the carver hot loop).

use super::Validator;
use crate::{baseline_recoverability, Validation};
use crc::{Crc, CRC_32_ISO_HDLC};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct PngValidator;

const PNG_SIG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const CRC: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);
// Defensive cap on chunk size — way larger than any real chunk.
const MAX_CHUNK_LEN: u32 = 256 * 1024 * 1024;

impl Validator for PngValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < PNG_SIG.len() + 12 || &w[..PNG_SIG.len()] != PNG_SIG {
            return if w.len() < PNG_SIG.len() {
                Validation::NeedsMore
            } else {
                Validation::Rejected
            };
        }
        let mut i = PNG_SIG.len();
        let mut saw_ihdr = false;
        let mut chunks_seen = 0u32;
        loop {
            if i + 8 > w.len() {
                return Validation::NeedsMore;
            }
            let len = match super::read_u32_be(w, i) {
                Some(v) => v,
                None => return Validation::NeedsMore,
            };
            if len > MAX_CHUNK_LEN {
                return Validation::Rejected;
            }
            let kind = &w[i + 4..i + 8];
            // Chunk type bytes must be ASCII letters per spec.
            if !kind.iter().all(|b| b.is_ascii_alphabetic()) {
                return Validation::Rejected;
            }
            let data_off = i + 8;
            let crc_off = data_off + len as usize;
            let next = crc_off + 4;
            if next > w.len() {
                return Validation::NeedsMore;
            }

            // CRC-check the IHDR (cheap, catches misaligned/random hits).
            if !saw_ihdr {
                if kind != b"IHDR" {
                    return Validation::Rejected;
                }
                if len != 13 {
                    return Validation::Rejected;
                }
                let want = match super::read_u32_be(w, crc_off) {
                    Some(v) => v,
                    None => return Validation::NeedsMore,
                };
                let got = CRC.checksum(&w[i + 4..crc_off]);
                if want != got {
                    return Validation::Rejected;
                }
                saw_ihdr = true;
            }

            chunks_seen += 1;
            if chunks_seen > 1_000_000 {
                return Validation::Rejected;
            }
            if kind == b"IEND" {
                return Validation::Confirmed {
                    length: next as u64,
                    recoverability: baseline_recoverability(FileKind::Png),
                };
            }
            i = next;
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Png
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_png() -> Vec<u8> {
        let mut v: Vec<u8> = PNG_SIG.to_vec();
        // IHDR chunk: len=13, type=IHDR, then 13 bytes of header data
        v.extend_from_slice(&13u32.to_be_bytes());
        let ihdr_payload: [u8; 13] = [
            b'I', b'H', b'D', b'R',
            0, 0, 0, 1, // width = 1
            0, 0, 0, 1, // height = 1
            8,          // bit depth
            // 4 more bytes: colour type, compression, filter, interlace
            // wait: IHDR is type(4) + 13 data = 17 bytes total in slice we feed CRC
            // but our payload here as a slice for CRC includes type (4) + 13 = 17
        ];
        // The "type+data" run is what's passed to CRC.
        let mut type_and_data = Vec::with_capacity(17);
        type_and_data.extend_from_slice(b"IHDR");
        type_and_data.extend_from_slice(&[
            0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0,
        ]);
        // first push type+data into v
        v.extend_from_slice(&type_and_data);
        let crc = CRC.checksum(&type_and_data);
        v.extend_from_slice(&crc.to_be_bytes());
        let _ = ihdr_payload;
        // IEND chunk: len=0, type=IEND, crc
        v.extend_from_slice(&0u32.to_be_bytes());
        let iend_td = b"IEND";
        v.extend_from_slice(iend_td);
        let crc = CRC.checksum(iend_td);
        v.extend_from_slice(&crc.to_be_bytes());
        v
    }

    #[test]
    fn validates_minimal_png() {
        let v = PngValidator;
        let b = minimal_png();
        match v.validate(&b) {
            Validation::Confirmed { length, .. } => assert_eq!(length as usize, b.len()),
            other => panic!("expected confirmed, got {other:?}"),
        }
    }

    #[test]
    fn rejects_corrupt_ihdr_crc() {
        let v = PngValidator;
        let mut b = minimal_png();
        // Flip a byte in IHDR data
        b[16] ^= 0xFF;
        assert!(matches!(v.validate(&b), Validation::Rejected));
    }

    #[test]
    fn rejects_non_png() {
        let v = PngValidator;
        let buf = vec![0u8; 64];
        assert!(matches!(v.validate(&buf), Validation::Rejected));
    }
}
