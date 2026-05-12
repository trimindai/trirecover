//! TriRecover desktop shell. Wires the recovery engine, carver, and filesystem
//! parsers to the frontend via Tauri invoke handlers. The source drive is never
//! written to.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tr_carver::scanner::CancelToken;
use tr_carver::{Carver, ScanConfig};
use tr_core::{
    CloudDestination, FileKind, JobId, JobState, RecoverDestination, RecoveryStrategy,
    ScanProgress, ScanRequest, SessionId,
};
use tr_recovery_engine::JobManager;
use tr_storage::{enumerate_drives, open_drive, MmapReader, SectorReader, SectorReaderExt};

// ------------------------------------------------------------------ types

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CarvedSummary {
    id: u64,
    kind: String,
    extension: String,
    offset_bytes: u64,
    length_bytes: u64,
    recoverability: u8,
    signature: String,
}

#[derive(Debug, Serialize, Clone)]
struct ScanProgressEvent {
    bytes_scanned: u64,
    bytes_total: u64,
    files_found: u64,
}

#[derive(Debug, Serialize, Clone)]
struct ScanDoneEvent {
    files_found: u64,
    bytes_recoverable: u64,
    elapsed_ms: u64,
}

#[derive(Debug, Serialize, Clone)]
struct DriveEntry {
    path: String,
    model: String,
    serial: String,
    size_bytes: u64,
    sector_size: u32,
    kind: String,
    bus: String,
}

#[derive(Debug, Serialize, Clone)]
struct PartitionEntry {
    index: u32,
    scheme: String,
    type_id: String,
    name: Option<String>,
    filesystem: Option<String>,
    start_lba: u64,
    length_sectors: u64,
    sector_size: u32,
}

#[derive(Debug, Serialize, Clone)]
struct JobInfo {
    job_id: String,
    session_id: String,
    state: String,
}

#[derive(Debug, Serialize, Clone)]
struct CloudDest {
    provider: String,
    label: String,
    local_path: String,
    free_bytes: Option<u64>,
    sync_active: bool,
}

// ------------------------------------------------------------------ helpers

fn parse_kinds(kinds: &[String]) -> Option<Vec<FileKind>> {
    if kinds.is_empty() {
        return None;
    }
    let parsed: Vec<FileKind> = kinds
        .iter()
        .filter_map(|s| match s.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Some(FileKind::Jpg),
            "png" => Some(FileKind::Png),
            "gif" => Some(FileKind::Gif),
            "bmp" => Some(FileKind::Bmp),
            "tiff" | "tif" => Some(FileKind::Tiff),
            "mp4" => Some(FileKind::Mp4),
            "mov" => Some(FileKind::Mov),
            "mkv" => Some(FileKind::Mkv),
            "avi" => Some(FileKind::Avi),
            "pdf" => Some(FileKind::Pdf),
            "docx" => Some(FileKind::Docx),
            "xlsx" => Some(FileKind::Xlsx),
            "pptx" => Some(FileKind::Pptx),
            "zip" => Some(FileKind::Zip),
            "rar" => Some(FileKind::Rar),
            "7z" | "sevenz" => Some(FileKind::SevenZ),
            "psd" => Some(FileKind::Psd),
            "ai" => Some(FileKind::Ai),
            "txt" => Some(FileKind::Txt),
            "csv" => Some(FileKind::Csv),
            "sql" => Some(FileKind::Sql),
            _ => None,
        })
        .collect();
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

fn parse_strategy(s: &str) -> RecoveryStrategy {
    match s.to_ascii_lowercase().as_str() {
        "quick" => RecoveryStrategy::Quick,
        "deep" => RecoveryStrategy::Deep,
        "raw" => RecoveryStrategy::Raw,
        "partition" => RecoveryStrategy::Partition,
        "formatted" => RecoveryStrategy::Formatted,
        "corrupted" | "corrupted-fs" | "corruptedfs" => RecoveryStrategy::CorruptedFs,
        _ => RecoveryStrategy::Deep,
    }
}

fn state_str(s: JobState) -> &'static str {
    match s {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Paused => "paused",
        JobState::Finished => "finished",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    }
}

/// Returns true if the path looks like a physical drive path rather than a file.
fn is_drive_path(path: &str) -> bool {
    path.starts_with(r"\\.\PhysicalDrive")
        || path.starts_with("/dev/sd")
        || path.starts_with("/dev/nvme")
        || path.starts_with("/dev/mmcblk")
        || path.starts_with("/dev/vd")
        || path.starts_with("/dev/hd")
        || path.starts_with("/dev/xvd")
}

