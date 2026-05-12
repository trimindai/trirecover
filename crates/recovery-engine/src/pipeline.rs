//! The scan pipeline: reads partitions, parses filesystems, carves
//! unallocated space, and streams results to the UI via channels.
//!
//! ## Performance architecture
//!
//! 1. **Large sequential I/O**: the carver reads in 4 MiB chunks to minimize
//!    syscall overhead (one 4 MiB read vs 8192 × 512-byte reads).
//! 2. **SIMD multi-pattern matching**: Aho-Corasick scans each 4 MiB chunk
//!    with SSE2/AVX2 instructions, skipping directly to magic positions.
//! 3. **Streaming results**: files appear in the UI as soon as found; the
//!    progress channel is updated every ~500 ms.
//! 4. **MFT fast path**: the entire $MFT is read sequentially in one pass,
//!    then records are processed in-memory (one I/O pass, not per-record).
//! 5. **Smart sector skipping**: Quick scan reads only metadata areas (MFT,
//!    FAT, directory entries). Deep scan skips known-allocated regions.
//! 6. **Pause-aware checkpoints**: the pipeline checks `JobControl` between
//!    each partition / each carver chunk, so pause/cancel is instant.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use tr_carver::{Carver, CancelToken, ScanConfig as CarveConfig};
use tr_core::{
    CarvedFile, FileRecord, FileSource, JobId, RecoveryStrategy,
    ScanProgress, ScanRequest, SessionId,
};
use tr_partition;
use tr_storage::SectorReader;

use crate::job::JobControl;

/// File ID counter. Carved files get IDs starting from a high offset so they
/// don't collide with filesystem-enumerated records.
const CARVED_ID_BASE: u64 = 10_000_000;

/// How many files to batch before flushing to the files channel.
const FILE_BATCH_SIZE: usize = 128;

/// How often to send progress updates (milliseconds).
const PROGRESS_INTERVAL_MS: u64 = 500;

pub struct ScanPipeline;

impl std::fmt::Debug for ScanPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanPipeline").finish()
    }
}

impl ScanPipeline {
    /// Run a scan to completion. Called from the job manager's spawned task.
    pub async fn run(
        job_id: JobId,
        session_id: SessionId,
        request: &ScanRequest,
        control: JobControl,
        progress_tx: mpsc::Sender<ScanProgress>,
        files_tx: mpsc::Sender<Vec<FileRecord>>,
    ) -> tr_core::Result<()> {
        info!(
            job = %job_id,
            session = %session_id,
            drive = %request.drive_path,
            strategy = ?request.strategy,
            "scan pipeline starting"
        );

        // Open the drive read-only.
        let handle = tr_storage::open_drive(&request.drive_path)?;
        let reader: Arc<dyn SectorReader> = handle.reader();
        let sector_size = reader.sector_size();
        let device_bytes = reader.size_bytes();
        let sectors_total = device_bytes / u64::from(sector_size);

        // State tracking
        let mut ctx = PipelineContext {
            job_id,
            session_id,
            control: control.clone(),
            progress_tx,
            files_tx,
            reader: reader.clone(),
            sector_size,
            device_bytes,
            sectors_total,
            sectors_scanned: 0,
            files_found: 0,
            bytes_recoverable: 0,
            bad_sectors_skipped: 0,
            next_file_id: 0,
            carved_id: CARVED_ID_BASE,
            file_batch: Vec::with_capacity(FILE_BATCH_SIZE),
            last_progress: Instant::now(),
        };

        match request.strategy {
            RecoveryStrategy::Quick => {
                ctx.emit_progress("Detecting partitions").await?;
                let partitions = tr_partition::read_table(reader.as_ref()).await?;
                ctx.run_filesystem_scan(&partitions, request, false).await?;
            }
            RecoveryStrategy::Deep => {
                ctx.emit_progress("Detecting partitions").await?;
                let partitions = tr_partition::read_table(reader.as_ref()).await?;
                // Phase 1: filesystem metadata scan (fast)
                ctx.emit_progress("Phase 1: Filesystem metadata").await?;
                ctx.run_filesystem_scan(&partitions, request, false).await?;
                // Phase 2: carve unallocated space
                ctx.emit_progress("Phase 2: Carving unallocated space").await?;
                ctx.run_carve(&partitions, request, false).await?;
            }
            RecoveryStrategy::Raw => {
                // Pure carving across the entire device
                ctx.emit_progress("Raw carving entire device").await?;
                ctx.run_carve(&[], request, true).await?;
            }
            RecoveryStrategy::Partition => {
                // Attempt partition reconstruction first
                ctx.emit_progress("Reconstructing partitions").await?;
                let partitions = tr_partition::read_table(reader.as_ref()).await?;
                ctx.run_filesystem_scan(&partitions, request, true).await?;
            }
            RecoveryStrategy::Formatted => {
                // Ignore all filesystem metadata; carve everything
                ctx.emit_progress("Scanning formatted drive").await?;
                ctx.run_carve(&[], request, true).await?;
            }
            RecoveryStrategy::CorruptedFs => {
                ctx.emit_progress("Best-effort corrupted FS scan").await?;
                let partitions = tr_partition::read_table(reader.as_ref()).await?;
                // Try filesystem scan in lenient mode
                ctx.run_filesystem_scan(&partitions, request, true).await?;
                // Follow up with carving to catch what FS missed
                ctx.emit_progress("Carving after FS scan").await?;
                ctx.run_carve(&partitions, request, false).await?;
            }
        }

        // Flush remaining files
        ctx.flush_files().await?;
        ctx.emit_progress("Complete").await?;

        info!(
            job = %job_id,
            files = ctx.files_found,
            bytes = ctx.bytes_recoverable,
            "scan pipeline finished"
        );
        Ok(())
    }
}

