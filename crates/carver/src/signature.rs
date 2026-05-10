//! Signature catalog + first-byte index used by the scanner.
//!
//! A [`Signature`] is a (kind, magic, magic_offset, max_size, validator-id)
//! tuple. Magics may contain wildcard bytes (`None`), which match any byte at
//! that position — needed for formats like MP4 whose four-byte big-endian
//! atom-size precedes the `ftyp` magic.

use std::sync::OnceLock;

use crate::validators::{
    self, BmpValidator, GifValidator, JpegValidator, MkvValidator, Mp4Validator, OoxmlValidator,
    PdfValidator, PngValidator, PsdValidator, RarValidator, RiffAviValidator, SevenZValidator,
    TextValidator, TiffValidator, Validator, ZipValidator,
};
use tr_core::FileKind;

/// One byte of a magic pattern: `Some(b)` matches exactly, `None` is a wildcard.
pub type MagicByte = Option<u8>;

/// Stable identifier for a [`Signature`] within the catalog. Index into the
/// slice returned by [`signatures`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignatureId(pub u16);

/// A single carving signature.
pub struct Signature {
    /// Output file kind for this signature.
    pub kind: FileKind,
    /// Stable display name (used in `CarvedFile.signature`).
    pub name: &'static str,
    /// Magic bytes, possibly with wildcards.
    pub magic: &'static [MagicByte],
    /// How far past the candidate start the magic begins. For most formats
    /// this is 0; for MP4/MOV/QT it is 4 because the size word precedes `ftyp`.
    pub magic_offset: usize,
    /// Largest plausible file size we will carve, in bytes.
    pub max_size: u64,
    /// Validator implementation. Borrowed (not boxed) so the table is `'static`.
    pub validator: &'static (dyn Validator + Sync),
}

impl std::fmt::Debug for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signature")
            .field("kind", &self.kind)
            .field("name", &self.name)
            .field("magic_len", &self.magic.len())
            .field("magic_offset", &self.magic_offset)
            .field("max_size", &self.max_size)
            .finish()
    }
}

impl Signature {
    /// Position within `magic` of the first non-wildcard byte. Used as the
    /// anchor for the first-byte index. Returns 0 if the entire magic is
    /// wildcard (which we never construct).
    #[must_use]
    pub fn anchor_idx(&self) -> usize {
        self.magic
            .iter()
            .position(Option::is_some)
            .unwrap_or(0)
    }

    /// The actual byte at the anchor position. Panics if all wildcard, which
    /// is forbidden by construction.
    #[must_use]
    pub fn anchor_byte(&self) -> u8 {
        self.magic[self.anchor_idx()].expect("signatures must contain at least one fixed byte")
    }

    /// Where (relative to a buffer position `p` that hit the anchor byte) the
    /// candidate file begins.
    #[must_use]
    pub fn candidate_offset_from_anchor(&self) -> usize {
        self.magic_offset + self.anchor_idx()
    }

    /// Compare the magic against `buf` starting at `start` (i.e. the start of
    /// the candidate region; `magic` is checked at `start + magic_offset`).
    /// Returns `false` on out-of-bounds.
    #[must_use]
    pub fn matches(&self, buf: &[u8], start: usize) -> bool {
        let mstart = match start.checked_add(self.magic_offset) {
            Some(v) => v,
            None => return false,
        };
        let mend = match mstart.checked_add(self.magic.len()) {
            Some(v) => v,
            None => return false,
        };
        if mend > buf.len() {
            return false;
        }
        let region = &buf[mstart..mend];
        for (m, b) in self.magic.iter().zip(region.iter()) {
            if let Some(want) = m {
                if want != b {
                    return false;
                }
            }
        }
        true
    }
}

// ---------- the validator instances (one allocation per format) ----------

static V_JPEG: JpegValidator = JpegValidator;
static V_PNG: PngValidator = PngValidator;
static V_GIF: GifValidator = GifValidator;
static V_BMP: BmpValidator = BmpValidator;
static V_TIFF: TiffValidator = TiffValidator;
static V_MP4: Mp4Validator = Mp4Validator;
static V_MKV: MkvValidator = MkvValidator;
static V_AVI: RiffAviValidator = RiffAviValidator;
static V_PDF: PdfValidator = PdfValidator;
static V_ZIP: ZipValidator = ZipValidator;
static V_OOXML: OoxmlValidator = OoxmlValidator;
static V_RAR: RarValidator = RarValidator;
static V_SEVENZ: SevenZValidator = SevenZValidator;
static V_PSD: PsdValidator = PsdValidator;
static V_TXT: TextValidator = validators::TextValidator::new(FileKind::Txt);
static V_CSV: TextValidator = validators::TextValidator::new(FileKind::Csv);
static V_SQL: TextValidator = validators::TextValidator::new(FileKind::Sql);

