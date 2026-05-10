//! Chunked async scanner over a [`SectorReader`].
//!
//! The scanner reads the device in fixed-size chunks with a "head overlap"
//! large enough to contain the longest signature magic plus enough lookahead
//! for header-driven validators. When a candidate is found, the scanner
//! attempts validation with a window starting at the candidate offset and
//! growing up to the signature's `max_size` (capped at a configurable budget,
//! since we cannot load multi-GiB windows entirely into RAM).
//!
//! Emission goes through an async [`tokio::sync::mpsc::Sender`] so the
//! recovery-engine can stream `CarvedFile` results to SQLite.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::signature::{Signature, SignatureIndex};
use crate::Validation;
use tokio::sync::mpsc::Sender;
use tr_core::{CarvedFile, Error, FileKind, Result};
use tr_storage::SectorReader;

/// Tunable scanner parameters.
#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// Bytes per chunk read from the device.
    pub chunk_size: usize,
    /// Size of the head overlap kept at the front of each chunk so candidates
    /// straddling a chunk boundary are not missed. Must be ≥ longest magic.
    pub overlap: usize,
    /// Initial validation window size. Validators returning `NeedsMore` cause
    /// the scanner to grow this up to `max_validation_window` (or the
    /// signature-specific `max_size`, whichever is smaller).
    pub initial_validation_window: usize,
    /// Hard cap on a single validation window — keeps RAM bounded even when
    /// a signature claims a huge `max_size`.
    pub max_validation_window: usize,
    /// If `Some`, restrict carving to these file kinds. Empty / `None` = all.
    pub kinds: Option<Vec<FileKind>>,
    /// If `Some`, drop carved files smaller than this many bytes.
    pub min_carve_bytes: u64,
    /// Skip this many bytes after a confirmed carve (anti-overlap). Set to 0
    /// to keep scanning every byte, accepting some duplicate hits.
    pub skip_after_hit: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            chunk_size: 4 * 1024 * 1024,
            overlap: 64 * 1024,
            initial_validation_window: 1024 * 1024,
            max_validation_window: 64 * 1024 * 1024,
            kinds: None,
            min_carve_bytes: 0,
            skip_after_hit: true,
        }
    }
}

/// Aggregate statistics returned at the end of a scan.
#[derive(Debug, Default, Clone)]
pub struct ScanStats {
    pub bytes_scanned: u64,
    pub candidates_examined: u64,
    pub files_confirmed: u64,
    pub rejections: u64,
    pub needs_more_giveups: u64,
}

/// Cancellation token. Cheap to clone; flips one atomic.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// The carver. Owns a reader and the signature index; thread-safe.
#[derive(Debug)]
pub struct Carver {
    reader: Arc<dyn SectorReader>,
    index: SignatureIndex,
    config: ScanConfig,
}

impl Carver {
    #[must_use]
    pub fn new(reader: Arc<dyn SectorReader>, config: ScanConfig) -> Self {
        let index = SignatureIndex::build();
        Self {
            reader,
            index,
            config,
        }
    }

