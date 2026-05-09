//! FAT32 read-only parser. Walks the root directory and recursively each
//! sub-directory; emits both allocated and deleted (`0xE5`) entries.

pub mod bpb;
pub mod dir;
pub mod fat;

use crate::fat::bpb::Fat32Bpb;
use crate::fat::dir::{DirEntryRaw, DirIter, FatDirEntry};
use std::sync::Arc;
use tr_core::{DataRun, Error, FileKind, FileRecord, FileSource, PartitionInfo, Result};
use tr_storage::{SectorReader, SectorReaderExt};

#[derive(Debug)]
pub struct Fat32Volume {
    pub bpb: Fat32Bpb,
    pub partition: PartitionInfo,
    reader: Arc<dyn SectorReader>,
}

impl Fat32Volume {
    pub async fn open(
        reader: Arc<dyn SectorReader>,
        partition: PartitionInfo,
    ) -> Result<Self> {
        let bs = reader.read_lba(partition.start_lba).await?;
        let bpb = Fat32Bpb::parse(&bs, partition.start_lba)?;
        Ok(Self {
            bpb,
            partition,
            reader,
        })
    }

    /// Convert a FAT32 cluster number to its absolute byte offset on the
    /// device.
    fn cluster_byte_offset(&self, cluster: u32) -> u64 {
        let bytes_per_sector = u64::from(self.bpb.bytes_per_sector);
        let reserved = u64::from(self.bpb.reserved_sectors);
        let fat_total = u64::from(self.bpb.fat_size_sectors) * u64::from(self.bpb.num_fats);
        let data_start_sector = reserved + fat_total;
        let cluster_offset = (u64::from(cluster) - 2)
            * u64::from(self.bpb.sectors_per_cluster);
        let lba = self.partition.start_lba + data_start_sector + cluster_offset;
        lba * bytes_per_sector
    }

    fn cluster_byte_size(&self) -> u64 {
        u64::from(self.bpb.bytes_per_sector) * u64::from(self.bpb.sectors_per_cluster)
    }

    /// Read the FAT chain starting at `first_cluster` and return as data runs.
    /// Stops at end-of-chain marker, bad cluster, or `max_clusters`.
    pub async fn cluster_chain(&self, first_cluster: u32, max_clusters: u32) -> Result<Vec<DataRun>> {
        let mut runs: Vec<DataRun> = Vec::new();
        let mut cluster = first_cluster;
        let mut count = 0u32;

        let bytes_per_sector = u64::from(self.bpb.bytes_per_sector);
        let fat_byte_offset = u64::from(self.bpb.reserved_sectors) * bytes_per_sector
            + self.partition.start_lba * bytes_per_sector;

        // Streamed read: 4 KiB FAT page covers 1024 cluster entries.
        const FAT_PAGE: usize = 4096;
        let mut page_base: u64 = u64::MAX;
        let mut page = vec![0u8; FAT_PAGE];

        while count < max_clusters {
            if cluster < 2 || cluster >= 0x0FFF_FFF8 {
                break;
            }
            let entry_byte = fat_byte_offset + u64::from(cluster) * 4;
            let entry_page = entry_byte & !(FAT_PAGE as u64 - 1);
            if page_base != entry_page {
                let n = self.reader.read_at(entry_page, &mut page).await?;
                if n == 0 {
                    break;
                }
                page_base = entry_page;
            }
            let off = (entry_byte - entry_page) as usize;
            let next = u32::from_le_bytes([page[off], page[off + 1], page[off + 2], page[off + 3]])
                & 0x0FFF_FFFF;

            let cluster_lba = (self.cluster_byte_offset(cluster)) / bytes_per_sector;
            let length = u64::from(self.bpb.sectors_per_cluster);

            // Coalesce contiguous runs.
            if let Some(last) = runs.last_mut() {
                if last.start_lba + last.length_sectors == cluster_lba {
                    last.length_sectors += length;
                } else {
                    runs.push(DataRun {
                        start_lba: cluster_lba,
                        length_sectors: length,
                    });
                }
            } else {
                runs.push(DataRun {
                    start_lba: cluster_lba,
                    length_sectors: length,
                });
            }
            count += 1;
            cluster = next;
        }

        Ok(runs)
    }

    /// Iterate the root directory and return file records.
    pub async fn collect_files(
        &self,
        partition_index: u32,
        max_entries: u64,
    ) -> Result<Vec<FileRecord>> {
        let mut out = Vec::new();
        let cluster_bytes = self.cluster_byte_size();
        let mut buffer = vec![0u8; cluster_bytes as usize];

        let mut clusters_to_visit = vec![self.bpb.root_cluster];
        let mut visited = std::collections::HashSet::new();
        let mut id: u64 = 0;

        while let Some(cluster) = clusters_to_visit.pop() {
            if !visited.insert(cluster) {
                continue;
            }
            if cluster < 2 || cluster >= 0x0FFF_FFF8 {
                continue;
            }
            let off = self.cluster_byte_offset(cluster);
            let n = self.reader.read_at(off, &mut buffer).await?;
            if n == 0 {
                continue;
            }
            let mut iter = DirIter::new(&buffer[..n]);
            while let Some(entry) = iter.next_entry() {
                if out.len() as u64 >= max_entries {
                    break;
                }
                match entry {
                    FatDirEntry::EndOfDirectory => break,
                    FatDirEntry::Volume(_) => continue,
                    FatDirEntry::Subdir(sub) => {
                        if sub.first_cluster >= 2 && sub.name != "." && sub.name != ".." {
                            clusters_to_visit.push(sub.first_cluster);
                        }
                    }
                    FatDirEntry::File(f) => {
                        let kind = crate::ntfs::guess_kind(&f.name);
                        let runs = self
                            .cluster_chain(f.first_cluster, 1024 * 1024)
                            .await
                            .unwrap_or_default();
                        let head_hex = if !runs.is_empty() {
                            // Read first 16 bytes of file for sniffing
                            let first_byte = runs[0].start_lba * u64::from(self.bpb.bytes_per_sector);
                            let mut head = [0u8; 16];
                            let _ = self.reader.read_at(first_byte, &mut head).await;
                            head.iter().map(|b| format!("{b:02X}")).collect()
                        } else {
                            String::new()
                        };
                        id += 1;
                        out.push(FileRecord {
                            id,
                            name: f.name.clone(),
                            path: f.name.clone(),
                            kind,
                            size_bytes: u64::from(f.size_bytes),
                            modified: None,
                            source: FileSource::Filesystem {
                                partition_index,
                                record_id: id,
                                is_deleted: f.is_deleted,
                                is_resident: false,
                                runs,
                            },
                            recoverability: if f.is_deleted { 60 } else { 95 },
                            head_hex,
                        });
                    }
                    FatDirEntry::DeletedRaw(_) => continue,
                }
            }
        }
        Ok(out)
    }
}

// re-export so callers can `use tr_filesystem::fat::Fat32Bpb`.
pub use bpb::Fat32Bpb as ReExportFat32Bpb;
pub use dir::{FileEntry, SubdirEntry};

// Suppress dead-code warning on the unused import above when no consumer pulls it.
#[allow(dead_code)]
const _USE: u8 = 0;

#[allow(dead_code)]
fn _ensure_error_imported() -> Option<Error> {
    None
}

#[allow(dead_code)]
fn _ensure_filekind_imported() -> Option<FileKind> {
    None
}

#[allow(dead_code)]
fn _ensure_dir_iter_imported() -> Option<DirEntryRaw> {
    None
}