// ---------- helpers for building magic patterns ----------

/// Build a magic from a `&[u8]` literal (no wildcards).
const fn fixed(bytes: &'static [u8]) -> &'static [MagicByte] {
    // Encode each byte as Some(b) at compile time. `const` evaluation cannot
    // build a `Vec`, so we instead store fully-fixed magics as `&[MagicByte]`
    // arrays declared inline below.
    bytes_to_magic(bytes)
}

const fn bytes_to_magic(_b: &'static [u8]) -> &'static [MagicByte] {
    // Stub — replaced below per signature with hand-built static arrays so
    // each entry stays `const`.
    panic!("use the per-signature static MAGIC_*: &[MagicByte] arrays")
}

// fully-fixed magics
const M_JPG: &[MagicByte] = &[Some(0xFF), Some(0xD8), Some(0xFF)];
const M_PNG: &[MagicByte] = &[
    Some(0x89),
    Some(0x50),
    Some(0x4E),
    Some(0x47),
    Some(0x0D),
    Some(0x0A),
    Some(0x1A),
    Some(0x0A),
];
const M_GIF87: &[MagicByte] = &[
    Some(b'G'),
    Some(b'I'),
    Some(b'F'),
    Some(b'8'),
    Some(b'7'),
    Some(b'a'),
];
const M_GIF89: &[MagicByte] = &[
    Some(b'G'),
    Some(b'I'),
    Some(b'F'),
    Some(b'8'),
    Some(b'9'),
    Some(b'a'),
];
const M_BMP: &[MagicByte] = &[Some(b'B'), Some(b'M')];
const M_TIFF_LE: &[MagicByte] = &[Some(0x49), Some(0x49), Some(0x2A), Some(0x00)];
const M_TIFF_BE: &[MagicByte] = &[Some(0x4D), Some(0x4D), Some(0x00), Some(0x2A)];
// MP4: bytes 4..8 are "ftyp"; bytes 0..4 are big-endian atom size (wildcard).
const M_MP4: &[MagicByte] = &[Some(b'f'), Some(b't'), Some(b'y'), Some(b'p')];
const M_MKV: &[MagicByte] = &[Some(0x1A), Some(0x45), Some(0xDF), Some(0xA3)];
const M_RIFF_AVI: &[MagicByte] = &[
    Some(b'R'),
    Some(b'I'),
    Some(b'F'),
    Some(b'F'),
    None,
    None,
    None,
    None,
    Some(b'A'),
    Some(b'V'),
    Some(b'I'),
    Some(b' '),
];
const M_PDF: &[MagicByte] = &[Some(b'%'), Some(b'P'), Some(b'D'), Some(b'F'), Some(b'-')];
const M_ZIP_LFH: &[MagicByte] = &[Some(0x50), Some(0x4B), Some(0x03), Some(0x04)];
const M_RAR4: &[MagicByte] = &[
    Some(0x52),
    Some(0x61),
    Some(0x72),
    Some(0x21),
    Some(0x1A),
    Some(0x07),
    Some(0x00),
];
const M_RAR5: &[MagicByte] = &[
    Some(0x52),
    Some(0x61),
    Some(0x72),
    Some(0x21),
    Some(0x1A),
    Some(0x07),
    Some(0x01),
    Some(0x00),
];
const M_7Z: &[MagicByte] = &[
    Some(0x37),
    Some(0x7A),
    Some(0xBC),
    Some(0xAF),
    Some(0x27),
    Some(0x1C),
];
const M_PSD: &[MagicByte] = &[Some(b'8'), Some(b'B'), Some(b'P'), Some(b'S')];

// silence unused warnings on the placeholder helpers above
const _: fn(&'static [u8]) -> &'static [MagicByte] = fixed;

// sizes
const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;

/// Returns the static signature catalog. Validator borrow is `'static`.
#[must_use]
pub fn signatures() -> &'static [Signature] {
    &SIGNATURES
}

static SHARED_INDEX: OnceLock<SignatureIndex> = OnceLock::new();

/// Returns a lazily-initialized, shared signature index (built once per process).
#[must_use]
pub fn shared_index() -> &'static SignatureIndex {
    SHARED_INDEX.get_or_init(SignatureIndex::build)
}

#[allow(clippy::declare_interior_mutable_const)]
static SIGNATURES: [Signature; 19] = [
    Signature {
        kind: FileKind::Jpg,
        name: "jpg",
        magic: M_JPG,
        magic_offset: 0,
        max_size: 64 * MIB,
        validator: &V_JPEG,
    },
    Signature {
        kind: FileKind::Png,
        name: "png",
        magic: M_PNG,
        magic_offset: 0,
        max_size: 256 * MIB,
        validator: &V_PNG,
    },
    Signature {
        kind: FileKind::Gif,
        name: "gif87a",
        magic: M_GIF87,
        magic_offset: 0,
        max_size: 32 * MIB,
        validator: &V_GIF,
    },
    Signature {
        kind: FileKind::Gif,
        name: "gif89a",
        magic: M_GIF89,
        magic_offset: 0,
        max_size: 32 * MIB,
        validator: &V_GIF,
    },
    Signature {
        kind: FileKind::Bmp,
        name: "bmp",
        magic: M_BMP,
        magic_offset: 0,
        max_size: GIB,
        validator: &V_BMP,
    },
    Signature {
        kind: FileKind::Tiff,
        name: "tiff-le",
        magic: M_TIFF_LE,
        magic_offset: 0,
        max_size: 4 * GIB,
        validator: &V_TIFF,
    },
    Signature {
        kind: FileKind::Tiff,
        name: "tiff-be",
        magic: M_TIFF_BE,
        magic_offset: 0,
        max_size: 4 * GIB,
        validator: &V_TIFF,
    },
    Signature {
        kind: FileKind::Mp4,
        name: "mp4",
        magic: M_MP4,
        magic_offset: 4,
        max_size: 16 * GIB,
        validator: &V_MP4,
    },
    Signature {
        kind: FileKind::Mkv,
        name: "mkv",
        magic: M_MKV,
        magic_offset: 0,
        max_size: 16 * GIB,
        validator: &V_MKV,
    },
    Signature {
        kind: FileKind::Avi,
        name: "avi",
        magic: M_RIFF_AVI,
        magic_offset: 0,
        max_size: 16 * GIB,
        validator: &V_AVI,
    },
    Signature {
        kind: FileKind::Pdf,
        name: "pdf",
        magic: M_PDF,
        magic_offset: 0,
        max_size: GIB,
        validator: &V_PDF,
    },
    // ZIP local-file-header — disambiguated into OOXML or plain ZIP by inspection.
    // OOXML must come first because it's a strict superset (a ZIP with a known
    // [Content_Types].xml entry).
    Signature {
        kind: FileKind::Docx, // overridden by validator
        name: "ooxml",
        magic: M_ZIP_LFH,
        magic_offset: 0,
        max_size: 256 * MIB,
        validator: &V_OOXML,
    },
    Signature {
        kind: FileKind::Zip,
        name: "zip",
        magic: M_ZIP_LFH,
        magic_offset: 0,
        max_size: 4 * GIB,
        validator: &V_ZIP,
    },
    Signature {
        kind: FileKind::Rar,
        name: "rar4",
        magic: M_RAR4,
        magic_offset: 0,
        max_size: 4 * GIB,
        validator: &V_RAR,
    },
    Signature {
        kind: FileKind::Rar,
        name: "rar5",
        magic: M_RAR5,
        magic_offset: 0,
        max_size: 4 * GIB,
        validator: &V_RAR,
    },
    Signature {
        kind: FileKind::SevenZ,
        name: "7z",
        magic: M_7Z,
        magic_offset: 0,
        max_size: 4 * GIB,
        validator: &V_SEVENZ,
    },
    Signature {
        kind: FileKind::Psd,
        name: "psd",
        magic: M_PSD,
        magic_offset: 0,
        max_size: 2 * GIB,
        validator: &V_PSD,
    },
    // Heuristic text. These are zero-magic — enabled via a separate scan path
    // (see scanner::scan_text_window). They are NOT placed in the first-byte
    // index. Listed here only so callers can introspect supported kinds.
    Signature {
        kind: FileKind::Txt,
        name: "txt",
        magic: &[Some(0)], // sentinel; never used by the index
        magic_offset: 0,
        max_size: 16 * MIB,
        validator: &V_TXT,
    },
    Signature {
        kind: FileKind::Csv,
        name: "csv",
        magic: &[Some(0)],
        magic_offset: 0,
        max_size: 64 * MIB,
        validator: &V_CSV,
    },
];

// SQL is tracked but not in the active table to keep the array length stable.
// Use validators::TextValidator::new(FileKind::Sql) for ad-hoc invocation.
#[doc(hidden)]
pub fn sql_validator() -> &'static TextValidator {
    &V_SQL
}

