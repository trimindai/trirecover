//! `tr-carver` — signature-based file carver for unallocated regions.
//!
//! ## Architecture
//! Every supported format is described by a [`Signature`] (magic bytes +
//! offset + max plausible size) and validated by a [`Validator`] which inspects
//! a candidate window and returns the precise file length, or `None` if the
//! candidate is a false hit.
//!
//! The [`Carver`] runs over any [`tr_storage::SectorReader`] in chunked passes
//! with header-overlap so signatures spanning chunk boundaries are not lost.
//!
//! See `docs/architecture.md` §3 and `docs/file-signatures.md`.
#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::too_many_lines
)]

pub mod scanner;
pub mod signature;
pub mod validators;

pub use scanner::{Carver, ScanConfig, ScanStats};
pub use signature::{Signature, SignatureId, signatures};
pub use validators::Validator;

use tr_core::FileKind;

/// Outcome of a single validation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validation {
    /// Confirmed hit. The file occupies `length` bytes from the candidate start.
    Confirmed { length: u64, recoverability: u8 },
    /// Almost certainly a false positive — discard.
    Rejected,
    /// The window was not large enough to decide. The scanner SHOULD widen
    /// the window (up to `max_size`) and retry.
    NeedsMore,
}

impl Validation {
    #[must_use]
    pub fn confirm(length: u64) -> Self {
        Self::Confirmed {
            length,
            recoverability: 80,
        }
    }

    #[must_use]
    pub fn confirm_with(length: u64, recoverability: u8) -> Self {
        Self::Confirmed {
            length,
            recoverability,
        }
    }
}

/// Quick mapping from a [`FileKind`] to a recoverability prior. Validators
/// can override this with format-specific evidence.
#[must_use]
pub fn baseline_recoverability(kind: FileKind) -> u8 {
    match kind {
        // Container formats with explicit length: high confidence.
        FileKind::Bmp | FileKind::Psd | FileKind::SevenZ => 90,
        FileKind::Mp4 | FileKind::Mov | FileKind::Mkv | FileKind::Avi => 85,
        // Length-by-footer formats: medium-high.
        FileKind::Jpg | FileKind::Png | FileKind::Gif | FileKind::Pdf => 80,
        // Archives — strong validation, but fragmentation hurts archives more.
        FileKind::Zip | FileKind::Docx | FileKind::Xlsx | FileKind::Pptx | FileKind::Rar => 75,
        FileKind::Tiff => 70,
        FileKind::Ai => 70,
        // Heuristic text formats.
        FileKind::Txt | FileKind::Csv | FileKind::Sql => 50,
        FileKind::Other => 40,
    }
}
