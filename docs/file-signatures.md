# File Signature Reference

This is the reference data used by `tr-carver`. Each signature includes:
- `magic`: bytes that must be at the start of a candidate region
- `magic_offset`: offset within the candidate region (usually 0)
- `footer` (optional): bytes that mark the end
- `max_size`: the largest plausible file size we will carve
- `validator`: the function in `crates/carver/src/validators/` that confirms a hit and returns the precise length

| Type | Magic (hex) | Offset | Footer (hex) | Max size  | Validator           |
| ---- | ----------- | ------ | ------------ | --------- | ------------------- |
| JPG  | `FF D8 FF`  | 0      | `FF D9`      | 64 MiB    | `validators::jpeg`  |
| PNG  | `89 50 4E 47 0D 0A 1A 0A` | 0 | `49 45 4E 44 AE 42 60 82` | 256 MiB | `validators::png`  |
| GIF  | `47 49 46 38 [37\|39] 61` | 0 | `00 3B`     | 32 MiB    | `validators::gif`   |
| BMP  | `42 4D`     | 0      | (length in header) | 1 GiB | `validators::bmp`   |
| TIFF | `49 49 2A 00` or `4D 4D 00 2A` | 0 | (IFD-driven) | 4 GiB | `validators::tiff` |
| MP4  | `?? ?? ?? ?? 66 74 79 70`     | 4 | (atom chain) | 16 GiB | `validators::mp4`   |
| MOV  | `?? ?? ?? ?? 66 74 79 70 71 74` | 4 | (atom chain) | 16 GiB | `validators::mp4`  |
| MKV  | `1A 45 DF A3` | 0    | (EBML-driven) | 16 GiB    | `validators::mkv`   |
| AVI  | `52 49 46 46 ?? ?? ?? ?? 41 56 49 20` | 0 | (RIFF size) | 16 GiB | `validators::riff` |
| PDF  | `25 50 44 46 2D` | 0  | `25 25 45 4F 46` | 1 GiB | `validators::pdf`   |
| DOCX | `50 4B 03 04` | 0    | `50 4B 05 06` | 256 MiB  | `validators::ooxml` |
| XLSX | `50 4B 03 04` | 0    | `50 4B 05 06` | 256 MiB  | `validators::ooxml` |
| PPTX | `50 4B 03 04` | 0    | `50 4B 05 06` | 256 MiB  | `validators::ooxml` |
| ZIP  | `50 4B 03 04` | 0    | `50 4B 05 06` | 4 GiB    | `validators::zip`   |
| RAR4 | `52 61 72 21 1A 07 00` | 0 | (vol-end record) | 4 GiB | `validators::rar` |
| RAR5 | `52 61 72 21 1A 07 01 00` | 0 | (vol-end record) | 4 GiB | `validators::rar` |
| 7Z   | `37 7A BC AF 27 1C` | 0 | (length in header) | 4 GiB | `validators::sevenz` |
| PSD  | `38 42 50 53` | 0    | (length in header) | 2 GiB | `validators::psd`   |
| AI   | (PDF stream — handled by PDF validator with `/Creator (Adobe Illustrator)`) | | | 1 GiB | `validators::pdf` |
| TXT  | (heuristic) | n/a  | (printable density) | 16 MiB | `validators::text`  |
| CSV  | (heuristic) | n/a  | (printable density + delimiter) | 64 MiB | `validators::csv`   |
| SQL  | (heuristic) | n/a  | (printable density + keywords)  | 64 MiB | `validators::sql`   |

## Validator contract

```rust
pub trait Validator: Sync + Send {
    /// Given a window starting at the candidate magic, return the validated
    /// file length (in bytes). Returns None if the candidate is a false hit.
    fn validate(&self, window: &[u8]) -> Option<usize>;

    fn extension(&self) -> &'static str;
    fn kind(&self) -> FileKind;
}
```

A validator must be **read-only**, **bounded** (no unbounded recursion), and **side-effect free**. Validators run inside the rayon pool.