/// First-byte index over [`signatures`]: for each byte value, the list of
/// signature indices whose anchor byte equals that value.
#[derive(Debug)]
pub struct SignatureIndex {
    by_anchor: [Vec<u16>; 256],
    /// Indices that participate in regular (magic-driven) scanning. Excludes
    /// heuristic text formats.
    pub magic_driven: Vec<u16>,
    /// Largest `magic_offset + magic.len()` across all signatures — the
    /// minimum number of trailing bytes a buffer must have past a candidate
    /// position before that position can be tested.
    pub max_magic_extent: usize,
}

impl SignatureIndex {
    #[must_use]
    pub fn build() -> Self {
        // SAFETY (no unsafe): we initialize via from_fn.
        let mut by_anchor: [Vec<u16>; 256] = std::array::from_fn(|_| Vec::new());
        let mut magic_driven = Vec::new();
        let mut max_extent = 0usize;
        for (idx, sig) in signatures().iter().enumerate() {
            // Skip heuristic-only entries (sentinel single-byte magic of 0x00
            // at offset 0 that the docs never permit at the start of a real
            // file is our marker).
            if matches!(sig.kind, FileKind::Txt | FileKind::Csv | FileKind::Sql) {
                continue;
            }
            let id = u16::try_from(idx).expect("signature catalog smaller than u16::MAX");
            magic_driven.push(id);
            by_anchor[sig.anchor_byte() as usize].push(id);
            let extent = sig.magic_offset + sig.magic.len();
            if extent > max_extent {
                max_extent = extent;
            }
        }
        Self {
            by_anchor,
            magic_driven,
            max_magic_extent: max_extent,
        }
    }

