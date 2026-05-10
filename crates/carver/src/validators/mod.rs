//! Per-format validators. Each validator inspects a byte window starting at a
//! candidate magic and returns the precise file length, or rejects the hit.
//!
//! Contract (from `docs/file-signatures.md`):
//! - `validate(window)` is read-only, side-effect free, bounded.
//! - The returned length is in bytes, measured from the magic offset.
//! - `Validation::NeedsMore` asks the scanner to widen the window.

use crate::Validation;
use tr_core::FileKind;

mod bmp;
mod gif;
mod jpeg;
mod mkv;
mod mp4;
mod ooxml;
mod pdf;
mod png;
mod psd;
mod rar;
mod riff_avi;
mod sevenz;
mod text;
mod tiff;
mod zip;

pub use bmp::BmpValidator;
pub use gif::GifValidator;
pub use jpeg::JpegValidator;
pub use mkv::MkvValidator;
pub use mp4::Mp4Validator;
pub use ooxml::OoxmlValidator;
pub use pdf::PdfValidator;
pub use png::PngValidator;
pub use psd::PsdValidator;
pub use rar::RarValidator;
pub use riff_avi::RiffAviValidator;
pub use sevenz::SevenZValidator;
pub use text::TextValidator;
pub use tiff::TiffValidator;
pub use zip::ZipValidator;

/// Read-only, side-effect-free format checker.
pub trait Validator: std::fmt::Debug + Send + Sync {
    /// Inspect `window`. Window starts at the candidate magic position
    /// (i.e. for an MP4 hit, `window[0]` is the start of the 32-bit atom size,
    /// not the `f` of `ftyp`).
    fn validate(&self, window: &[u8]) -> Validation;

    /// Reported file kind. Some validators (OOXML) determine this dynamically;
    /// such validators should override [`Validator::validate_with_kind`] and
    /// leave this as a placeholder.
    fn kind(&self) -> FileKind;

    /// Optional: when the validator can refine the kind (OOXML → docx/xlsx/pptx),
    /// this returns the precise kind alongside the validation.
    fn validate_with_kind(&self, window: &[u8]) -> (Validation, FileKind) {
        (self.validate(window), self.kind())
    }
}

// ---------- shared helpers ----------

/// Read a big-endian u32 at `off` from `buf`, or `None` if OOB.
#[inline]
pub(crate) fn read_u32_be(buf: &[u8], off: usize) -> Option<u32> {
    let end = off.checked_add(4)?;
    if end > buf.len() {
        return None;
    }
    Some(u32::from_be_bytes(buf[off..end].try_into().ok()?))
}

/// Read a little-endian u32 at `off`.
#[inline]
pub(crate) fn read_u32_le(buf: &[u8], off: usize) -> Option<u32> {
    let end = off.checked_add(4)?;
    if end > buf.len() {
        return None;
    }
    Some(u32::from_le_bytes(buf[off..end].try_into().ok()?))
}

/// Read a big-endian u64 at `off`.
#[inline]
pub(crate) fn read_u64_be(buf: &[u8], off: usize) -> Option<u64> {
    let end = off.checked_add(8)?;
    if end > buf.len() {
        return None;
    }
    Some(u64::from_be_bytes(buf[off..end].try_into().ok()?))
}

/// Read a little-endian u64 at `off`.
#[inline]
pub(crate) fn read_u64_le(buf: &[u8], off: usize) -> Option<u64> {
    let end = off.checked_add(8)?;
    if end > buf.len() {
        return None;
    }
    Some(u64::from_le_bytes(buf[off..end].try_into().ok()?))
}

/// Read a little-endian u16 at `off`.
#[inline]
pub(crate) fn read_u16_le(buf: &[u8], off: usize) -> Option<u16> {
    let end = off.checked_add(2)?;
    if end > buf.len() {
        return None;
    }
    Some(u16::from_le_bytes(buf[off..end].try_into().ok()?))
}

/// Read a big-endian u16 at `off`.
#[inline]
pub(crate) fn read_u16_be(buf: &[u8], off: usize) -> Option<u16> {
    let end = off.checked_add(2)?;
    if end > buf.len() {
        return None;
    }
    Some(u16::from_be_bytes(buf[off..end].try_into().ok()?))
}

/// SIMD-accelerated search for `needle` in `haystack[start..]`. Returns the
/// absolute position of the match, or `None`.
pub(crate) fn find_from(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= haystack.len() || needle.len() > haystack.len() - start {
        return None;
    }
    memchr::memmem::find(&haystack[start..], needle).map(|p| p + start)
}

/// SIMD-accelerated reverse search for `needle` ending at or before `end`.
pub(crate) fn rfind_within(haystack: &[u8], needle: &[u8], end: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let upper = end.min(haystack.len());
    if needle.len() > upper {
        return None;
    }
    memchr::memmem::rfind(&haystack[..upper], needle)
}
