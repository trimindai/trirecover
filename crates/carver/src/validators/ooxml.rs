//! OOXML validator. OOXML files (DOCX/XLSX/PPTX) are ZIP archives whose first
//! local-file-header entry is `[Content_Types].xml`. We piggy-back on the ZIP
//! validator for length, then decide DOCX/XLSX/PPTX by inspecting the
//! `[Content_Types].xml` file (or the path of the first non-trivial entry).

use super::zip::locate_eocd;
use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct OoxmlValidator;

const MAX_OOXML: u64 = 256 * 1024 * 1024;
const ZIP_LFH: &[u8] = &[0x50, 0x4B, 0x03, 0x04];

impl Validator for OoxmlValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        let (val, _) = self.validate_with_kind(w);
        val
    }

    fn kind(&self) -> FileKind {
        // Default fallback; real kind comes from validate_with_kind.
        FileKind::Docx
    }

    fn validate_with_kind(&self, w: &[u8]) -> (Validation, FileKind) {
        if w.len() < 64 || &w[..4] != ZIP_LFH {
            return (Validation::Rejected, FileKind::Docx);
        }
        // Quick discriminator: OOXML's first entry MUST be [Content_Types].xml.
        // We check the filename in the first local-file-header.
        let fname_len = match super::read_u16_le(w, 26) {
            Some(v) => v as usize,
            None => return (Validation::NeedsMore, FileKind::Docx),
        };
        let fname_off = 30usize;
        if fname_off + fname_len > w.len() {
            return (Validation::NeedsMore, FileKind::Docx);
        }
        let fname = &w[fname_off..fname_off + fname_len];
        if fname != b"[Content_Types].xml" {
            return (Validation::Rejected, FileKind::Docx);
        }
        // Length: piggy-back on ZIP EOCD.
        let eocd = match locate_eocd(w) {
            Some(p) => p,
            None => return (Validation::NeedsMore, FileKind::Docx),
        };
        let cmt_len = match super::read_u16_le(w, eocd + 20) {
            Some(v) => v as usize,
            None => return (Validation::NeedsMore, FileKind::Docx),
        };
        let total = (eocd + 22 + cmt_len) as u64;
        if total > MAX_OOXML {
            return (Validation::Rejected, FileKind::Docx);
        }
        if total > w.len() as u64 {
            return (Validation::NeedsMore, FileKind::Docx);
        }

        // Determine specific kind by sniffing the window for a known path.
        let kind = if super::find_from(w, b"word/document.xml", 0).is_some() {
            FileKind::Docx
        } else if super::find_from(w, b"xl/workbook.xml", 0).is_some() {
            FileKind::Xlsx
        } else if super::find_from(w, b"ppt/presentation.xml", 0).is_some() {
            FileKind::Pptx
        } else {
            // Looks like OOXML container but not a recognised flavour — fall
            // back to plain ZIP so we still recover the file.
            FileKind::Zip
        };

        (
            Validation::Confirmed {
                length: total,
                recoverability: baseline_recoverability(kind),
            },
            kind,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_lfh(name: &[u8]) -> Vec<u8> {
        let mut v = ZIP_LFH.to_vec();
        v.extend_from_slice(&[0x14, 0]); // version
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(name);
        v
    }

    fn build_eocd() -> Vec<u8> {
        let mut v = vec![0x50, 0x4B, 0x05, 0x06];
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        v
    }

    #[test]
    fn detects_docx() {
        let mut v = build_lfh(b"[Content_Types].xml");
        v.extend_from_slice(b"...filler... word/document.xml ...filler...");
        v.extend_from_slice(&build_eocd());
        let (val, k) = OoxmlValidator.validate_with_kind(&v);
        assert!(matches!(val, Validation::Confirmed { .. }));
        assert_eq!(k, FileKind::Docx);
    }

    #[test]
    fn rejects_plain_zip() {
        let mut v = build_lfh(b"foo.txt");
        v.extend_from_slice(&build_eocd());
        let (val, _) = OoxmlValidator.validate_with_kind(&v);
        assert!(matches!(val, Validation::Rejected));
    }
}
