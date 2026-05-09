//! NTFS read-only parser.
//!
//! Layout (as we read it):
//! - `boot.rs`      : NTFS boot sector → cluster size, MFT location.
//! - `mft.rs`       : FILE record header, fixup application, attribute walking.
//! - `attribute.rs` : $STANDARD_INFORMATION (0x10), $FILE_NAME (0x30), $DATA (0x80).
//! - `runlist.rs`   : non-resident DATA decoded into `Vec<DataRun>`.
//!
//! `NtfsVolume::iter_records` yields every MFT record on the volume, including
//! freed records (the recoverable ones). The recovery engine consumes this
//! stream and turns each record into a `FileRecord`.

pub mod attribute;
pub mod boot;
pub mod mft;
pub mod path;
pub mod runlist;

use crate::ntfs::boot::NtfsBoot;
use crate::ntfs::mft::{MftRecord, MFT_RECORD_SIZE_DEFAULT};
use crate::ntfs::path::PathResolver;
use std::collections::HashSet;
use std::sync::Arc;
use tr_core::{DataRun, FileKind, FileRecord, FileSource, PartitionInfo, Result};
use tr_storage::{SectorReader, SectorReaderExt};

/// Maximum extension MFT records we'll follow per base record. Real Windows
/// files virtually never need more than a handful; an unbounded value risks
/// runaway behavior on a corrupt $ATTRIBUTE_LIST.
const MAX_EXTENSION_RECORDS: usize = 64;

#[derive(Debug)]
pub struct NtfsVolume {
    pub boot: NtfsBoot,
    pub partition: PartitionInfo,
    reader: Arc<dyn SectorReader>,
    /// Cached data runs of the $MFT itself, decoded from the boot record.
    /// Used to translate VCN → LCN within the MFT.
    mft_runs: Vec<DataRun>,
    /// Resolved MFT record size in bytes.
    pub mft_record_size: u32,
}

impl NtfsVolume {
    pub async fn open(
        reader: Arc<dyn SectorReader>,
        partition: PartitionInfo,
    ) -> Result<Self> {
        let bs = reader.read_lba(partition.start_lba).await?;
        let boot = NtfsBoot::parse(&bs, partition.start_lba)?;
        let mft_record_size = boot.mft_record_bytes();

        // Read the MFT's own first record by computing its byte offset on the
        // disk and decoding the $DATA attribute's run list.
        let mft_byte_offset = partition.start_lba * u64::from(boot.bytes_per_sector)
            + boot.mft_lcn * u64::from(boot.cluster_bytes());
        let buf_len = std::cmp::max(mft_record_size as usize, 1024);
        let mut buf = vec![0u8; buf_len];
        reader.read_at(mft_byte_offset, &mut buf).await?;

        let rec = MftRecord::parse(&buf, boot.bytes_per_sector, mft_record_size)?;
        let mft_runs = rec
            .data_runs(
                boot.cluster_bytes(),
                boot.bytes_per_sector,
                partition.start_lba,
            )
            .unwrap_or_default();

        Ok(Self {
            boot,
            partition,
            reader,
            mft_runs,
            mft_record_size,
        })
    }

    /// Total number of records the MFT can hold given its current size on disk.
    #[must_use]
    pub fn approximate_record_count(&self) -> u64 {
        let total: u64 = self
            .mft_runs
            .iter()
            .map(|r| r.length_sectors * u64::from(self.boot.bytes_per_sector))
            .sum();
        total / u64::from(self.mft_record_size)
    }

    /// Read MFT record at the given record index.
    pub async fn read_record(&self, index: u64) -> Result<MftRecord> {
        let byte = self.mft_byte_offset_of(index)?;
        let mut buf = vec![0u8; self.mft_record_size as usize];
        self.reader.read_at(byte, &mut buf).await?;
        MftRecord::parse(&buf, self.boot.bytes_per_sector, self.mft_record_size)
    }

    /// If `base` carries an `$ATTRIBUTE_LIST`, walk it, fetch every unique
    /// extension MFT record it references, and append their attributes onto
    /// `base`. Returns the (possibly merged) record. A no-op when there is no
    /// attribute list. Errors reading individual extension records are
    /// logged and skipped — recovery should make the best of partial data.
    ///
    /// Also returns the list of extension record indices visited so the
    /// caller can mark them as already-consumed and skip them in the
    /// top-level scan.
    pub async fn expand_record(&self, mut base: MftRecord) -> Result<(MftRecord, Vec<u64>)> {
        if !base.is_initialised() {
            return Ok((base, Vec::new()));
        }
        let Some(entries) = base.attribute_list_entries() else {
            return Ok((base, Vec::new()));
        };
        let base_index = base.record_index();
        let mut extensions: Vec<u64> = Vec::new();
        for e in &entries {
            if Some(e.mft_record) == base_index {
                continue;
            }
            if !extensions.contains(&e.mft_record) {
                extensions.push(e.mft_record);
                if extensions.len() >= MAX_EXTENSION_RECORDS {
                    tracing::warn!(
                        record = base_index,
                        cap = MAX_EXTENSION_RECORDS,
                        "expand_record: $ATTRIBUTE_LIST exceeds extension cap; truncating"
                    );
                    break;
                }
            }
        }
        for ext_idx in &extensions {
            match self.read_record(*ext_idx).await {
                Ok(ext) => {
                    if !ext.is_initialised() {
                        tracing::trace!(
                            base = base_index,
                            ext = *ext_idx,
                            "expand_record: extension record uninitialised"
                        );
                        continue;
                    }
                    for a in ext.take_attributes() {
                        base.append_attribute(a);
                    }
                }
                Err(e) => {
                    tracing::trace!(
                        base = base_index,
                        ext = *ext_idx,
                        "expand_record: skip unreadable extension: {e}"
                    );
                }
            }
        }
        Ok((base, extensions))
    }

