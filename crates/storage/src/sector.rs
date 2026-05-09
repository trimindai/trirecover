//! The `SectorReader` trait — the entire read-only contract.

use async_trait::async_trait;
use tr_core::Result;

/// A read-only random-access view over a block device or image file.
///
/// **Contract:**
/// - All offsets are absolute byte offsets from sector 0.
/// - `read_at` MUST NOT modify any external state (read-only).
/// - Implementations must be `Send + Sync` so multiple scan workers can share
///   one reader (typically wrapped in `Arc`).
/// - Bad-sector handling is implementation-defined: see [`ReadOptions`].
#[async_trait]
pub trait SectorReader: Send + Sync + std::fmt::Debug {
    /// Read exactly `buf.len()` bytes starting at `offset`. Returns the number
    /// of bytes actually read on success. A short read at EOF is signalled by
    /// returning a value less than `buf.len()`.
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize>;

    /// Read with options (e.g. permitting partial reads through bad sectors).
    async fn read_at_with(
        &self,
        offset: u64,
        buf: &mut [u8],
        _opts: ReadOptions,
    ) -> Result<usize> {
        // default ignores options; concrete impls override
        self.read_at(offset, buf).await
    }

    /// Total length of the underlying device, in bytes.
    fn size_bytes(&self) -> u64;

    /// Logical sector size advertised by the device.
    fn sector_size(&self) -> u32;

    /// Stable identifier used in logs / events.
    fn label(&self) -> &str;
}

/// Knobs controlling how we deal with unreliable media.
#[derive(Debug, Clone, Copy)]
pub struct ReadOptions {
    /// On bad sector: return zeroes for unreadable ranges (default true).
    pub zero_on_bad: bool,
    /// On bad sector: retry with shrinking I/O size down to one sector.
    pub shrink_retry: bool,
    /// Maximum retries per sector.
    pub max_retries: u8,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            zero_on_bad: true,
            shrink_retry: true,
            max_retries: 3,
        }
    }
}

/// Convenience extensions.
#[async_trait]
pub trait SectorReaderExt: SectorReader {
    /// Read `len` bytes starting at `offset` into a fresh `Vec<u8>`.
    async fn read_vec(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut v = vec![0u8; len];
        let n = self.read_at(offset, &mut v).await?;
        v.truncate(n);
        Ok(v)
    }

    /// Read exactly one sector at the given LBA.
    async fn read_lba(&self, lba: u64) -> Result<Vec<u8>> {
        let s = self.sector_size() as usize;
        self.read_vec(lba * s as u64, s).await
    }

    /// Read `count` sectors starting at LBA.
    async fn read_lba_run(&self, lba: u64, count: u64) -> Result<Vec<u8>> {
        let s = self.sector_size() as usize;
        let len = (count as usize)
            .checked_mul(s)
            .ok_or_else(|| tr_core::Error::internal("read_lba_run: length overflow"))?;
        self.read_vec(lba * s as u64, len).await
    }
}

#[async_trait]
impl<T: SectorReader + ?Sized> SectorReaderExt for T {}
