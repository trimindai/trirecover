//! TIFF validator. TIFF is IFD-driven and has no length field, so we walk
//! all IFDs, compute the highest `(offset + size)` reached by any tag's data,
//! and report that as the file length. Endianness comes from the byte-order
//! mark (`II` = little-endian, `MM` = big-endian).

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct TiffValidator;

const MAX_IFDS: usize = 64;
const MAX_TIFF_SIZE: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Copy)]
enum Endian {
    Le,
    Be,
}

impl Endian {
    fn u16(self, b: &[u8]) -> Option<u16> {
        match self {
            Endian::Le => super::read_u16_le(b, 0),
            Endian::Be => super::read_u16_be(b, 0),
        }
    }
    fn u32(self, b: &[u8]) -> Option<u32> {
        match self {
            Endian::Le => super::read_u32_le(b, 0),
            Endian::Be => super::read_u32_be(b, 0),
        }
    }
}

impl Validator for TiffValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < 8 {
            return Validation::NeedsMore;
        }
        let endian = match &w[..2] {
            b"II" => Endian::Le,
            b"MM" => Endian::Be,
            _ => return Validation::Rejected,
        };
        let magic = match endian.u16(&w[2..]) {
            Some(v) => v,
            None => return Validation::NeedsMore,
        };
        if magic != 42 {
            return Validation::Rejected;
        }
        let mut ifd_off = match endian.u32(&w[4..]) {
            Some(v) => v as usize,
            None => return Validation::NeedsMore,
        };
        if ifd_off < 8 {
            return Validation::Rejected;
        }
        let mut max_end = 8u64;
        let mut visited: Vec<usize> = Vec::with_capacity(8);
        for _ in 0..MAX_IFDS {
            if visited.contains(&ifd_off) {
                return Validation::Rejected; // cycle
            }
            visited.push(ifd_off);
            if ifd_off + 2 > w.len() {
                return Validation::NeedsMore;
            }
            let n = match endian.u16(&w[ifd_off..]) {
                Some(v) => v as usize,
                None => return Validation::NeedsMore,
            };
            if n == 0 || n > 65535 {
                return Validation::Rejected;
            }
            let entries_end = ifd_off + 2 + n * 12;
            if entries_end + 4 > w.len() {
                return Validation::NeedsMore;
            }
            for k in 0..n {
                let off = ifd_off + 2 + k * 12;
                let _tag = endian.u16(&w[off..]).unwrap();
                let typ = endian.u16(&w[off + 2..]).unwrap();
                let count = endian.u32(&w[off + 4..]).unwrap();
                let type_size: u64 = match typ {
                    1 | 2 | 6 | 7 => 1,
                    3 | 8 => 2,
                    4 | 9 | 11 => 4,
                    5 | 10 | 12 => 8,
                    _ => 1, // be permissive on unknown types
                };
                let bytes = type_size.saturating_mul(count as u64);
                let entry_end = if bytes <= 4 {
                    (off + 12) as u64
                } else {
                    let data_off = endian.u32(&w[off + 8..]).unwrap() as u64;
                    data_off + bytes
                };
                if entry_end > MAX_TIFF_SIZE {
                    return Validation::Rejected;
                }
                if entry_end > max_end {
                    max_end = entry_end;
                }
            }
            // next IFD offset
            let next = endian.u32(&w[entries_end..]).unwrap() as usize;
            if (entries_end as u64 + 4) > max_end {
                max_end = entries_end as u64 + 4;
            }
            if next == 0 {
                break;
            }
            ifd_off = next;
        }
        if max_end > w.len() as u64 {
            return Validation::NeedsMore;
        }
        Validation::Confirmed {
            length: max_end,
            recoverability: baseline_recoverability(FileKind::Tiff),
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Tiff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tiff_le() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"II"); // LE
        v.extend_from_slice(&42u16.to_le_bytes());
        v.extend_from_slice(&8u32.to_le_bytes()); // first IFD at 8
        // IFD at 8: 1 entry
        v.extend_from_slice(&1u16.to_le_bytes());
        // tag 256 (ImageWidth), type 3 (SHORT), count 1, value=128 (fits in 4 bytes)
        v.extend_from_slice(&256u16.to_le_bytes());
        v.extend_from_slice(&3u16.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&[128, 0, 0, 0]);
        // next IFD = 0
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    #[test]
    fn validates_simple_tiff() {
        let v = build_tiff_le();
        let r = TiffValidator.validate(&v);
        match r {
            Validation::Confirmed { length, .. } => assert!(length as usize <= v.len()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_bad_endian_mark() {
        let v = vec![0u8; 32];
        assert!(matches!(TiffValidator.validate(&v), Validation::Rejected));
    }
}
