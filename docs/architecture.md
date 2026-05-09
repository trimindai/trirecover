# TriRecover — Architecture

> **Status:** v0.1 foundation. This document is the source of truth for module boundaries, data flow, and invariants. Every other document defers to this one.

## 1. Goals & non-goals

### Goals
- **Read-only forensics**: never write to a source drive. Period.
- **Crash-safe**: a crash mid-scan must lose at most the last few seconds of progress; the session resumes.
- **Memory-safe**: Rust + zero `unsafe` outside the FFI shim files in `tr-storage`.
- **Predictable performance**: linear sector I/O, bounded RAM regardless of drive size, work-stealing scan pipeline.
- **Distribution-ready packaging**: signed Windows installer, auto-update channel, telemetry toggle.

### Non-goals (v0.1)
- Writing recovered data back to the source drive.
- ReFS, encrypted volumes (BitLocker/VeraCrypt), Linux/macOS filesystems.
- Network/SAN/iSCSI imaging.
- RAID reconstruction.

## 2. High-level layers

```
+-----------------------------------------------------------+
|                   React + Tailwind UI                     |  frontend/
|   Dashboard | Wizard | Results | Preview | Settings       |
+-----------------------------▲-----------------------------+
                              |  Tauri IPC (typed commands)
+-----------------------------▼-----------------------------+
|                       tauri shell                         |  src-tauri/
|   commands.rs  |  state.rs  |  events.rs  |  updater     |
+-----------------------------▲-----------------------------+
                              |  in-process API
+-----------------------------▼-----------------------------+
|                    recovery-engine                        |  crates/recovery-engine
|   jobs | strategies | sessions | probability | integrity |
+--------▲--------▲--------▲--------▲----------------------+
         |        |        |        |
+--------▼--+ +---▼----+ +-▼------+ +▼---------------------+
| filesys.  | | carver | | parti- | |       core           |
| ntfs/fat/ | |  sigs+ | | tion   | |  errors / db /       |
| exfat     | |  vald. | | mbr+gpt| |  config / logging    |
+-----------+ +--------+ +--------+ +----------------------+
                              ▲
+-----------------------------|----------------------------+
|                    storage crate                          |  crates/storage
|   SectorReader trait | windows IOCTL | linux /dev/sdX    |
|   drive enumeration  | SMART query   | bad-sector retry  |
+-----------------------------------------------------------+
```

Crates depend upward only. `tr-core` has zero dependencies on other workspace crates.

## 3. Crate map

| Crate                 | Purpose                                                                 |
| --------------------- | ----------------------------------------------------------------------- |
| `tr-core`             | Errors, config, logging init, SQLite session DB, shared domain types.   |
| `tr-storage`          | Read-only raw sector I/O. Windows `IOCTL_*` + Linux `/dev/sdX` fallback.|
| `tr-partition`        | MBR + GPT parsers. Pure functions over byte slices.                     |
| `tr-filesystem`       | NTFS / FAT32 / exFAT parsers. Pull-based readers over `SectorReader`.   |
| `tr-carver`           | Signature DB + per-format validators for unallocated-space carving.     |
| `tr-recovery-engine`  | Scan strategies, async job manager, pause/resume, session persistence.  |
| `src-tauri`           | Tauri 2 shell, typed IPC, app state, updater, system tray.              |

## 4. Data flow — end-to-end scan

```
User clicks "Scan"
  └─► Tauri cmd: scan_start(drive_id, strategy, options)
        └─► engine::JobManager::spawn(JobSpec)
              ├─► storage::open_drive(handle)        // FILE_SHARE_READ + GENERIC_READ only
              ├─► partition::read_table(reader)
              ├─► filesystem::open(partition, reader)
              │      └─► emits FileRecord stream
              ├─► (deep) carver::scan_unallocated(reader, sig_db)
              │      └─► emits CarvedFile stream
              └─► merge → dedup → sqlite session.files
                  ├─► event "scan/progress" every 250 ms
                  └─► event "scan/file_found" batched 64 at a time
User clicks "Recover" on N files
  └─► Tauri cmd: recover_files(session_id, ids[], dest)
        └─► engine::recover(...)  // ALWAYS writes to a different volume
              ├─► assert dest_volume != source_volume   (hard error if equal)
              ├─► reconstruct stream from data runs / carved offsets
              ├─► validate per-format integrity
              └─► write to dest with atomic rename
```

## 5. Read-only invariant

The single most important invariant in this project. Enforced at three layers:

