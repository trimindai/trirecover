//! JPEG validator: walk JPEG markers from SOI (`FFD8`) to EOI (`FFD9`).
//!
//! Strategy: a real JPEG is a stream of segments. Each marker is `FF xx` where
//! `xx` is a marker byte. Stand-alone markers (RSTn, SOI, EOI, TEM) have no
//! payload; SOS introduces an entropy-coded segment terminated by the next
//! non-RST marker; everything else has a 16-bit big-endian length immediately
//! after the marker.
//!
//! We walk the stream until we find EOI and return the offset just after EOI.

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct JpegValidator;

impl Validator for JpegValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        // SOI must be present.
        if w.len() < 4 || w[0] != 0xFF || w[1] != 0xD8 || w[2] != 0xFF {
            return Validation::Rejected;
        }

        let mut i = 2usize; // start at first marker after SOI
        loop {
            // Skip stuffing FF bytes
            while i < w.len() && w[i] == 0xFF {
                i += 1;
            }
            if i >= w.len() {
                return Validation::NeedsMore;
            }
            let marker = w[i];
            i += 1;

            match marker {
                0xD9 => {
                    // EOI — end of image
                    return Validation::Confirmed {
                        length: i as u64,
                        recoverability: baseline_recoverability(FileKind::Jpg),
                    };
                }
                0x00 | 0xFF => {
                    // 0xFF00 is escape inside entropy data; should only appear
                    // inside SOS (handled below). Encountered here means
                    // misalignment — reject.
                    return Validation::Rejected;
                }
                0xD0..=0xD7 | 0x01 => {
                    // RSTn / TEM — no payload
                    continue;
                }
                0xD8 => {
                    // Another SOI inside? malformed
                    return Validation::Rejected;
                }
                0xDA => {
                    // SOS: read length, skip header, then scan entropy-coded
                    // bytes until we hit a non-RST marker.
                    let len = match super::read_u16_be(w, i) {
                        Some(l) => l as usize,
                        None => return Validation::NeedsMore,
                    };
                    if len < 2 {
                        return Validation::Rejected;
                    }
                    let after_sos = i.checked_add(len).unwrap_or(usize::MAX);
                    if after_sos > w.len() {
                        return Validation::NeedsMore;
                    }
                    i = after_sos;
                    // Scan entropy data: skip any byte except FF, and FF00/FFFFs
                    while i < w.len() {
                        if w[i] != 0xFF {
                            i += 1;
                            continue;
                        }
                        // run of FFs
                        let mut j = i + 1;
                        while j < w.len() && w[j] == 0xFF {
                            j += 1;
                        }
                        if j >= w.len() {
                            return Validation::NeedsMore;
                        }
                        let m = w[j];
                        if m == 0x00 {
                            // stuffed FF — keep scanning
                            i = j + 1;
                            continue;
                        }
                        if (0xD0..=0xD7).contains(&m) {
                            // RSTn — keep scanning
                            i = j + 1;
                            continue;
                        }
                        // real next marker — return to outer loop pointing AT 0xFF
                        i = j - 1; // backtrack to the last 0xFF before m
                        // outer loop expects to read FFs then marker
                        break;
                    }
                    if i >= w.len() {
                        return Validation::NeedsMore;
                    }
                }
                _ => {
                    // Generic segment: 16-bit BE length follows
                    let len = match super::read_u16_be(w, i) {
                        Some(l) => l as usize,
                        None => return Validation::NeedsMore,
                    };
                    if len < 2 {
                        return Validation::Rejected;
                    }
                    let after = i.checked_add(len).unwrap_or(usize::MAX);
                    if after > w.len() {
                        return Validation::NeedsMore;
                    }
                    i = after;
                }
            }
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Jpg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_jpeg() -> Vec<u8> {
        // SOI + APP0 (JFIF) + SOS-with-tiny-payload + EOI
        let mut v = vec![0xFF, 0xD8]; // SOI
        // APP0 segment, length=16
        v.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        v.extend_from_slice(b"JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00");
        // Bogus DQT, length=4 (just length itself + 2 dummy bytes)
        v.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x04, 0x00, 0x00]);
        // SOS, length=4
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x04, 0x00, 0x00]);
        // entropy data
        v.extend_from_slice(&[0x12, 0x34, 0xFF, 0x00, 0x56]);
        // EOI
        v.extend_from_slice(&[0xFF, 0xD9]);
        v
    }

    #[test]
    fn validates_minimal_jpeg() {
        let v = JpegValidator;
        let buf = minimal_jpeg();
        match v.validate(&buf) {
            Validation::Confirmed { length, .. } => assert_eq!(length as usize, buf.len()),
            other => panic!("expected confirmed, got {other:?}"),
        }
    }

    #[test]
    fn rejects_non_jpeg() {
        let v = JpegValidator;
        assert!(matches!(v.validate(b"hello world"), Validation::Rejected));
    }

    #[test]
    fn needs_more_if_truncated() {
        let v = JpegValidator;
        // SOI + start of segment but no length+payload
        let buf = [0xFF, 0xD8, 0xFF, 0xE0, 0x00];
        assert!(matches!(v.validate(&buf), Validation::NeedsMore));
    }
}