    fn mft_byte_offset_of(&self, index: u64) -> Result<u64> {
        let bytes_per_record = u64::from(self.mft_record_size);
        let target_record_byte = index * bytes_per_record;
        // Walk the run list to find which run contains target_record_byte.
        let bps = u64::from(self.boot.bytes_per_sector);
        let mut consumed: u64 = 0;
        for run in &self.mft_runs {
            let run_bytes = run.length_sectors * bps;
            if target_record_byte < consumed + run_bytes {
                let inside = target_record_byte - consumed;
                let abs = run.start_lba * bps + inside;
                return Ok(abs);
            }
            consumed += run_bytes;
        }
        Err(tr_core::Error::corrupt(
            "mft",
            target_record_byte,
            "record index past MFT end",
        ))
    }

    /// Iterate all MFT records and convert them into `FileRecord`s. Both
    /// allocated and freed records are emitted; the consumer filters on
    /// `is_deleted` if needed.
    ///
    /// Done as a single MFT pass: every initialised record (file *or*
    /// directory) registers its best $FILE_NAME with the [`PathResolver`], so
    /// that when we resolve file paths in a second in-memory pass every parent
    /// directory is already known. This keeps disk I/O to one read per
    /// record while still producing fully reconstructed paths.
    pub async fn collect_files(
        &self,
        partition_index: u32,
        max_records: u64,
    ) -> Result<Vec<FileRecord>> {
        let count = std::cmp::min(self.approximate_record_count(), max_records);

        // Pending file rows (record_id stored separately so we can resolve
        // its path against the resolver after the scan completes).
        let mut pending: Vec<(u64, FileRecord)> = Vec::new();
        let mut resolver = PathResolver::new();
        // Records already absorbed into a base via $ATTRIBUTE_LIST expansion;
        // skip when the outer loop reaches them so we don't re-read or
        // double-count their attributes.
        let mut consumed_extensions: HashSet<u64> = HashSet::new();

        for i in 0..count {
            if consumed_extensions.contains(&i) {
                continue;
            }
            let raw = match self.read_record(i).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::trace!(record = i, "skip unreadable MFT record: {e}");
                    continue;
                }
            };
            if !raw.is_initialised() {
                continue;
            }
            // Extension records have no $FILE_NAME and exist only to carry
            // overflow attributes for their base. Skip them here; their
            // contents are picked up via expand_record from the base side.
            if raw.is_extension_record() {
                continue;
            }

            // Follow $ATTRIBUTE_LIST (if any) so $DATA chunks scattered across
            // extension MFT records get folded back into this base record.
            let (rec, exts) = match self.expand_record(raw).await {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::trace!(record = i, "expand_record failed: {e}");
                    continue;
                }
            };
            for e in exts {
                consumed_extensions.insert(e);
            }

            let record_id = rec.record_index().unwrap_or(i);

            // Register *every* named record (files AND directories) so that
            // child files can later walk up through them.
            if let Some(fname) = rec.best_filename() {
                resolver.register(record_id, &fname);
            }

            if rec.is_directory() {
                continue;
            }
            let Some(name) = rec.best_name() else {
                continue;
            };
            let size = rec.real_size();
            let runs = rec
                .data_runs(
                    self.boot.cluster_bytes(),
                    self.boot.bytes_per_sector,
                    self.partition.start_lba,
                )
                .unwrap_or_default();
            let head_hex = rec.head_hex(16);
            let kind = guess_kind(&name);
            pending.push((
                record_id,
                FileRecord {
                    id: record_id,
                    name: name.clone(),
                    // Placeholder — filled in by the resolve pass below.
                    path: name,
                    kind,
                    size_bytes: size,
                    modified: rec.modified_time(),
                    source: FileSource::Filesystem {
                        partition_index,
                        record_id,
                        is_deleted: !rec.is_in_use(),
                        is_resident: rec.is_data_resident(),
                        runs,
                    },
                    recoverability: rec.recoverability_score(),
                    head_hex,
                },
            ));
        }

        // Second pass: resolve full paths now that the resolver has every
        // directory registered.
        let mut out = Vec::with_capacity(pending.len());
        for (record_id, mut fr) in pending {
            fr.path = resolver.resolve(record_id);
            out.push(fr);
        }
        Ok(out)
    }
}

/// Guess `FileKind` from extension. The recovery engine refines this with
/// signature sniffing once any data is read.
#[must_use]
pub fn guess_kind(name: &str) -> FileKind {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "jpg" | "jpeg" => FileKind::Jpg,
        "png" => FileKind::Png,
        "gif" => FileKind::Gif,
        "bmp" => FileKind::Bmp,
        "tif" | "tiff" => FileKind::Tiff,
        "mp4" | "m4v" => FileKind::Mp4,
        "mov" => FileKind::Mov,
        "mkv" => FileKind::Mkv,
        "avi" => FileKind::Avi,
        "pdf" => FileKind::Pdf,
        "docx" => FileKind::Docx,
        "xlsx" => FileKind::Xlsx,
        "pptx" => FileKind::Pptx,
        "zip" => FileKind::Zip,
        "rar" => FileKind::Rar,
        "7z" => FileKind::SevenZ,
        "psd" => FileKind::Psd,
        "ai" => FileKind::Ai,
        "txt" | "log" | "md" => FileKind::Txt,
        "csv" => FileKind::Csv,
        "sql" => FileKind::Sql,
        _ => FileKind::Other,
    }
}

#[allow(dead_code)]
const _MFT_DEFAULT: u32 = MFT_RECORD_SIZE_DEFAULT;
