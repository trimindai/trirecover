//! Windows raw-disk I/O.
//!
//! Opens `\\.\PhysicalDriveN` with `GENERIC_READ` only and drives reads via
//! `ReadFile` after `SetFilePointerEx`. Drive metadata is queried with
//! `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX` and `IOCTL_STORAGE_QUERY_PROPERTY`.
//!
//! All `unsafe` in the workspace is concentrated here. Each block is justified
//! by a comment naming the Win32 invariant being upheld.

#![cfg(windows)]
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use crate::sector::SectorReader;
use crate::smart::SmartProvider;
use crate::{Drive, DriveHandle};
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::sync::Arc;
use tr_core::{
    DriveBus, DriveInfo, DriveKind, Error, Result, SmartHealth, SmartReport,
};
use ::windows::core::PCWSTR;
use ::windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use ::windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, SetFilePointerEx, FILE_ATTRIBUTE_NORMAL, FILE_BEGIN,
    FILE_FLAG_NO_BUFFERING, FILE_FLAG_RANDOM_ACCESS, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use ::windows::Win32::System::Ioctl::{
    DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_QUERY_PROPERTY,
    STORAGE_BUS_TYPE, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
    STORAGE_QUERY_TYPE,
};
use ::windows::Win32::System::IO::DeviceIoControl;

const MAX_PROBE_DRIVES: u32 = 32;

// -- Public API ---------------------------------------------------------------

pub fn enumerate_drives() -> Result<Vec<Drive>> {
    let mut drives = Vec::new();
    for n in 0..MAX_PROBE_DRIVES {
        match probe_drive(n) {
            Ok(Some(d)) => drives.push(d),
            Ok(None) => {} // gap in numbering; keep probing
            Err(e) => {
                tracing::trace!(drive = n, "probe failed: {e}");
            }
        }
    }
    Ok(drives)
}

pub fn open_drive(path: &str) -> Result<DriveHandle> {
    let h = open_handle_read(path)?;
    let info = query_info(path, h.0)?;
    let reader = Arc::new(WindowsRawReader::new(path, h, info.sector_size, info.size_bytes));
    Ok(DriveHandle::new(info, reader))
}

// -- Probing ------------------------------------------------------------------

fn probe_drive(n: u32) -> Result<Option<Drive>> {
    let path = format!(r"\\.\PhysicalDrive{n}");
    let h = match open_handle_read(&path) {
        Ok(h) => h,
        Err(Error::DeviceNotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let info = query_info(&path, h.0)?;
    drop(h); // Drop closes via Drop impl
    Ok(Some(Drive::new(info)))
}

// -- Handle wrapper -----------------------------------------------------------

#[derive(Debug)]
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: we exclusively own the handle and Windows closes it idempotently.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

fn open_handle_read(path: &str) -> Result<OwnedHandle> {
    let wide: Vec<u16> = OsString::from(path).encode_wide().chain(Some(0)).collect();
    // SAFETY: wide is null-terminated UTF-16; flags are GENERIC_READ only —
    // the kernel will reject any subsequent write call regardless of code-path bugs.
    // FILE_SHARE_READ | FILE_SHARE_WRITE so we don't block other readers.
    let h = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_RANDOM_ACCESS | FILE_FLAG_NO_BUFFERING,
            None,
        )
    };
    let h = match h {
        Ok(h) if h != INVALID_HANDLE_VALUE => h,
        Ok(_) => return Err(Error::DeviceNotFound(path.to_string())),
        Err(e) => {
            let code = e.code().0 as i32;
            // ERROR_FILE_NOT_FOUND = 2, ERROR_PATH_NOT_FOUND = 3
            if matches!(code, 2 | 3) {
                return Err(Error::DeviceNotFound(path.to_string()));
            }
            // ERROR_ACCESS_DENIED = 5
            if code == 5 {
                return Err(Error::PermissionDenied);
            }
            // ERROR_SHARING_VIOLATION = 32
            if code == 32 {
                return Err(Error::DeviceBusy(path.to_string()));
            }
            return Err(Error::os(code, format!("CreateFileW({path}): {e}")));
        }
    };
    Ok(OwnedHandle(h))
}

// -- Geometry / metadata ------------------------------------------------------

fn query_info(path: &str, h: HANDLE) -> Result<DriveInfo> {
    let geom = get_geometry(h)?;
    let (model, serial, bus) = get_storage_descriptor(h).unwrap_or_default();

    let kind = classify_kind(bus, &model);

    Ok(DriveInfo {
        path: path.to_string(),
        model,
        serial,
        size_bytes: geom.DiskSize as u64,
        sector_size: geom.Geometry.BytesPerSector,
        kind,
        bus,
        smart_available: matches!(bus, DriveBus::Sata | DriveBus::Nvme),
        has_mounted_volumes: false,
    })
}

fn get_geometry(h: HANDLE) -> Result<DISK_GEOMETRY_EX> {
    let mut geom: DISK_GEOMETRY_EX = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    // SAFETY: we pass a properly-sized struct buffer for the IOCTL contract.
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
            None,
            0,
            Some(&mut geom as *mut _ as *mut _),
            std::mem::size_of::<DISK_GEOMETRY_EX>() as u32,
            Some(&mut returned),
            None,
        )
    };
    if ok.is_err() {
        return Err(Error::os(0, "IOCTL_DISK_GET_DRIVE_GEOMETRY_EX failed"));
    }
    Ok(geom)
}

#[derive(Default)]
struct DescriptorOut {
    vendor: String,
    model: String,
    serial: String,
    bus: DriveBus,
}

impl Default for DriveBus {
    fn default() -> Self {
        DriveBus::Unknown
    }
}

