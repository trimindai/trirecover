//! Linux raw-disk I/O — used during development on the Linux VPS.
//!
//! Production target is Windows; this module exists so `cargo test` and
//! `cargo tauri dev` work on the dev host. It opens `/dev/sd*`, `/dev/nvme*n*`,
//! and `/dev/mmcblk*` with `O_RDONLY`.

#![cfg(unix)]
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use crate::sector::SectorReader;
use crate::smart::SmartProvider;
use crate::{Drive, DriveHandle};
use async_trait::async_trait;
use chrono::Utc;
use std::ffi::CString;
use std::fs;
use std::os::fd::RawFd;
use std::sync::Arc;
use tr_core::{
    DriveBus, DriveInfo, DriveKind, Error, Result, SmartHealth, SmartReport,
};

pub fn enumerate_drives() -> Result<Vec<Drive>> {
    let mut out = Vec::new();
    let entries = match fs::read_dir("/sys/block") {
        Ok(e) => e,
        Err(_) => return Ok(out), // sysfs may not exist (containers)
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // Filter to whole disks: sd[a-z], nvme*n*, mmcblk*, vd*, xvd*
        if !is_whole_disk(&name) {
            continue;
        }
        let dev = format!("/dev/{name}");
        if let Ok(info) = info_from_sysfs(&name, &dev) {
            out.push(Drive::new(info));
        }
    }
    Ok(out)
}

fn is_whole_disk(name: &str) -> bool {
    let starts_with = ["sd", "nvme", "mmcblk", "vd", "xvd", "hd"];
    if !starts_with.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    // exclude partitions: sdaN, nvme0n1pN, mmcblk0pN
    if name.starts_with("sd") || name.starts_with("hd") || name.starts_with("vd") || name.starts_with("xvd") {
        return name.chars().last().is_some_and(|c| !c.is_ascii_digit());
    }
    if name.starts_with("nvme") {
        return !name.contains('p');
    }
    if name.starts_with("mmcblk") {
        return !name.contains('p');
    }
    true
}

fn info_from_sysfs(name: &str, dev_path: &str) -> Result<DriveInfo> {
    let base = format!("/sys/block/{name}");
    let read = |sub: &str| fs::read_to_string(format!("{base}/{sub}")).ok();
    let read_dev = |sub: &str| fs::read_to_string(format!("{base}/device/{sub}")).ok();

    let size_sectors: u64 = read("size")
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let logical: u32 = read("queue/logical_block_size")
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(512);
    let model = read_dev("model").map(|s| s.trim().to_string()).unwrap_or_default();
    let vendor = read_dev("vendor").map(|s| s.trim().to_string()).unwrap_or_default();
    let serial = read_dev("serial").map(|s| s.trim().to_string()).unwrap_or_default();

    let model_full = if vendor.is_empty() {
        model.clone()
    } else {
        format!("{vendor} {model}")
    };

    let (kind, bus) = classify(name, &model_full);
    let size_bytes = size_sectors.saturating_mul(512); // sysfs always reports 512-byte blocks

    Ok(DriveInfo {
        path: dev_path.to_string(),
        model: model_full,
        serial,
        size_bytes,
        sector_size: logical,
        kind,
        bus,
        smart_available: matches!(bus, DriveBus::Sata | DriveBus::Nvme | DriveBus::Scsi),
        has_mounted_volumes: false,
    })
}

fn classify(name: &str, model: &str) -> (DriveKind, DriveBus) {
    let m = model.to_uppercase();
    if name.starts_with("nvme") {
        return (DriveKind::Nvme, DriveBus::Nvme);
    }
    if name.starts_with("mmcblk") {
        return (DriveKind::SdCard, DriveBus::Sd);
    }
    if name.starts_with("vd") || name.starts_with("xvd") {
        return (DriveKind::Virtual, DriveBus::Virtual);
    }
    let kind = if m.contains("SSD") || m.contains("SOLID") {
        DriveKind::Ssd
    } else {
        DriveKind::Hdd
    };
    (kind, DriveBus::Sata)
}

