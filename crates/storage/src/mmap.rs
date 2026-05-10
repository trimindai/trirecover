//! Memory-mapped `SectorReader` for disk image files.
//!
//! Uses `memmap2` for zero-copy reads — the OS pages data in on demand,
//! avoiding the explicit read+copy that [`crate::FixtureReader`] does.
//! For multi-GiB images this eliminates the upfront allocation and lets
//! the kernel manage the page cache directly.

use crate::SectorReader;
use async_trait::async_trait;
use memmap2::Mmap;
use std::fs::File;
use std::path::Path;
use tr_core::{Error, Result};

#[derive(Debug)]
pub struct MmapReader {
    label: String,
    mmap: Mmap,
    sector_size: u32,
}

impl MmapReader {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref();
        let file = File::open(p)?;
        // SAFETY: the file is opened read-only and TriRecover never writes
        // to source media. If another process truncates the file while
        // mapped we may get SIGBUS, which is an acceptable trade-off for
        // a recovery tool reading disk images.
        let mmap = unsafe { Mmap::map(&file) }
            .map_err(|e| Error::internal(format!("mmap: {e}")))?;
        Ok(Self {
            label: p.to_string_lossy().to_string(),
            mmap,
            sector_size: 512,
        })
    }
}

#[async_trait]
impl SectorReader for MmapReader {
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let off = offset as usize;
        if off >= self.mmap.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.mmap.len() - off);
        buf[..n].copy_from_slice(&self.mmap[off..off + n]);
        Ok(n)
    }

    fn size_bytes(&self) -> u64 {
        self.mmap.len() as u64
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn label(&self) -> &str {
        &self.label
    }
}