/// Open a reader for a drive path or a disk image file.
fn open_source(path: &str) -> Result<Arc<dyn SectorReader>, String> {
    if is_drive_path(path) {
        let handle = open_drive(path).map_err(|e| format!("opening drive: {e}"))?;
        Ok(handle.reader())
    } else {
        let reader = MmapReader::from_file(path).map_err(|e| format!("opening image: {e}"))?;
        Ok(Arc::new(reader))
    }
}

// ================================================================== commands
// === Drive / partition discovery ===

#[tauri::command]
async fn list_drives() -> Result<Vec<DriveEntry>, String> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(|| enumerate_drives()),
    )
    .await;

    let drives = match result {
        Ok(Ok(Ok(d))) => d,
        Ok(Ok(Err(e))) => {
            tracing::warn!("enumerate_drives failed: {e}");
            vec![]
        }
        Ok(Err(e)) => {
            tracing::warn!("enumerate task panicked: {e}");
            vec![]
        }
        Err(_) => {
            tracing::warn!("enumerate_drives timed out after 10s");
            vec![]
        }
    };

    Ok(drives
        .into_iter()
        .map(|d| DriveEntry {
            path: d.info.path,
            model: d.info.model,
            serial: d.info.serial,
            size_bytes: d.info.size_bytes,
            sector_size: d.info.sector_size,
            kind: d.info.kind.as_str().to_string(),
            bus: d.info.bus.as_str().to_string(),
        })
        .collect())
}

#[tauri::command]
async fn list_partitions(drive_path: String) -> Result<Vec<PartitionEntry>, String> {
    let reader = open_source(&drive_path)?;
    let parts = tr_partition::read_table(reader.as_ref())
        .await
        .map_err(|e| format!("read partition table: {e}"))?;
    Ok(parts
        .into_iter()
        .map(|p| PartitionEntry {
            index: p.index,
            scheme: format!("{:?}", p.scheme).to_lowercase(),
            type_id: p.type_id,
            name: p.name,
            filesystem: p.filesystem,
            start_lba: p.start_lba,
            length_sectors: p.length_sectors,
            sector_size: p.sector_size,
        })
        .collect())
}

#[tauri::command]
async fn detect_filesystem(drive_path: String, partition_index: u32) -> Result<String, String> {
    let reader = open_source(&drive_path)?;
    let parts = tr_partition::read_table(reader.as_ref())
        .await
        .map_err(|e| format!("read partition table: {e}"))?;
    let part = parts
        .iter()
        .find(|p| p.index == partition_index)
        .ok_or_else(|| format!("partition {partition_index} not found"))?;
    let fs = tr_filesystem::detect(reader.as_ref(), part)
        .await
        .map_err(|e| format!("detect fs: {e}"))?;
    Ok(fs.name().to_string())
}

// === Cloud destination detection ===

#[tauri::command]
async fn detect_cloud_destinations() -> Result<Vec<CloudDest>, String> {
    let dests = tokio::task::spawn_blocking(tr_core::cloud::detect_all)
        .await
        .map_err(|e| format!("cloud detection: {e}"))?;
    Ok(dests
        .into_iter()
        .map(|d| CloudDest {
            provider: format!("{:?}", d.provider),
            label: d.label,
            local_path: d.local_path.to_string_lossy().to_string(),
            free_bytes: d.free_bytes,
            sync_active: d.sync_active,
        })
        .collect())
}

// === Strategy-based scan (recovery engine) ===

#[tauri::command]
async fn start_scan(
    app: tauri::AppHandle,
    drive_path: String,
    strategy: String,
    partitions: Vec<u32>,
    file_kinds: Vec<String>,
    min_size: u64,
) -> Result<String, String> {
    let mgr = app.state::<JobManager>();
    let request = ScanRequest {
        drive_path,
        strategy: parse_strategy(&strategy),
        partitions,
        file_kinds: parse_kinds(&file_kinds).unwrap_or_default(),
        min_carve_bytes: min_size,
        resume_session: None,
    };
    let job_id = mgr.start_scan(request);
    Ok(job_id.to_string())
}

#[tauri::command]
async fn scan_status(app: tauri::AppHandle, job_id: String) -> Result<JobInfo, String> {
    let mgr = app.state::<JobManager>();
    let id: JobId = parse_job_id(&job_id)?;
    let state = mgr
        .job_state(id)
        .ok_or_else(|| format!("job {job_id} not found"))?;
    Ok(JobInfo {
        job_id: job_id.clone(),
        session_id: String::new(),
        state: state_str(state).to_string(),
    })
}