1. **OS handle**: every drive handle is opened with `GENERIC_READ` only and `FILE_SHARE_READ | FILE_SHARE_WRITE`. The Windows API will reject any write attempt with `ACCESS_DENIED` regardless of code-path bugs.
2. **Type system**: the `SectorReader` trait has only `read_at(&self, offset, buf)`. There is no `write_at`. There is no way to obtain a writable handle from a `SectorReader`.
3. **Recovery destination guard**: before any byte is written during recovery, `engine::recover` calls `same_volume(src_drive, dest_path)`. If true, it returns `Error::SameVolumeRecoveryRefused` and writes nothing.

`unsafe` is allowed only in `crates/storage/src/windows.rs` and `crates/storage/src/linux.rs` (FFI shims). Every other crate is `#![forbid(unsafe_code)]`.

## 6. Concurrency model

- Tauri main thread: UI events only.
- A single tokio multi-thread runtime owns all scan jobs.
- Each `Job` runs on a tokio task. CPU-heavy work (carving, hashing) is dispatched to a `rayon` pool via `spawn_blocking`-bridged channels.
- Progress events go through a bounded `tokio::sync::mpsc<Event>(1024)`. Backpressure to the producer if the UI is slow — events are coalesced, never dropped silently.
- Pause: cooperative. Each scan loop checks an `AtomicU8` state every N sectors. Resume picks up at the last persisted offset.

## 7. Persistence

SQLite via `sqlx`. One file per scan session under `%APPDATA%/TriRecover/sessions/<uuid>.db`. WAL mode on. Every 5 s the session checkpoints `progress` and the last 100 newly-found files. On crash, replay starts from `progress.last_offset`.

Schema: see `crates/core/migrations/0001_init.sql`.

## 8. IPC contract

All Rust→TS and TS→Rust types live in `crates/core/src/types.rs` and are mirrored in `frontend/src/lib/types.ts`. Mismatches are caught at build time by a lightweight binding-check script (`scripts/check-bindings.sh`).

Tauri commands (`src-tauri/src/commands.rs`):
- `list_drives() -> Vec<DriveInfo>`
- `drive_smart(drive_id) -> SmartReport`
- `scan_start(req: ScanRequest) -> JobId`
- `scan_pause(JobId) -> ()`
- `scan_resume(JobId) -> ()`
- `scan_cancel(JobId) -> ()`
- `session_load(session_id) -> SessionSummary`
- `session_query(session_id, filter) -> Page<FileRecord>`
- `preview_open(session_id, file_id) -> PreviewBlob`
- `recover_files(req: RecoverRequest) -> RecoverReport`
- `settings_get() / settings_set(...)`

Events emitted to the frontend:
- `scan/progress` — `{ job_id, sectors_scanned, sectors_total, files_found, eta_secs }`
- `scan/file_found` — `Vec<FileRecord>` (batched)
- `scan/state` — `{ job_id, state }` (running, paused, finished, error)
- `drive/health` — `{ drive_id, smart_summary }`

## 9. Threat model (summary — see `threat-model.md`)

| Threat                                       | Mitigation                                              |
| -------------------------------------------- | ------------------------------------------------------- |
| Buggy parser corrupts the source drive       | Read-only handles + type-system enforcement (§5).       |
| Malicious crafted MFT triggers RCE           | Bounded reads + `zerocopy` + no `unsafe` in parsers.    |
| Disk full → recovery writes overwrite source | `same_volume` guard refuses recovery to source.         |
| Privilege misuse on Windows                  | Manifest requires `requireAdministrator`; UAC prompt.   |
| Telemetry leaks PII                          | Off by default. When on, only build/version/error hash. |

## 10. Build & ship

| Target            | Toolchain                        | Output                            |
| ----------------- | -------------------------------- | --------------------------------- |
| Windows installer | Rust 1.80 + Inno Setup 6 + WiX   | `TriRecover-Setup-x.y.z.exe`      |
| Windows portable  | Rust + 7z                        | `TriRecover-Portable-x.y.z.zip`   |
| Linux dev build   | Rust + GTK (Tauri prereqs)       | `trirecover` binary (dev only)    |

CI: GitHub Actions, see `.github/workflows/`.

## 11. Versioning & licensing

- SemVer 2.0.0.
- Dual-licensed: GPLv3 (community) + commercial license for closed-source redistribution. See `LICENSE-COMMUNITY.md` and `LICENSE-COMMERCIAL.md`.

## 12. Roadmap (post v0.1)

- exFAT full implementation (currently scaffolded).
- ReFS read support.
- BitLocker volume unlocking with user-supplied recovery key.
- RAID-0/1/5 reconstruction wizard.
- Bad-block aware imaging (clone-and-skip with libddrescue-style retries).
- macOS HFS+ / APFS.
- Linux ext4 / btrfs / xfs.
- Cloud-licensed activation server + offline floating licenses.
- Crash dump collector (opt-in).
