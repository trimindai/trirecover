//! Heuristic text validator. Used for plain TXT, CSV, SQL — any format with
//! no fixed magic. We accept a window only if it consists overwhelmingly of
//! printable ASCII (or UTF-8) and, for CSV/SQL, contains the expected
//! tell-tale characters (delimiters / SQL keywords).

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Clone, Copy)]
pub struct TextValidator {
    kind: FileKind,
}

impl TextValidator {
    pub const fn new(kind: FileKind) -> Self {
        Self { kind }
    }
}

const MIN_TEXT_BYTES: usize = 256;
const PRINTABLE_THRESHOLD_PCT: u32 = 95;

impl Validator for TextValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < MIN_TEXT_BYTES {
            return Validation::NeedsMore;
        }
        // Count printable ASCII + common whitespace.
        let mut printable: u32 = 0;
        for &b in w {
            if (0x20..0x7F).contains(&b) || b == b'\n' || b == b'\r' || b == b'\t' {
                printable += 1;
            }
        }
        let pct = (printable as u64 * 100 / w.len() as u64) as u32;
        if pct < PRINTABLE_THRESHOLD_PCT {
            return Validation::Rejected;
        }

        // Format-specific tell-tales
        match self.kind {
            FileKind::Csv => {
                let commas = w.iter().filter(|&&b| b == b',').count();
                let newlines = w.iter().filter(|&&b| b == b'\n').count();
                if newlines == 0 || commas < newlines {
                    return Validation::Rejected;
                }
            }
            FileKind::Sql => {
                let lower: String = w
                    .iter()
                    .take(4096)
                    .map(|&b| (b as char).to_ascii_lowercase())
                    .collect();
                let has_kw = ["select ", "insert ", "update ", "create ", "delete "]
                    .iter()
                    .any(|kw| lower.contains(kw));
                if !has_kw {
                    return Validation::Rejected;
                }
            }
            _ => {}
        }

        // Find the first non-printable byte beyond the threshold to bound length.
        let end = w
            .iter()
            .position(|&b| {
                !((0x20..0x7F).contains(&b) || b == b'\n' || b == b'\r' || b == b'\t')
            })
            .unwrap_or(w.len());
        if end < MIN_TEXT_BYTES {
            return Validation::Rejected;
        }
        Validation::Confirmed {
            length: end as u64,
            recoverability: baseline_recoverability(self.kind),
        }
    }

    fn kind(&self) -> FileKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_simple_txt() {
        let v = TextValidator::new(FileKind::Txt);
        let mut buf = Vec::new();
        for _ in 0..32 {
            buf.extend_from_slice(b"Hello, world!\n");
        }
        match v.validate(&buf) {
            Validation::Confirmed { .. } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_binary() {
        let v = TextValidator::new(FileKind::Txt);
        let buf = vec![0xFFu8; 1024];
        assert!(matches!(v.validate(&buf), Validation::Rejected));
    }

    #[test]
    fn validates_csv() {
        let v = TextValidator::new(FileKind::Csv);
        let mut buf = String::new();
        for _ in 0..64 {
            buf.push_str("a,b,c,d,e\n");
        }
        match v.validate(buf.as_bytes()) {
            Validation::Confirmed { .. } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_csv_without_delimiters() {
        let v = TextValidator::new(FileKind::Csv);
        let mut buf = Vec::new();
        for _ in 0..64 {
            buf.extend_from_slice(b"plain text line\n");
        }
        assert!(matches!(v.validate(&buf), Validation::Rejected));
    }
}
