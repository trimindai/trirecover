//! In-memory `SectorReader` for tests and disk-image dev workflows.
//!
//! Loading a `.img` / `.bin` file with this reader is the recommended way to
//! exercise parsers in CI. Real raw-disk I/O cannot run in CI anyway, so
//! every parser test should accept any `SectorReader`.

use crate::SectorReader;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::fs;
use std::path::Path;
use tr_core::{Error, Result};

#[derive(Debug)]
pub struct FixtureReader {
    label: String,
    sector_size: u32,
    data: RwLock<Vec<u8>>,
}

impl FixtureReader {
    #[must_use]
    pub fn new(label: impl Into<String>, sector_size: u32, bytes: Vec<u8>) -> Self {
        Self {
            label: label.into(),
            sector_size,
            data: RwLock::new(bytes),
        }
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref();
        let bytes = fs::read(p)?;
        Ok(Self::new(p.to_string_lossy().to_string(), 512, bytes))
    }

    /// Test helper — replace the backing bytes.
    pub fn set_bytes(&self, bytes: Vec<u8>) {
        *self.data.write() = bytes;
    }
}

#[async_trait]
impl SectorReader for FixtureReader {
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let data = self.data.read();
        let off = usize::try_from(offset)
            .map_err(|_| Error::internal("FixtureReader: offset > usize::MAX"))?;
        if off >= data.len() {
            return Ok(0);
        }
        let n = std::cmp::min(buf.len(), data.len() - off);
        buf[..n].copy_from_slice(&data[off..off + n]);
        Ok(n)
    }

    fn size_bytes(&self) -> u64 {
        self.data.read().len() as u64
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn label(&self) -> &str {
        &self.label
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_within_bounds() {
        let r = FixtureReader::new("t", 512, vec![0xAA; 1024]);
        let mut buf = [0u8; 16];
        let n = r.read_at(100, &mut buf).await.unwrap();
        assert_eq!(n, 16);
        assert!(buf.iter().all(|b| *b == 0xAA));
    }

    #[tokio::test]
    async fn returns_short_at_eof() {
        let r = FixtureReader::new("t", 512, vec![0xAA; 100]);
        let mut buf = [0u8; 50];
        let n = r.read_at(80, &mut buf).await.unwrap();
        assert_eq!(n, 20);
    }

    #[tokio::test]
    async fn returns_zero_past_eof() {
        let r = FixtureReader::new("t", 512, vec![0xAA; 100]);
        let mut buf = [0u8; 50];
        let n = r.read_at(1000, &mut buf).await.unwrap();
        assert_eq!(n, 0);
    }
}