    /// Signatures whose anchor byte equals `b`.
    #[must_use]
    pub fn candidates(&self, b: u8) -> &[u16] {
        &self.by_anchor[b as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_signature_has_at_least_one_fixed_byte() {
        for s in signatures() {
            assert!(
                s.magic.iter().any(Option::is_some),
                "signature {} has no fixed byte",
                s.name
            );
        }
    }

    #[test]
    fn jpg_magic_matches_at_zero() {
        let sig = &signatures()[0];
        assert_eq!(sig.kind, FileKind::Jpg);
        let buf = [0xFF, 0xD8, 0xFF, 0xE0, 0x00];
        assert!(sig.matches(&buf, 0));
    }

    #[test]
    fn mp4_magic_matches_with_offset_4() {
        let mp4_idx = signatures()
            .iter()
            .position(|s| s.name == "mp4")
            .unwrap();
        let sig = &signatures()[mp4_idx];
        // 4 bytes of size, then "ftyp"
        let buf = [
            0x00, 0x00, 0x00, 0x20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm',
        ];
        assert!(sig.matches(&buf, 0));
        assert_eq!(sig.candidate_offset_from_anchor(), 4);
        assert_eq!(sig.anchor_byte(), b'f');
    }

    #[test]
    fn riff_avi_uses_wildcards() {
        let avi = signatures()
            .iter()
            .find(|s| s.name == "avi")
            .unwrap();
        let mut buf = b"RIFF\x00\x00\x00\x00AVI \x00".to_vec();
        buf.extend_from_slice(&[0u8; 64]);
        assert!(avi.matches(&buf, 0));
    }

    #[test]
    fn signature_index_routes_first_byte() {
        let idx = SignatureIndex::build();
        // 0xFF is the JPEG anchor.
        let cands = idx.candidates(0xFF);
        assert!(!cands.is_empty());
        // 0x00 is no signature's anchor.
        assert!(idx.candidates(0x00).is_empty());
    }

    #[test]
    fn signature_index_excludes_text_formats() {
        let idx = SignatureIndex::build();
        for &id in &idx.magic_driven {
            let s = &signatures()[id as usize];
            assert!(!matches!(
                s.kind,
                FileKind::Txt | FileKind::Csv | FileKind::Sql
            ));
        }
    }
}