#[tauri::command]
async fn scan_progress(app: tauri::AppHandle, job_id: String) -> Result<Vec<ScanProgress>, String> {
    let mgr = app.state::<JobManager>();
    let id = parse_job_id(&job_id)?;
    Ok(mgr.drain_progress(id))
}

#[tauri::command]
async fn scan_files(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let mgr = app.state::<JobManager>();
    let id = parse_job_id(&job_id)?;
    let files = mgr.drain_files(id);
    files
        .into_iter()
        .map(|f| serde_json::to_value(f).map_err(|e| format!("serialize: {e}")))
        .collect()
}

#[tauri::command]
async fn pause_scan(app: tauri::AppHandle, job_id: String) -> Result<(), String> {
    let mgr = app.state::<JobManager>();
    let id = parse_job_id(&job_id)?;
    mgr.pause(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn resume_scan(app: tauri::AppHandle, job_id: String) -> Result<(), String> {
    let mgr = app.state::<JobManager>();
    let id = parse_job_id(&job_id)?;
    mgr.resume(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn cancel_scan(app: tauri::AppHandle, job_id: String) -> Result<(), String> {
    let mgr = app.state::<JobManager>();
    let id = parse_job_id(&job_id)?;
    mgr.cancel(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_jobs(app: tauri::AppHandle) -> Result<Vec<JobInfo>, String> {
    let mgr = app.state::<JobManager>();
    Ok(mgr
        .active_jobs()
        .into_iter()
        .map(|(jid, sid, state)| JobInfo {
            job_id: jid.to_string(),
            session_id: sid.to_string(),
            state: state_str(state).to_string(),
        })
        .collect())
}

fn parse_job_id(s: &str) -> Result<JobId, String> {
    let uuid = uuid::Uuid::parse_str(s).map_err(|e| format!("invalid job id: {e}"))?;
    Ok(JobId(uuid))
}

// === Raw carver scan (direct, for disk images) ===

#[tauri::command]
async fn scan_image(
    app: tauri::AppHandle,
    image_path: String,
    kinds: Vec<String>,
    min_size: u64,
) -> Result<Vec<CarvedSummary>, String> {
    let reader = open_source(&image_path)?;
    let total = reader.size_bytes();

    let chunk_size = if total > 1024 * 1024 * 1024 {
        16 * 1024 * 1024
    } else {
        8 * 1024 * 1024
    };

    let cfg = ScanConfig {
        chunk_size,
        min_carve_bytes: min_size,
        kinds: parse_kinds(&kinds),
        ..Default::default()
    };

    let (tx, mut rx) = mpsc::channel(4096);
    let cancel = CancelToken::new();

    let n_workers = if total > 256 * 1024 * 1024 {
        std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(4)
    } else {
        1
    };

    let carver = Arc::new(Carver::new(reader.clone(), cfg));
    let region = total / n_workers as u64;
    let mut scan_handles = Vec::with_capacity(n_workers);
    for i in 0..n_workers {
        let carver = carver.clone();
        let tx = tx.clone();
        let cancel = cancel.clone();
        let start = i as u64 * region;
        let end = if i == n_workers - 1 {
            total
        } else {
            (i as u64 + 1) * region
        };
        scan_handles.push(tokio::spawn(async move {
            carver.scan_range(start, end, tx, cancel).await
        }));
    }
    drop(tx);

    let started = std::time::Instant::now();
    let mut summaries: Vec<CarvedSummary> = Vec::with_capacity(1024);
    let mut id_seq: u64 = 0;
    let mut bytes_recoverable: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    let mut max_offset: u64 = 0;
    while let Some(c) = rx.recv().await {
        id_seq += 1;
        bytes_recoverable += c.length_bytes;
        let file_end = c.offset_bytes + c.length_bytes;
        if file_end > max_offset {
            max_offset = file_end;
        }
        let now = std::time::Instant::now();
        if id_seq % 50 == 0 || now.duration_since(last_emit).as_millis() >= 100 {
            let _ = app.emit(
                "scan/progress",
                ScanProgressEvent {
                    bytes_scanned: max_offset,
                    bytes_total: total,
                    files_found: id_seq,
                },
            );
            last_emit = now;
        }
        summaries.push(CarvedSummary {
            id: id_seq,
            kind: c.kind.as_str().to_string(),
            extension: c.kind.extension().to_string(),
            offset_bytes: c.offset_bytes,
            length_bytes: c.length_bytes,
            recoverability: c.recoverability,
            signature: c.signature.to_string(),
        });
    }

    for h in scan_handles {
        h.await
            .map_err(|e| format!("scan task panicked: {e}"))?
            .map_err(|e| format!("scan failed: {e}"))?;
    }

    let _ = app.emit(
        "scan/done",
        ScanDoneEvent {
            files_found: summaries.len() as u64,
            bytes_recoverable,
            elapsed_ms: started.elapsed().as_millis() as u64,
        },
    );
    Ok(summaries)
}

// === File recovery ===

#[derive(Debug, Deserialize)]
struct RecoverItem {
    offset_bytes: u64,
    length_bytes: u64,
    extension: String,
    id: u64,
}

#[derive(Debug, Serialize)]
struct RecoverResult {
    written: u64,
    failed: u64,
    bytes_written: u64,
    destination: String,
}

async fn recover_one_streaming(
    reader: Arc<dyn SectorReader>,
    out_path: PathBuf,
    offset: u64,
    length: u64,
) -> std::result::Result<u64, String> {
    let mut f = tokio::fs::File::create(&out_path)
        .await
        .map_err(|e| format!("create: {e}"))?;
    const BUF_SIZE: usize = 8 * 1024 * 1024;
    let mut buf = vec![0u8; BUF_SIZE];
    let mut remaining = length;
    let mut pos = offset;
    while remaining > 0 {
        let to_read = (remaining as usize).min(BUF_SIZE);
        let n = reader
            .read_at(pos, &mut buf[..to_read])
            .await
            .map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])
            .await
            .map_err(|e| format!("write: {e}"))?;
        pos += n as u64;
        remaining -= n as u64;
    }
    f.flush().await.map_err(|e| format!("flush: {e}"))?;
    Ok(length - remaining)
}

#[tauri::command]
async fn recover_files(
    image_path: String,
    items: Vec<RecoverItem>,
    destination: String,
) -> Result<RecoverResult, String> {
    let dest = PathBuf::from(&destination);
    std::fs::create_dir_all(&dest).map_err(|e| format!("create dest dir: {e}"))?;
    let reader = open_source(&image_path)?;

    let written = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let bytes_written = Arc::new(AtomicU64::new(0));
    let sem = Arc::new(tokio::sync::Semaphore::new(8));

    let mut handles = Vec::with_capacity(items.len());
    for item in items {
        let reader = reader.clone();
        let dest = dest.clone();
        let sem = sem.clone();
        let written = written.clone();
        let failed = failed.clone();
        let bytes_written = bytes_written.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let name = format!(
                "{:08}_{:016x}.{}",
                item.id, item.offset_bytes, item.extension
            );
            let out_path = dest.join(&name);

            match recover_one_streaming(reader, out_path, item.offset_bytes, item.length_bytes)
                .await
            {
                Ok(n) => {
                    written.fetch_add(1, Ordering::Relaxed);
                    bytes_written.fetch_add(n, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::warn!(target: "trirecover", "recovery failed for {name}: {e}");
                    failed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    Ok(RecoverResult {
        written: written.load(Ordering::Relaxed),
        failed: failed.load(Ordering::Relaxed),
        bytes_written: bytes_written.load(Ordering::Relaxed),
        destination: dest.to_string_lossy().to_string(),
    })
}

// === Meta ===

#[tauri::command]
fn app_version() -> String {
    tr_core::VERSION.to_string()
}

#[tauri::command]
fn strategy_info() -> Vec<serde_json::Value> {
    use tr_recovery_engine::strategies;
    [
        RecoveryStrategy::Quick,
        RecoveryStrategy::Deep,
        RecoveryStrategy::Raw,
        RecoveryStrategy::Partition,
        RecoveryStrategy::Formatted,
        RecoveryStrategy::CorruptedFs,
    ]
    .into_iter()
    .map(|s| {
        serde_json::json!({
            "id": format!("{s:?}").to_lowercase(),
            "description": strategies::description(s),
            "speed": strategies::speed_rating(s),
        })
    })
    .collect()
}

// ================================================================== app

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info,tr_carver=info,trirecover_app_lib=info")
        .with_target(false)
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .manage(JobManager::new())
        .invoke_handler(tauri::generate_handler![
            // Drive / partition
            list_drives,
            list_partitions,
            detect_filesystem,
            // Cloud
            detect_cloud_destinations,
            // Strategy scan (recovery engine)
            start_scan,
            scan_status,
            scan_progress,
            scan_files,
            pause_scan,
            resume_scan,
            cancel_scan,
            list_jobs,
            // Raw carver (direct)
            scan_image,
            // Recovery
            recover_files,
            // Meta
            app_version,
            strategy_info,
        ])
        .setup(|app| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running TriRecover");
}
