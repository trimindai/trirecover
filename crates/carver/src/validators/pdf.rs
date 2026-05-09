//! PDF validator. Walk forward from the `%PDF-` header looking for the last
//! `%%EOF` token. PDFs may contain multiple `%%EOF`s due to incremental
//! updates; we accept the last one in the window.

use super::Validator;
use crate::{baseline_recoverability, Validation};
use tr_core::FileKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct PdfValidator;

const EOF_MARKER: &[u8] = b"%%EOF";
const MIN_PDF: usize = 64;
const MAX_PDF: u64 = 1024 * 1024 * 1024;

impl Validator for PdfValidator {
    fn validate(&self, w: &[u8]) -> Validation {
        if w.len() < MIN_PDF {
            return Validation::NeedsMore;
        }
        if &w[..5] != b"%PDF-" {
            return Validation::Rejected;
        }
        // Sanity: a PDF needs an `xref` table or `startxref`. Reject if
        // neither appears in the window — that guards against the giant-window
        // case where we'd otherwise accept random data following %PDF-.
        let has_startxref = super::find_from(w, b"startxref", 5).is_some();
        if !has_startxref {
            // The %%EOF must be near the end; if we don't see startxref yet,
            // ask for more data.
            return Validation::NeedsMore;
        }
        // Find last %%EOF within the window
        let last = match super::rfind_within(w, EOF_MARKER, w.len()) {
            Some(p) => p,
            None => return Validation::NeedsMore,
        };
        // Allow optional trailing whitespace/newlines (up to 4 bytes)
        let mut end = last + EOF_MARKER.len();
        while end < w.len() && (w[end] == b'\n' || w[end] == b'\r' || w[end] == b' ') {
            end += 1;
            if end - last > EOF_MARKER.len() + 4 {
                break;
            }
        }
        if (end as u64) > MAX_PDF {
            return Validation::Rejected;
        }
        Validation::Confirmed {
            length: end as u64,
            recoverability: baseline_recoverability(FileKind::Pdf),
        }
    }

    fn kind(&self) -> FileKind {
        FileKind::Pdf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_minimal_pdf() {
        let mut v = b"%PDF-1.4\n".to_vec();
        // pad up to MIN_PDF
        v.extend_from_slice(&[b' '; MIN_PDF]);
        v.extend_from_slice(b"\nstartxref\n0\n%%EOF\n");
        match PdfValidator.validate(&v) {
            Validation::Confirmed { length, .. } => {
                assert!(length as usize >= b"%%EOF".len());
                assert!(length as usize <= v.len());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_no_pdf_header() {
        let v = vec![0u8; 256];
        assert!(matches!(PdfValidator.validate(&v), Validation::Rejected));
    }
}