pub fn open_drive(path: &str) -> Result<DriveHandle> {
    let cs = CString::new(path).map_err(|_| Error::DeviceNotFound(path.into()))?;
    // SAFETY: `cs` is a valid C string; O_RDONLY guarantees no write capability.
    let fd = unsafe { libc::open(cs.as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        let raw = err.raw_os_error().unwrap_or(0);
        if raw == libc::EACCES || raw == libc::EPERM {
            return Err(Error::PermissionDenied);
        }
        if raw == libc::ENOENT {
            return Err(Error::DeviceNotFound(path.into()));
        }
        return Err(Error::os(raw, format!("open({path}): {err}")));
    }

    let (size_bytes, sector_size) = blkdev_size(fd);
    // Build a synthetic DriveInfo if sysfs lookup fails.
    let info = if let Some(name) = path.strip_prefix("/dev/") {
        info_from_sysfs(name, path).unwrap_or(DriveInfo {
            path: path.to_string(),
            model: String::new(),
            serial: String::new(),
            size_bytes,
            sector_size,
            kind: DriveKind::Unknown,
            bus: DriveBus::Unknown,
            smart_available: false,
            has_mounted_volumes: false,
        })
    } else {
        DriveInfo {
            path: path.to_string(),
            model: String::new(),
            serial: String::new(),
            size_bytes,
            sector_size,
            kind: DriveKind::Unknown,
            bus: DriveBus::Unknown,
            smart_available: false,
            has_mounted_volumes: false,
        }
    };

    let reader = Arc::new(LinuxRawReader::new(path, fd, sector_size, size_bytes));
    Ok(DriveHandle::new(info, reader))
}

// BLKGETSIZE64 isn't re-exported by every libc release, so define it inline.
// _IOR(0x12, 114, sizeof(u64)) under the standard Linux ioctl encoding
// = (_IOC_READ << 30) | (8 << 16) | (0x12 << 8) | 114 = 0x80081272.
// This value is identical across the architectures we support (x86_64,
// aarch64, i686).
const BLKGETSIZE64: libc::c_ulong = 0x8008_1272;

fn blkdev_size(fd: RawFd) -> (u64, u32) {
    let mut size: u64 = 0;
    // SAFETY: ioctl read into a u64 we own.
    let r = unsafe { libc::ioctl(fd, BLKGETSIZE64 as _, &mut size) };
    if r < 0 {
        size = 0;
    }
    let mut block: i32 = 512;
    // BLKSSZGET — logical sector size
    // SAFETY: ioctl read into an i32 we own.
    unsafe {
        libc::ioctl(fd, libc::BLKSSZGET as _, &mut block);
    }
    (size, u32::try_from(block).unwrap_or(512))
}

#[derive(Debug)]
struct OwnedFd(RawFd);

impl Drop for OwnedFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            // SAFETY: we exclusively own the fd.
            unsafe {
                libc::close(self.0);
            }
        }
    }
}

unsafe impl Send for OwnedFd {}
unsafe impl Sync for OwnedFd {}

#[derive(Debug)]
pub struct LinuxRawReader {
    label: String,
    fd: OwnedFd,
    sector_size: u32,
    size_bytes: u64,
}

impl LinuxRawReader {
    fn new(label: &str, fd: RawFd, sector_size: u32, size_bytes: u64) -> Self {
        Self {
            label: label.to_string(),
            fd: OwnedFd(fd),
            sector_size,
            size_bytes,
        }
    }
}

#[async_trait]
impl SectorReader for LinuxRawReader {
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        // SAFETY: pread is thread-safe and does not use the file position
        // pointer, so concurrent calls on the same fd are safe without a lock.
        let n = unsafe {
            libc::pread(
                self.fd.0,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                offset as libc::off_t,
            )
        };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            let raw = err.raw_os_error().unwrap_or(0);
            if raw == libc::EIO {
                // bad sector — surface as a typed error so the engine can retry
                return Err(Error::corrupt("sector", offset, "EIO"));
            }
            return Err(Error::os(raw, format!("pread: {err}")));
        }
        Ok(n as usize)
    }

    fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn label(&self) -> &str {
        &self.label
    }
}

#[derive(Debug, Default)]
pub struct LinuxSmart;

#[async_trait]
impl SmartProvider for LinuxSmart {
    async fn query(&self, drive_path: &str) -> Result<SmartReport> {
        // Linux SMART access goes through the SG_IO ioctl or NVMe admin commands.
        // For dev-only purposes we return an Unknown report.
        Ok(SmartReport {
            drive_path: drive_path.to_string(),
            overall: SmartHealth::Unknown,
            temperature_c: None,
            power_on_hours: None,
            reallocated_sectors: None,
            pending_sectors: None,
            wear_leveling_remaining: None,
            raw_attributes: vec![],
            captured_at: Utc::now(),
        })
    }
}