    /// Scan the byte range `[start, end)` of the underlying device, emitting
    /// `CarvedFile` records into `tx` as they are confirmed.
    pub async fn scan_range(
        &self,
        start: u64,
        end: u64,
        tx: Sender<CarvedFile>,
        cancel: CancelToken,
    ) -> Result<ScanStats> {
        if end <= start {
            return Ok(ScanStats::default());
        }
        let device_end = self.reader.size_bytes();
        let end = end.min(device_end);
        if start >= end {
            return Ok(ScanStats::default());
        }

        let mut stats = ScanStats::default();
        let chunk_size = self.config.chunk_size;
        let overlap = self.config.overlap.max(self.index.max_magic_extent);

        // Read buffer is `chunk_size + overlap` so we can feed one chunk's
        // worth of bytes per iteration while keeping `overlap` from the
        // previous chunk in front.
        let mut buf = vec![0u8; chunk_size + overlap];
        // Absolute byte offset of `buf[0]`.
        let mut buf_origin: u64 = start;
        // Initial fill. `buf_len` is the number of valid bytes in `buf`.
        let mut buf_len: usize = self
            .reader
            .read_at(start, &mut buf[..chunk_size])
            .await?;

        while buf_len > 0 {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }

            // We may scan up to `buf_len - overlap` bytes; the trailing
            // `overlap` bytes are kept for the next chunk so that a candidate
            // beginning within them can be tested against bytes that follow.
            // For the LAST iteration (no more bytes available) we scan up to
            // buf_len to not miss a hit at the very end.
            let mut scan_end = buf_len;
            // Probe whether more data exists; we read it lazily below.
            let next_offset = buf_origin
                .checked_add(buf_len as u64)
                .unwrap_or(u64::MAX);
            let more_available = next_offset < end;
            if more_available && buf_len > overlap {
                scan_end = buf_len - overlap;
            }

            // Walk the buffer.
            let mut p = 0usize;
            while p < scan_end {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                let candidates = self.index.candidates(buf[p]);
                if candidates.is_empty() {
                    p += 1;
                    continue;
                }
                let mut hit_len: Option<u64> = None;
                let mut hit_kind: Option<FileKind> = None;
                let mut hit_name: Option<&'static str> = None;
                let mut hit_recoverability: u8 = 0;
                let mut hit_abs_start: u64 = 0;
                for &id in candidates {
                    let sig = &crate::signature::signatures()[id as usize];
                    if !self.kind_enabled(sig.kind) {
                        continue;
                    }
                    // The candidate file starts at `p - candidate_offset_from_anchor`.
                    let cof = sig.candidate_offset_from_anchor();
                    if p < cof {
                        continue;
                    }
                    let cstart = p - cof;
                    if !sig.matches(&buf[..buf_len], cstart) {
                        continue;
                    }
                    stats.candidates_examined += 1;
                    let abs_start = buf_origin + cstart as u64;

                    match self
                        .try_validate(sig, abs_start, &buf[cstart..buf_len])
                        .await?
                    {
                        Some((len, kind, recov)) => {
                            if len >= self.config.min_carve_bytes {
                                hit_len = Some(len);
                                hit_kind = Some(kind);
                                hit_name = Some(sig.name);
                                hit_recoverability = recov;
                                hit_abs_start = abs_start;
                                stats.files_confirmed += 1;
                                break; // first matching signature wins
                            }
                            stats.rejections += 1;
                        }
                        None => {
                            stats.rejections += 1;
                        }
                    }
                }

                if let (Some(len), Some(kind), Some(name)) = (hit_len, hit_kind, hit_name) {
                    let abs_start = hit_abs_start;
                    let cf = CarvedFile {
                        kind,
                        offset_bytes: abs_start,
                        length_bytes: len,
                        signature: name.to_string(),
                        recoverability: hit_recoverability,
                    };
                    if tx.send(cf).await.is_err() {
                        // Receiver dropped — cancel the scan.
                        return Ok(stats);
                    }
                    if self.config.skip_after_hit {
                        // Move past the entire confirmed region.
                        let abs_after = abs_start + len;
                        // If the carved file extends beyond the current
                        // buffer, advance buf_origin and refill.
                        if abs_after >= buf_origin + buf_len as u64 {
                            // Drop entire current buffer; reseed past abs_after.
                            buf_origin = abs_after.min(end);
                            buf_len = 0;
                            break;
                        }
                        p = (abs_after - buf_origin) as usize;
                    } else {
                        p += 1;
                    }
                } else {
                    p += 1;
                }
            }

            stats.bytes_scanned += scan_end as u64;

            // Refill: shift the unread tail (`buf[scan_end..buf_len]`) to the
            // front, then read more bytes after it.
            if buf_len == 0 {
                if buf_origin >= end {
                    break;
                }
                let n = self
                    .reader
                    .read_at(
                        buf_origin,
                        &mut buf[..chunk_size.min((end - buf_origin) as usize)],
                    )
                    .await?;
                if n == 0 {
                    break;
                }
                buf_len = n;
                continue;
            }

            let tail = buf_len - scan_end;
            buf.copy_within(scan_end..buf_len, 0);
            buf_origin += scan_end as u64;
            buf_len = tail;
            // How much room do we have to read more?
            let want = (chunk_size + overlap).saturating_sub(buf_len);
            let remaining = end.saturating_sub(buf_origin + buf_len as u64);
            let to_read = (want as u64).min(remaining) as usize;
            if to_read == 0 {
                // No more data; let the loop condition re-evaluate scan_end
                // for the final pass.
                if !more_available {
                    // Scanned what we had and there's nothing else — done.
                    break;
                }
                continue;
            }
            let n = self
                .reader
                .read_at(buf_origin + buf_len as u64, &mut buf[buf_len..buf_len + to_read])
                .await?;
            if n == 0 {
                // Treat short reads at EOF: do one final scan of what we have.
                if !more_available {
                    break;
                }
            }
            buf_len += n;
        }