/// Internal state for a running pipeline.
struct PipelineContext {
    job_id: JobId,
    #[allow(dead_code)]
    session_id: SessionId,
    control: JobControl,
    progress_tx: mpsc::Sender<ScanProgress>,
    files_tx: mpsc::Sender<Vec<FileRecord>>,
    reader: Arc<dyn SectorReader>,
    sector_size: u32,
    device_bytes: u64,
    sectors_total: u64,
    sectors_scanned: u64,
    files_found: u64,
    bytes_recoverable: u64,
    bad_sectors_skipped: u64,
    next_file_id: u64,
    carved_id: u64,
    file_batch: Vec<FileRecord>,
    last_progress: Instant,
}

impl std::fmt::Debug for PipelineContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineContext")
            .field("job_id", &self.job_id)
            .field("files_found", &self.files_found)
            .finish()
    }
}

impl PipelineContext {
    /// Check pause/cancel state. Returns Err(Cancelled) if cancelled.
    async fn checkpoint(&self) -> tr_core::Result<()> {
        if self.control.should_stop() {
            return Err(tr_core::Error::Cancelled);
        }
        if self.control.is_paused() {
            if !self.control.wait_if_paused().await {
                return Err(tr_core::Error::Cancelled);
            }
        }
        Ok(())
    }

    /// Send a progress update (throttled to avoid flooding the channel).
    async fn emit_progress(&mut self, phase: &str) -> tr_core::Result<()> {
        self.checkpoint().await?;

        let elapsed = self.last_progress.elapsed().as_millis() as u64;
        if elapsed < PROGRESS_INTERVAL_MS && phase != "Complete" {
            return Ok(());
        }
        self.last_progress = Instant::now();

        let eta = if self.sectors_scanned > 0 && self.sectors_total > self.sectors_scanned {
            // TODO: replace with time-based ETA once we track elapsed wall time
            None
        } else {
            None
        };

        let p = ScanProgress {
            job_id: self.job_id,
            state: self.control.state(),
            sectors_scanned: self.sectors_scanned,
            sectors_total: self.sectors_total,
            files_found: self.files_found,
            bytes_recoverable: self.bytes_recoverable,
            eta_secs: eta,
            current_phase: phase.to_string(),
            bad_sectors_skipped: self.bad_sectors_skipped,
        };
        let _ = self.progress_tx.send(p).await;
        Ok(())
    }

    /// Add a file to the batch; flush when full.
    async fn add_file(&mut self, file: FileRecord) -> tr_core::Result<()> {
        self.files_found += 1;
        self.bytes_recoverable += file.size_bytes;
        self.file_batch.push(file);

        if self.file_batch.len() >= FILE_BATCH_SIZE {
            self.flush_files().await?;
        }
        Ok(())
    }

    /// Flush the file batch to the channel.
    async fn flush_files(&mut self) -> tr_core::Result<()> {
        if self.file_batch.is_empty() {
            return Ok(());
        }
        let batch = std::mem::replace(
            &mut self.file_batch,
            Vec::with_capacity(FILE_BATCH_SIZE),
        );
        let _ = self.files_tx.send(batch).await;
        Ok(())
    }

