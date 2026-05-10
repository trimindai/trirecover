//! TriRecover desktop shell. Wires the carver to the frontend via Tauri
//! invoke handlers. The source drive is never written to.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tr_carver::scanner::CancelToken;
use tr_carver::{Carver, ScanConfig};
use tr_core::FileKind;
use tr_storage::{enumerate_drives, open_drive, FixtureReader, SectorReader, SectorReaderExt};

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

/// Returns true if the path looks like a physical drive path rather than a file.
fn is_drive_path(path: &str) -> bool {
    // Windows: \\.\PhysicalDrive0, \\.\PhysicalDrive1, etc.
    // Linux: /dev/sda, /dev/nvme0n1, etc.
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
        let reader = FixtureReader::from_file(path).map_err(|e| format!("opening image: {e}"))?;
        Ok(Arc::new(reader))
    }
}

// ---------- Tauri commands ----------

#[tauri::command]
async fn list_drives() -> Result<Vec<DriveEntry>, String> {
    // enumerate_drives() does blocking I/O (CreateFileW on \\.\PhysicalDriveN).
    // Run on the blocking thread pool with a timeout so the UI never hangs.
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
async fn scan_image(
    app: tauri::AppHandle,
    image_path: String,
    kinds: Vec<String>,
    min_size: u64,
) -> Result<Vec<CarvedSummary>, String> {
    let reader = open_source(&image_path)?;
    let total = reader.size_bytes();
    let cfg = ScanConfig {
        min_carve_bytes: min_size,
        kinds: parse_kinds(&kinds),
        ..Default::default()
    };
    let carver = Carver::new(reader.clone(), cfg);
    let (tx, mut rx) = mpsc::channel(256);
    let cancel = CancelToken::new();
    let scan = tokio::spawn(async move {
        carver.scan_range(0, total, tx, cancel).await
    });

    let started = std::time::Instant::now();
    let mut summaries: Vec<CarvedSummary> = Vec::with_capacity(1024);
    let mut id_seq: u64 = 0;
    let mut bytes_recoverable: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    while let Some(c) = rx.recv().await {
        id_seq += 1;
        bytes_recoverable += c.length_bytes;
        // Batch UI events: emit at most every 100ms or every 50 files
        let now = std::time::Instant::now();
        if id_seq % 50 == 0 || now.duration_since(last_emit).as_millis() >= 100 {
            let _ = app.emit(
                "scan/progress",
                ScanProgressEvent {
                    bytes_scanned: c.offset_bytes + c.length_bytes,
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
            signature: c.signature,
        });
    }
    let _ = scan
        .await
        .map_err(|e| format!("scan task panicked: {e}"))?
        .map_err(|e| format!("scan failed: {e}"))?;
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

#[tauri::command]
async fn recover_files(
    image_path: String,
    items: Vec<RecoverItem>,
    destination: String,
) -> Result<RecoverResult, String> {
    let dest = PathBuf::from(&destination);
    std::fs::create_dir_all(&dest).map_err(|e| format!("create dest dir: {e}"))?;
    let reader = open_source(&image_path)?;

    let mut written = 0u64;
    let mut failed = 0u64;
    let mut bytes_written = 0u64;
    for item in items {
        let name = format!(
            "{:08}_{:016x}.{}",
            item.id, item.offset_bytes, item.extension
        );
        let out_path = dest.join(&name);
        let bytes = match reader.read_vec(item.offset_bytes, item.length_bytes as usize).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(target: "trirecover", "read failed for {name}: {e}");
                failed += 1;
                continue;
            }
        };
        match tokio::fs::File::create(&out_path).await {
            Ok(mut f) => {
                if let Err(e) = f.write_all(&bytes).await {
                    tracing::warn!(target: "trirecover", "write failed for {name}: {e}");
                    failed += 1;
                    continue;
                }
                let _ = f.flush().await;
                bytes_written += bytes.len() as u64;
                written += 1;
            }
            Err(e) => {
                tracing::warn!(target: "trirecover", "create failed for {name}: {e}");
                failed += 1;
            }
        }
    }

    Ok(RecoverResult {
        written,
        failed,
        bytes_written,
        destination: dest.to_string_lossy().to_string(),
    })
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

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
        .invoke_handler(tauri::generate_handler![
            list_drives,
            scan_image,
            recover_files,
            app_version
        ])
        .setup(|app| {
            // Bring the main window to front on launch.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running TriRecover");
}