        Ok(stats)
    }

    fn kind_enabled(&self, kind: FileKind) -> bool {
        match &self.config.kinds {
            None => true,
            Some(v) if v.is_empty() => true,
            Some(v) => v.contains(&kind),
        }
    }

    /// Run the validator against a window starting at `cstart_abs`. Grows the
    /// window adaptively up to the signature's `max_size` (or
    /// `max_validation_window`) when the validator asks for more.
    async fn try_validate(
        &self,
        sig: &'static Signature,
        cstart_abs: u64,
        initial: &[u8],
    ) -> Result<Option<(u64, FileKind, u8)>> {
        // Try with the bytes already in the chunk buffer first.
        let max_window =
            (sig.max_size.min(self.config.max_validation_window as u64)) as usize;
        match sig.validator.validate_with_kind(initial) {
            (Validation::Confirmed { length, recoverability }, kind) => {
                return Ok(Some((length, kind, recoverability)));
            }
            (Validation::Rejected, _) => return Ok(None),
            (Validation::NeedsMore, _) => {}
        }
        // Grow the window: read up to `max_window` from the device.
        if initial.len() >= max_window {
            return Ok(None);
        }
        // Pre-allocate the window and a reusable read buffer to avoid
        // per-iteration heap allocations.
        let initial_cap = max_window.min(self.config.initial_validation_window);
        let mut window = Vec::with_capacity(initial_cap);
        window.extend_from_slice(initial);
        let mut tmp = vec![0u8; self.config.initial_validation_window];
        let mut size = self
            .config
            .initial_validation_window
            .max(initial.len() * 2)
            .min(max_window);
        while size > window.len() {
            let need = size - window.len();
            // Grow tmp only if needed (rare: only when window doubles past initial)
            if need > tmp.len() {
                tmp.resize(need, 0);
            }
            let read_off = cstart_abs + window.len() as u64;
            let n = self.reader.read_at(read_off, &mut tmp[..need]).await?;
            if n == 0 {
                break;
            }
            window.extend_from_slice(&tmp[..n]);
            match sig.validator.validate_with_kind(&window) {
                (Validation::Confirmed { length, recoverability }, kind) => {
                    return Ok(Some((length, kind, recoverability)));
                }
                (Validation::Rejected, _) => return Ok(None),
                (Validation::NeedsMore, _) => {
                    if size >= max_window {
                        return Ok(None);
                    }
                    size = (size * 2).min(max_window);
                }
            }
        }
        // Final attempt at full window.
        match sig.validator.validate_with_kind(&window) {
            (Validation::Confirmed { length, recoverability }, kind) => {
                Ok(Some((length, kind, recoverability)))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tr_storage::FixtureReader;

    /// Embed a known-good JPEG into a sea of zeros and confirm the scanner
    /// finds it.
    #[tokio::test]
    async fn scans_one_jpeg_in_zero_padding() {
        // Build a minimal JPEG.
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        jpeg.extend_from_slice(b"JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00");
        jpeg.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x04, 0x00, 0x00]);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x04, 0x00, 0x00]);
        jpeg.extend_from_slice(&[0x12, 0x34, 0xFF, 0x00, 0x56]);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);

        let mut img = vec![0u8; 1 * 1024 * 1024];
        let off = 256 * 1024usize;
        img[off..off + jpeg.len()].copy_from_slice(&jpeg);

        let reader: Arc<dyn SectorReader> =
            Arc::new(FixtureReader::new("test", 512, img));
        let cfg = ScanConfig {
            chunk_size: 128 * 1024,
            overlap: 8 * 1024,
            ..Default::default()
        };
        let carver = Carver::new(reader.clone(), cfg);
        let (tx, mut rx) = mpsc::channel(8);
        let handle = tokio::spawn(async move {
            carver
                .scan_range(0, reader.size_bytes(), tx, CancelToken::new())
                .await
        });
        let mut found = Vec::new();
        while let Some(c) = rx.recv().await {
            found.push(c);
        }
        let stats = handle.await.unwrap().unwrap();
        assert_eq!(found.len(), 1, "expected one carved jpeg, got {found:?}");
        assert_eq!(found[0].kind, FileKind::Jpg);
        assert_eq!(found[0].offset_bytes, off as u64);
        assert_eq!(found[0].length_bytes as usize, jpeg.len());
        assert!(stats.files_confirmed >= 1);
    }

    #[tokio::test]
    async fn cancellation_terminates_scan() {
        let img = vec![0u8; 4 * 1024 * 1024];
        let reader: Arc<dyn SectorReader> = Arc::new(FixtureReader::new("z", 512, img));
        let cancel = CancelToken::new();
        cancel.cancel();
        let carver = Carver::new(reader.clone(), ScanConfig::default());
        let (tx, _rx) = mpsc::channel(1);
        let r = carver
            .scan_range(0, reader.size_bytes(), tx, cancel)
            .await;
        assert!(matches!(r, Err(Error::Cancelled)));
    }
}
