//! Low-level I/O to the official Apple Magic Trackpad driver (v2.3) haptics endpoints.
//!
//! Everything here is blocking and must be called from the haptics worker thread only.
//!
//! Protocol facts:
//! - USB: named pipe, message mode, one request per connection. Request is one command byte plus
//!   optional payload. Response is two bytes `[status, payload]`.
//! - Bluetooth: `DeviceIoControl` on the haptic device (actuate) / control device (battery, admin only).
//!
//! The pipe is opened for overlapped I/O and both the write and the read wait at most
//! [`PIPE_IO_TIMEOUT_MS`]; on timeout the I/O is cancelled and an error status results, so a driver
//! that accepts the connection but never answers cannot stall the worker thread for long (callers
//! are never blocked, they only enqueue).

use std::ffi::c_void;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PATH_NOT_FOUND,
    ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    NAMED_PIPE_MODE, PIPE_READMODE_MESSAGE, SetNamedPipeHandleState, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::PCWSTR;

use super::HapticStatus;

pub const USB_PIPE: &str = r"\\.\pipe\AmtPtpControlPanelUsbUmInterface";
pub const BT_HAPTIC_DEVICE: &str = r"\\.\AmtPtpHapticDeviceUm";
pub const BT_CONTROL_DEVICE: &str = r"\\.\AmtPtpControlDeviceUm";

pub const IOCTL_ACTUATE: u32 = 0x0022_A008;
pub const IOCTL_GET_BATTERY: u32 = 0x0022_2004;

pub const CMD_GET_BATTERY: u8 = 0x01;
pub const CMD_ACTUATE: u8 = 0x02;

pub const RESP_OK: u8 = 0x00;
pub const RESP_UNKNOWN: u8 = 0xFE;
pub const RESP_FAIL: u8 = 0xFF;

/// How long to wait for a busy pipe instance before giving up (WaitNamedPipeW).
pub const PIPE_CONNECT_TIMEOUT_MS: u32 = 200;

/// How long a pipe write or read may take before it is cancelled.
pub const PIPE_IO_TIMEOUT_MS: u32 = 200;

/// Why an endpoint could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HapticError {
    NotFound,
    AccessDenied,
    Other(String),
}

impl HapticError {
    pub fn to_status(&self) -> HapticStatus {
        match self {
            HapticError::NotFound => HapticStatus::NotFound,
            HapticError::AccessDenied => HapticStatus::AccessDenied,
            HapticError::Other(m) => HapticStatus::Error(m.clone()),
        }
    }
}

/// Map a Win32 error code from opening an endpoint to a [`HapticError`].
pub fn classify_win32(code: u32) -> HapticError {
    if code == ERROR_FILE_NOT_FOUND.0 || code == ERROR_PATH_NOT_FOUND.0 {
        HapticError::NotFound
    } else if code == ERROR_ACCESS_DENIED.0 {
        HapticError::AccessDenied
    } else {
        HapticError::Other(format!("Win32 error {code}"))
    }
}

fn win32_code(e: &windows::core::Error) -> u32 {
    // HRESULT_FROM_WIN32 keeps the Win32 code in the low 16 bits.
    (e.code().0 as u32) & 0xFFFF
}

fn win_err(e: windows::core::Error) -> HapticError {
    classify_win32(win32_code(&e))
}