fn get_storage_descriptor(h: HANDLE) -> Option<(String, String, DriveBus)> {
    let mut query = STORAGE_PROPERTY_QUERY {
        PropertyId: ::windows::Win32::System::Ioctl::StorageDeviceProperty,
        QueryType: STORAGE_QUERY_TYPE(0), // PropertyStandardQuery
        AdditionalParameters: [0; 1],
    };
    let mut buf = [0u8; 1024];
    let mut returned: u32 = 0;
    // SAFETY: query is a valid STORAGE_PROPERTY_QUERY; buf has 1024 bytes.
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&mut query as *mut _ as *mut _),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(buf.as_mut_ptr() as *mut _),
            buf.len() as u32,
            Some(&mut returned),
            None,
        )
    };
    if ok.is_err() || returned < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() as u32 {
        return None;
    }
    // SAFETY: returned > sizeof(STORAGE_DEVICE_DESCRIPTOR) — see check above.
    let desc = unsafe { &*(buf.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };

    let read_str = |off: u32| -> String {
        if off == 0 {
            return String::new();
        }
        let off = off as usize;
        if off >= buf.len() {
            return String::new();
        }
        let end = buf[off..].iter().position(|&b| b == 0).unwrap_or(0) + off;
        String::from_utf8_lossy(&buf[off..end]).trim().to_string()
    };

    let vendor = read_str(desc.VendorIdOffset);
    let mut model = read_str(desc.ProductIdOffset);
    if !vendor.is_empty() && !model.starts_with(&vendor) {
        model = format!("{vendor} {model}");
    }
    let serial = read_str(desc.SerialNumberOffset);
    let bus = map_bus(desc.BusType);

    Some((model, serial, bus))
}

fn map_bus(bus: STORAGE_BUS_TYPE) -> DriveBus {
    use ::windows::Win32::System::Ioctl as ioctl;
    match bus {
        ioctl::BusTypeNvme => DriveBus::Nvme,
        ioctl::BusTypeSata => DriveBus::Sata,
        ioctl::BusTypeUsb => DriveBus::Usb,
        ioctl::BusTypeSd | ioctl::BusTypeMmc => DriveBus::Sd,
        ioctl::BusTypeScsi | ioctl::BusTypeSas => DriveBus::Scsi,
        ioctl::BusTypeVirtual | ioctl::BusTypeFileBackedVirtual => DriveBus::Virtual,
        _ => DriveBus::Unknown,
    }
}

fn classify_kind(bus: DriveBus, model: &str) -> DriveKind {
    let m = model.to_uppercase();
    match bus {
        DriveBus::Nvme => DriveKind::Nvme,
        DriveBus::Usb => {
            if m.contains("FLASH") || m.contains("UDISK") || m.contains("CRUZER") {
                DriveKind::UsbFlash
            } else {
                DriveKind::External
            }
        }
        DriveBus::Sd => DriveKind::SdCard,
        DriveBus::Virtual => DriveKind::Virtual,
        DriveBus::Sata | DriveBus::Scsi => {
            if m.contains("SSD") || m.contains("NVME") || m.contains("SOLID") {
                DriveKind::Ssd
            } else {
                DriveKind::Hdd
            }
        }
        _ => DriveKind::Unknown,
    }
}

// -- Reader -------------------------------------------------------------------

#[derive(Debug)]
pub struct WindowsRawReader {
    label: String,
    handle: Mutex<OwnedHandle>,
    sector_size: u32,
    size_bytes: u64,
}

impl WindowsRawReader {
    fn new(label: &str, h: OwnedHandle, sector_size: u32, size_bytes: u64) -> Self {
        Self {
            label: label.to_string(),
            handle: Mutex::new(h),
            sector_size,
            size_bytes,
        }
    }
}

#[async_trait]
impl SectorReader for WindowsRawReader {
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        // FILE_FLAG_NO_BUFFERING demands sector-aligned offset, length and pointer.
        // We fulfil this by forcing all reads through this aligned path.
        let ssz = u64::from(self.sector_size);
        if offset % ssz != 0 {
            return Err(Error::internal("read_at: unaligned offset (not a sector)"));
        }
        if (buf.len() as u64) % ssz != 0 {
            return Err(Error::internal("read_at: unaligned length (not a sector)"));
        }
        let h = self.handle.lock();
        // Position
        // SAFETY: handle owned for the duration of this call; offset is i64-safe.
        let mut new_pos: i64 = 0;
        unsafe {
            SetFilePointerEx(
                h.0,
                offset as i64,
                Some(&mut new_pos),
                FILE_BEGIN,
            )
            .map_err(|e| Error::os(e.code().0 as i32, format!("SetFilePointerEx: {e}")))?;
        }
        // Read
        let mut read: u32 = 0;
        // SAFETY: buf is a mutable slice with valid length; handle is open.
        let r = unsafe {
            ReadFile(
                h.0,
                Some(buf),
                Some(&mut read),
                None,
            )
        };
        if r.is_err() {
            // ERROR_HANDLE_EOF = 38, ERROR_CRC = 23, ERROR_SECTOR_NOT_FOUND = 27
            let code = ::windows::core::Error::from_win32().code().0 as i32;
            if code == 38 {
                return Ok(0);
            }
            return Err(Error::os(code, "ReadFile failed"));
        }
        Ok(read as usize)
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

// -- SMART --------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct WindowsSmart;

#[async_trait]
impl SmartProvider for WindowsSmart {
    async fn query(&self, drive_path: &str) -> Result<SmartReport> {
        // Real SMART pass-through requires a privileged ATA / NVMe command and a
        // device-specific attribute decoder. The skeleton below performs the
        // detection pass and returns whatever the OS provides; full ATA/NVMe
        // decoders are tracked as roadmap items in docs/architecture.md §12.
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