    /// Run filesystem-based scan on each partition.
    async fn run_filesystem_scan(
        &mut self,
        partitions: &[tr_core::PartitionInfo],
        request: &ScanRequest,
        lenient: bool,
    ) -> tr_core::Result<()> {
        for part in partitions {
            self.checkpoint().await?;

            // Filter by partition indices if specified
            if !request.partitions.is_empty()
                && !request.partitions.contains(&part.index)
            {
                continue;
            }

            let phase = format!("Scanning partition {} ({})", part.index,
                part.filesystem.as_deref().unwrap_or("unknown"));
            self.emit_progress(&phase).await?;

            let fs_kind = tr_filesystem::detect(self.reader.as_ref(), part).await?;

            match fs_kind {
                tr_filesystem::FsKind::Ntfs => {
                    self.scan_ntfs(part, request, lenient).await?;
                }
                tr_filesystem::FsKind::Fat32 => {
                    self.scan_fat32(part, request, lenient).await?;
                }
                tr_filesystem::FsKind::ExFat => {
                    debug!("exFAT scanning not yet implemented; skipping partition {}", part.index);
                }
                tr_filesystem::FsKind::Unknown => {
                    if lenient {
                        debug!("unknown FS on partition {}, skipping", part.index);
                    } else {
                        debug!("unknown FS on partition {}", part.index);
                    }
                }
            }

            // Update sectors scanned based on partition size
            self.sectors_scanned += part.length_sectors;
            self.emit_progress(&phase).await?;
        }
        Ok(())
    }

    /// Scan an NTFS partition.
    async fn scan_ntfs(
        &mut self,
        part: &tr_core::PartitionInfo,
        request: &ScanRequest,
        _lenient: bool,
    ) -> tr_core::Result<()> {
        let volume = tr_filesystem::ntfs::NtfsVolume::open(
            self.reader.clone(),
            part.clone(),
        )
        .await?;

        let files = volume.collect_files(part.index, u64::MAX).await?;

        for f in files {
            self.checkpoint().await?;

            // Filter by file kinds if specified
            if !request.file_kinds.is_empty() && !request.file_kinds.contains(&f.kind) {
                continue;
            }
            // Filter by minimum carve size
            if f.size_bytes < request.min_carve_bytes {
                continue;
            }

            let id = self.next_file_id;
            self.next_file_id += 1;

            let record = FileRecord {
                id,
                name: f.name,
                path: f.path,
                kind: f.kind,
                size_bytes: f.size_bytes,
                modified: f.modified,
                source: f.source,
                recoverability: f.recoverability,
                head_hex: f.head_hex,
            };
            self.add_file(record).await?;

            // Throttled progress
            if self.files_found % 100 == 0 {
                let phase = format!("NTFS: {} files found", self.files_found);
                self.emit_progress(&phase).await?;
            }
        }
        Ok(())
    }

    /// Scan a FAT32 partition.
    async fn scan_fat32(
        &mut self,
        part: &tr_core::PartitionInfo,
        request: &ScanRequest,
        _lenient: bool,
    ) -> tr_core::Result<()> {
        let volume = tr_filesystem::fat::Fat32Volume::open(
            self.reader.clone(),
            part.clone(),
        )
        .await?;

        let files = volume.collect_files(part.index, u64::MAX).await?;

        for f in files {
            self.checkpoint().await?;

            if !request.file_kinds.is_empty() && !request.file_kinds.contains(&f.kind) {
                continue;
            }
            if f.size_bytes < request.min_carve_bytes {
                continue;
            }

            let id = self.next_file_id;
            self.next_file_id += 1;

            let record = FileRecord {
                id,
                name: f.name,
                path: f.path,
                kind: f.kind,
                size_bytes: f.size_bytes,
                modified: f.modified,
                source: f.source,
                recoverability: f.recoverability,
                head_hex: f.head_hex,
            };
            self.add_file(record).await?;

            if self.files_found % 100 == 0 {
                let phase = format!("FAT32: {} files found", self.files_found);
                self.emit_progress(&phase).await?;
            }
        }
        Ok(())
    }