/// USB `ACTUATE` request: command byte + 24-bit little-endian value.
pub fn encode_usb_actuate(value: u32) -> [u8; 4] {
    let v = value & 0x00FF_FFFF;
    [CMD_ACTUATE, v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

/// Interpret a two-byte USB response and return the payload byte.
pub fn parse_usb_response(resp: &[u8]) -> Result<u8, HapticError> {
    match resp {
        [RESP_OK, payload, ..] => Ok(*payload),
        [RESP_FAIL, ..] => Err(HapticError::Other("driver reported FAIL".into())),
        [RESP_UNKNOWN, ..] => Err(HapticError::Other("driver reported unknown command".into())),
        [s, ..] => Err(HapticError::Other(format!("unexpected status 0x{s:02X}"))),
        [] => Err(HapticError::Other("empty response".into())),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateFileW and is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn open_raw(
    path: &str,
    access: u32,
    flags: FILE_FLAGS_AND_ATTRIBUTES,
) -> windows::core::Result<Handle> {
    let w = wide(path);
    // SAFETY: `w` is NUL-terminated and outlives the call.
    unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            flags,
            None,
        )
    }
    .map(Handle)
}

fn open(path: &str, access: u32) -> Result<Handle, HapticError> {
    open_raw(path, access, FILE_ATTRIBUTE_NORMAL).map_err(win_err)
}

enum Io<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

/// One read or write on an overlapped handle, waiting at most [`PIPE_IO_TIMEOUT_MS`]. On timeout
/// the I/O is cancelled (and reaped, so the buffers are no longer in use) and an error returned.
fn overlapped_io(h: &Handle, io: Io) -> Result<u32, HapticError> {
    // SAFETY: plain manual-reset event; closed through `Handle` below.
    let ev = Handle(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(win_err)?);
    let mut ov = OVERLAPPED {
        hEvent: ev.0,
        ..Default::default()
    };
    // SAFETY: valid handle; the buffer and `ov` stay alive until the I/O has completed or been
    // cancelled and reaped (see below).
    let started = unsafe {
        match io {
            Io::Read(b) => ReadFile(h.0, Some(b), None, Some(&mut ov)),
            Io::Write(b) => WriteFile(h.0, Some(b), None, Some(&mut ov)),
        }
    };
    if let Err(e) = started
        && win32_code(&e) != ERROR_IO_PENDING.0
    {
        return Err(win_err(e));
    }
    // SAFETY: valid event handle.
    let waited = unsafe { WaitForSingleObject(ev.0, PIPE_IO_TIMEOUT_MS) };
    let timed_out = waited != WAIT_OBJECT_0;
    if timed_out {
        // SAFETY: cancels the request that uses `ov`; GetOverlappedResult below waits for it to
        // finish so `ov` and the buffer are released by the kernel before they go out of scope.
        unsafe {
            let _ = CancelIoEx(h.0, Some(&ov));
        }
    }
    let mut n = 0u32;
    // SAFETY: `ov` belongs to the request started above.
    let r = unsafe { GetOverlappedResult(h.0, &ov, &mut n, timed_out) };
    if timed_out {
        return Err(HapticError::Other(format!(
            "no response from the driver within {PIPE_IO_TIMEOUT_MS} ms"
        )));
    }
    r.map_err(win_err)?;
    Ok(n)
}

/// One USB request/response exchange (one connection per request).
pub fn usb_request(req: &[u8]) -> Result<[u8; 2], HapticError> {
    let wpath = wide(USB_PIPE);
    let mut attempt = 0;
    let h = loop {
        match open_raw(
            USB_PIPE,
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
        ) {
            Ok(h) => break h,
            Err(e) if win32_code(&e) == ERROR_PIPE_BUSY.0 && attempt < 2 => {
                attempt += 1;
                // SAFETY: `wpath` is NUL-terminated.
                unsafe {
                    let _ = WaitNamedPipeW(PCWSTR(wpath.as_ptr()), PIPE_CONNECT_TIMEOUT_MS);
                }
            }
            Err(e) => return Err(win_err(e)),
        }
    };
    let mode = NAMED_PIPE_MODE(PIPE_READMODE_MESSAGE.0);
    // SAFETY: valid handle; `mode` outlives the call.
    unsafe { SetNamedPipeHandleState(h.0, Some(&mode), None, None) }.map_err(win_err)?;
    overlapped_io(&h, Io::Write(req))?;
    let mut buf = [0u8; 2];
    let read = overlapped_io(&h, Io::Read(&mut buf))?;
    if read < 2 {
        return Err(HapticError::Other(format!("short response ({read} bytes)")));
    }
    Ok(buf)
}

/// Bluetooth haptic device handle (kept open between actuations).
pub struct BtDevice(Handle);

impl BtDevice {
    pub fn open() -> Result<Self, HapticError> {
        open(BT_HAPTIC_DEVICE, GENERIC_WRITE.0).map(BtDevice)
    }

    pub fn actuate(&self, value: u32) -> Result<(), HapticError> {
        let input: u32 = value & 0x00FF_FFFF;
        let mut ret = 0u32;
        // SAFETY: valid handle; input buffer is 4 bytes and outlives the call.
        unsafe {
            DeviceIoControl(
                self.0.0,
                IOCTL_ACTUATE,
                Some(&input as *const u32 as *const c_void),
                4,
                None,
                0,
                Some(&mut ret),
                None,
            )
        }
        .map_err(win_err)
    }
}

/// Battery level through the BT control device (needs admin; best effort).
pub fn bt_battery() -> Result<u8, HapticError> {
    let h = open(BT_CONTROL_DEVICE, GENERIC_READ.0 | GENERIC_WRITE.0)?;
    let mut out: u32 = 0;
    let mut ret = 0u32;
    // SAFETY: valid handle; output buffer is 4 bytes and outlives the call.
    unsafe {
        DeviceIoControl(
            h.0,
            IOCTL_GET_BATTERY,
            None,
            0,
            Some(&mut out as *mut u32 as *mut c_void),
            4,
            Some(&mut ret),
            None,
        )
    }
    .map_err(win_err)?;
    Ok(out as u8)
}

/// Which transport last worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Usb,
    Bluetooth,
}

/// Transport selection with a cached Bluetooth handle. Worker-thread only.
pub struct Backend {
    last_good: Option<Kind>,
    bt: Option<BtDevice>,
}

impl Backend {
    pub fn new() -> Self {
        Backend {
            last_good: None,
            bt: None,
        }
    }

    fn usb_actuate(value: u32) -> Result<(), HapticError> {
        let r = usb_request(&encode_usb_actuate(value))?;
        parse_usb_response(&r).map(|_| ())
    }

    fn bt_actuate(&mut self, value: u32) -> Result<(), HapticError> {
        if self.bt.is_none() {
            self.bt = Some(BtDevice::open()?);
        }
        let r = match &self.bt {
            Some(d) => d.actuate(value),
            None => Ok(()),
        };
        if r.is_err() {
            self.bt = None; // reopen next time
        }
        r
    }

    /// Fire one click on whichever transport works (last good first).
    pub fn actuate(&mut self, value: u32) -> Result<Kind, HapticError> {
        let order = if self.last_good == Some(Kind::Bluetooth) {
            [Kind::Bluetooth, Kind::Usb]
        } else {
            [Kind::Usb, Kind::Bluetooth]
        };
        let mut errs: Vec<HapticError> = Vec::new();
        for k in order {
            let r = match k {
                Kind::Usb => Self::usb_actuate(value),
                Kind::Bluetooth => self.bt_actuate(value),
            };
            match r {
                Ok(()) => {
                    self.last_good = Some(k);
                    return Ok(k);
                }
                Err(e) => errs.push(e),
            }
        }
        self.last_good = None;
        Err(merge_errors(errs))
    }

    /// Read-only probe: USB battery request, then BT device open. Returns status and battery.
    pub fn probe(&mut self) -> (HapticStatus, Option<u8>) {
        let mut errs: Vec<HapticError> = Vec::new();
        match usb_request(&[CMD_GET_BATTERY]).and_then(|r| parse_usb_response(&r)) {
            Ok(level) => {
                self.last_good = Some(Kind::Usb);
                return (HapticStatus::Available, Some(level));
            }
            Err(e) => errs.push(e),
        }
        match BtDevice::open() {
            Ok(d) => {
                self.bt = Some(d);
                self.last_good = Some(Kind::Bluetooth);
                return (HapticStatus::Available, bt_battery().ok());
            }
            Err(e) => errs.push(e),
        }
        self.last_good = None;
        (merge_errors(errs).to_status(), None)
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::new()
    }
}

/// Combine per-transport errors: AccessDenied beats Other beats NotFound.
pub fn merge_errors(errs: Vec<HapticError>) -> HapticError {
    if errs.contains(&HapticError::AccessDenied) {
        return HapticError::AccessDenied;
    }
    if let Some(e) = errs.iter().find(|e| matches!(e, HapticError::Other(_))) {
        return e.clone();
    }
    HapticError::NotFound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actuate_encoding_is_little_endian_24bit() {
        assert_eq!(encode_usb_actuate(0x06_06_17), [0x02, 0x17, 0x06, 0x06]);
        assert_eq!(encode_usb_actuate(0xFF_08_08_1E), [0x02, 0x1E, 0x08, 0x08]);
    }

    #[test]
    fn response_parsing() {
        assert_eq!(parse_usb_response(&[0, 87]), Ok(87));
        assert!(parse_usb_response(&[0xFF, 0]).is_err());
        assert!(parse_usb_response(&[0xFE, 0]).is_err());
        assert!(parse_usb_response(&[]).is_err());
    }

    #[test]
    fn win32_classification() {
        assert_eq!(classify_win32(2), HapticError::NotFound);
        assert_eq!(classify_win32(3), HapticError::NotFound);
        assert_eq!(classify_win32(5), HapticError::AccessDenied);
        assert!(matches!(classify_win32(87), HapticError::Other(_)));
    }

    #[test]
    fn merge_priority() {
        use HapticError::*;
        assert_eq!(merge_errors(vec![NotFound, AccessDenied]), AccessDenied);
        assert_eq!(merge_errors(vec![NotFound, NotFound]), NotFound);
        assert_eq!(
            merge_errors(vec![NotFound, Other("x".into())]),
            Other("x".into())
        );
    }
}