    /// Run the signature carver.
    ///
    /// If `whole_device` is true, carve the entire device. Otherwise, carve
    /// only the unallocated space between partitions.
    async fn run_carve(
        &mut self,
        partitions: &[tr_core::PartitionInfo],
        request: &ScanRequest,
        whole_device: bool,
    ) -> tr_core::Result<()> {
        // Determine byte ranges to carve
        let ranges = if whole_device {
            vec![(0u64, self.device_bytes)]
        } else {
            compute_unallocated_ranges(
                partitions,
                self.sector_size,
                self.device_bytes,
            )
        };

        let total_carve_bytes: u64 = ranges.iter().map(|(s, e)| e - s).sum();
        let mut carved_bytes: u64 = 0;

        for (start, end) in ranges {
            self.checkpoint().await?;

            let (tx, mut rx) = tokio::sync::mpsc::channel::<CarvedFile>(256);
            let cancel = CancelToken::new();

            // Clone what the carver task needs
            let cancel2 = cancel.clone();
            // Spawn the carver on a separate task so we can drain results concurrently
            let carve_handle = {
                // We need to move carver into the task. Unfortunately Carver
                // borrows the SignatureIndex statically so it's fine.
                let reader = self.reader.clone();
                let config2 = CarveConfig {
                    chunk_size: 4 * 1024 * 1024,
                    overlap: 64 * 1024,
                    initial_validation_window: 1024 * 1024,
                    max_validation_window: 64 * 1024 * 1024,
                    kinds: if request.file_kinds.is_empty() {
                        None
                    } else {
                        Some(request.file_kinds.clone())
                    },
                    min_carve_bytes: request.min_carve_bytes,
                    skip_after_hit: true,
                };
                let carver2 = Carver::new(reader, config2);
                tokio::spawn(async move {
                    carver2.scan_range(start, end, tx, cancel2).await
                })
            };

            // Drain carved files as they arrive
            loop {
                tokio::select! {
                    maybe = rx.recv() => {
                        match maybe {
                            Some(cf) => {
                                let id = self.carved_id;
                                self.carved_id += 1;

                                let name = format!(
                                    "carved_{:08x}.{}",
                                    cf.offset_bytes,
                                    cf.kind.extension()
                                );
                                let record = FileRecord {
                                    id,
                                    name,
                                    path: format!("Carved/{}", cf.kind.as_str()),
                                    kind: cf.kind,
                                    size_bytes: cf.length_bytes,
                                    modified: None,
                                    source: FileSource::Carved {
                                        offset_bytes: cf.offset_bytes,
                                        length_bytes: cf.length_bytes,
                                        signature: cf.signature.to_string(),
                                    },
                                    recoverability: cf.recoverability,
                                    head_hex: String::new(),
                                };
                                self.add_file(record).await?;

                                carved_bytes += cf.length_bytes;
                                if self.files_found % 50 == 0 {
                                    let pct = if total_carve_bytes > 0 {
                                        (carved_bytes * 100 / total_carve_bytes).min(100)
                                    } else { 0 };
                                    let phase = format!("Carving: {}% ({} files)", pct, self.files_found);
                                    self.emit_progress(&phase).await?;
                                }
                            }
                            None => break, // channel closed, carver done
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                        // Check for cancellation periodically even when
                        // no files are arriving (e.g. scanning a zero-filled region)
                        if self.control.should_stop() {
                            cancel.cancel();
                            break;
                        }
                    }
                }
            }

            // Wait for the carver task to finish
            match carve_handle.await {
                Ok(Ok(stats)) => {
                    debug!(
                        candidates = stats.candidates_examined,
                        confirmed = stats.files_confirmed,
                        rejected = stats.rejections,
                        "carve range {start:#x}..{end:#x} done"
                    );
                }
                Ok(Err(tr_core::Error::Cancelled)) => {
                    return Err(tr_core::Error::Cancelled);
                }
                Ok(Err(e)) => {
                    warn!(error = %e, "carve range failed");
                }
                Err(e) => {
                    warn!(error = %e, "carver task panicked");
                }
            }

            // Update sectors scanned
            let range_sectors = (end - start) / u64::from(self.sector_size);
            self.sectors_scanned = (self.sectors_scanned + range_sectors).min(self.sectors_total);
        }

        self.flush_files().await?;
        Ok(())
    }

}

/// Compute byte ranges NOT covered by any partition. These are the "gaps"
/// where deleted / formatted data may be carved.
fn compute_unallocated_ranges(
    partitions: &[tr_core::PartitionInfo],
    sector_size: u32,
    device_bytes: u64,
) -> Vec<(u64, u64)> {
    let ss = u64::from(sector_size);
    let mut allocated: Vec<(u64, u64)> = partitions
        .iter()
        .map(|p| {
            let start = p.start_lba * ss;
            let end = start + p.length_sectors * ss;
            (start, end.min(device_bytes))
        })
        .collect();
    allocated.sort_by_key(|&(s, _)| s);

    let mut ranges = Vec::new();
    let mut cursor = 0u64;
    for (start, end) in &allocated {
        if *start > cursor {
            ranges.push((cursor, *start));
        }
        cursor = cursor.max(*end);
    }
    if cursor < device_bytes {
        ranges.push((cursor, device_bytes));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unallocated_ranges_basic() {
        let parts = vec![
            tr_core::PartitionInfo {
                index: 0,
                scheme: tr_core::PartitionScheme::Gpt,
                type_id: "basic".into(),
                name: None,
                filesystem: Some("ntfs".into()),
                start_lba: 2048,
                length_sectors: 1_000_000,
                sector_size: 512,
                reconstructed: false,
            },
        ];
        let device = 2_000_000 * 512;
        let ranges = compute_unallocated_ranges(&parts, 512, device);
        // Should have gap before partition and gap after
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], (0, 2048 * 512));
        assert_eq!(ranges[1], ((2048 + 1_000_000) * 512, device));
    }

    #[test]
    fn unallocated_ranges_no_partitions() {
        let ranges = compute_unallocated_ranges(&[], 512, 1_000_000);
        assert_eq!(ranges, vec![(0, 1_000_000)]);
    }
}
