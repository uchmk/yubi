//! Non-admin access to the driver's haptic endpoints.
//!
//! By default only administrators can open the haptics pipe / device. Setting the DWORD
//! `AllowNonAdminAccess = 1` under the driver's `Parameters` keys opens them to interactive users
//! after the device restarts. Reading the values needs no elevation; writing them does, so
//! [`enable_non_admin_access`] launches an elevated helper (UAC prompt). Command-line building and
//! instance-id filtering are pure functions and are unit tested; the elevation itself is not.

use std::ffi::c_void;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GETIDLIST_FILTER_NONE, CM_Get_Device_ID_List_SizeW, CM_Get_Device_ID_ListW, CR_SUCCESS,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, RRF_RT_REG_DWORD, RRF_SUBKEY_WOW6464KEY,
    RegCloseKey, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::PCWSTR;

/// UMDF service key (USB). Value lives in its `Parameters` subkey.
pub const USB_SERVICE_KEY: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\WUDF\Services\AmtPtpDeviceUsbUm";
/// HID filter service key (Bluetooth).
pub const BT_SERVICE_KEY: &str = r"SYSTEM\CurrentControlSet\Services\AmtPtpHidFilter";
pub const VALUE_NAME: &str = "AllowNonAdminAccess";

/// Result of reading `AllowNonAdminAccess` from both keys. `None` = the value (or key) is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionState {
    pub usb: Option<bool>,
    pub bluetooth: Option<bool>,
}

impl PermissionState {
    /// True when every transport whose key exists has the value set to 1.
    pub fn is_enabled(&self) -> bool {
        let set = [self.usb, self.bluetooth];
        set.contains(&Some(true)) && set.iter().all(|v| *v != Some(false))
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn parameters_key(service_key: &str) -> String {
    format!(r"{service_key}\Parameters")
}

fn read_dword(subkey: &str, name: &str) -> Option<u32> {
    let k = wide(subkey);
    let n = wide(name);
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: strings are NUL-terminated; `data`/`size` describe a 4-byte buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    if r.0 == 0 { Some(data) } else { None }
}

fn key_exists(subkey: &str) -> bool {
    let k = wide(subkey);
    let mut h = HKEY::default();
    // SAFETY: NUL-terminated string; `h` receives the key handle.
    let r = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut h,
        )
    };
    if r.0 == 0 {
        // SAFETY: opened above.
        unsafe {
            let _ = RegCloseKey(h);
        }
        true
    } else {
        false
    }
}

/// Read the current registry state (no elevation needed).
pub fn read_state() -> PermissionState {
    let get = |svc: &str| read_dword(&parameters_key(svc), VALUE_NAME).map(|v| v == 1);
    PermissionState {
        usb: get(USB_SERVICE_KEY),
        bluetooth: get(BT_SERVICE_KEY),
    }
}

/// True when `id` only has characters that can appear in a device instance id and cannot break out
/// of the quoted argument or add a command (no `"`, `%`, `^`, `|`, `<`, `>`, spaces, ...).
/// The ids go into an elevated command line, so anything else is dropped.
pub fn is_safe_instance_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '\\' | '&' | '_' | '{' | '}' | '-' | '.' | '#')
        })
}

/// `reg add` command line (as one string) for one `Parameters` key under HKLM. `system_dir` is
/// the System32 directory (no trailing backslash).
pub fn reg_add_command(system_dir: &str, parameters_key_path: &str) -> String {
    format!(
        r#""{system_dir}\reg.exe" add "HKLM\{parameters_key_path}" /v {VALUE_NAME} /t REG_DWORD /d 1 /f /reg:64"#
    )
}

/// `pnputil /restart-device` command line for one instance id (`pnputil.exe` lives in System32).
pub fn restart_device_command(system_dir: &str, instance_id: &str) -> String {
    format!(r#""{system_dir}\pnputil.exe" /restart-device "{instance_id}""#)
}

/// Parameters for `cmd.exe`. The registry writes are chained with `&&`, so the device restarts only
/// run when every write succeeded; the restarts themselves are independent (`&`). Instance ids that
/// fail [`is_safe_instance_id`] are dropped. The whole thing is wrapped in one pair of quotes for
/// `cmd /c`.
pub fn build_elevated_params(
    system_dir: &str,
    parameters_keys: &[String],
    instance_ids: &[String],
) -> String {
    let mut chain: Vec<String> = parameters_keys
        .iter()
        .map(|k| reg_add_command(system_dir, k))
        .collect();
    let restarts: Vec<String> = instance_ids
        .iter()
        .filter(|i| is_safe_instance_id(i))
        .map(|i| restart_device_command(system_dir, i))
        .collect();
    if !restarts.is_empty() {
        chain.push(format!("({})", restarts.join(" & ")));
    }
    format!(r#"/c "{}""#, chain.join(" && "))
}

/// True for device instance ids that belong to a physical Magic Trackpad node worth restarting:
/// USB device nodes (`USB\VID_05AC&PID_xxxx\...`, not the `&MI_` interfaces) and Bluetooth HID nodes
/// (`...VID&0205AC_PID&xxxx...`).
pub fn is_trackpad_instance(id: &str) -> bool {
    let u = id.to_ascii_uppercase();
    if u.starts_with(r"USB\VID_05AC&PID_") {
        return !u.contains("&MI_");
    }
    u.contains("VID&0205AC_PID&") && (u.starts_with("HID\\") || u.starts_with("BTHENUM\\"))
}

/// Instance ids of the trackpad nodes currently known to PnP (best effort).
pub fn find_trackpad_instances() -> Vec<String> {
    let mut len = 0u32;
    // SAFETY: plain size query with no filter.
    if unsafe { CM_Get_Device_ID_List_SizeW(&mut len, PCWSTR::null(), CM_GETIDLIST_FILTER_NONE) }
        != CR_SUCCESS
        || len == 0
    {
        return Vec::new();
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` has the size the API asked for.
    if unsafe { CM_Get_Device_ID_ListW(PCWSTR::null(), &mut buf, CM_GETIDLIST_FILTER_NONE) }
        != CR_SUCCESS
    {
        return Vec::new();
    }
    buf.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .filter(|s| is_trackpad_instance(s) && is_safe_instance_id(s))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionError {
    /// The user declined the UAC prompt.
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnableOutcome {
    /// Exit code of the elevated helper (that of its last command).
    pub exit_code: u32,
    /// Instance ids the helper was *asked* to restart (not confirmed); empty means the user should
    /// replug the trackpad.
    pub restart_requested: Vec<String>,
    /// State read after the helper finished (always enabled when this outcome is returned).
    pub state: PermissionState,
}

/// The System32 directory without a trailing backslash.
fn system_directory() -> Result<String, PermissionError> {
    let mut buf = [0u16; 260];
    // SAFETY: `buf` is a valid writable buffer.
    let n = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(PermissionError::Failed(
            "could not get the system directory".into(),
        ));
    }
    Ok(String::from_utf16_lossy(&buf[..n])
        .trim_end_matches('\\')
        .to_string())
}

/// Launch the elevated helper (UAC prompt) that sets `AllowNonAdminAccess = 1` and then requests a
/// restart of the trackpad device nodes. Blocks until it finishes (up to 60 s): call it from a
/// worker thread. Returns `Ok` only when the registry now shows the value enabled; if
/// `restart_requested` is empty, tell the user to replug / reconnect the trackpad.
/// Never called from tests.
pub fn enable_non_admin_access() -> Result<EnableOutcome, PermissionError> {
    let keys: Vec<String> = [USB_SERVICE_KEY, BT_SERVICE_KEY]
        .iter()
        .filter(|svc| key_exists(svc))
        .map(|svc| parameters_key(svc))
        .collect();
    if keys.is_empty() {
        return Err(PermissionError::Failed(
            "driver registry keys not found (driver not installed?)".into(),
        ));
    }
    let sys = system_directory()?;
    let ids = find_trackpad_instances();
    let params = build_elevated_params(&sys, &keys, &ids);
    let exit_code = run_elevated(&sys, &params)?;
    let state = read_state();
    if !state.is_enabled() {
        return Err(PermissionError::Failed(format!(
            "the registry value is still not enabled after the helper finished (exit code {exit_code})"
        )));
    }
    Ok(EnableOutcome {
        exit_code,
        restart_requested: ids,
        state,
    })
}

fn run_elevated(system_dir: &str, params: &str) -> Result<u32, PermissionError> {
    let verb = wide("runas");
    let file = wide(&format!(r"{system_dir}\cmd.exe"));
    let dir = wide(system_dir);
    let params = wide(params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        lpDirectory: PCWSTR(dir.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` and the strings it points to outlive the call.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return if (e.code().0 as u32) & 0xFFFF == ERROR_CANCELLED.0 {
            Err(PermissionError::Cancelled)
        } else {
            Err(PermissionError::Failed(e.message()))
        };
    }
    let h = info.hProcess;
    if h.is_invalid() {
        return Err(PermissionError::Failed("no process handle".into()));
    }
    // SAFETY: valid process handle returned by ShellExecuteExW.
    unsafe {
        let w = WaitForSingleObject(h, 60_000);
        let mut code = 0u32;
        let result = if w == WAIT_OBJECT_0 {
            if GetExitCodeProcess(h, &mut code).is_ok() {
                Ok(code)
            } else {
                Err(PermissionError::Failed(
                    "could not read the helper's exit code".into(),
                ))
            }
        } else {
            Err(PermissionError::Failed(
                "timed out after 60 s waiting for the elevated helper (it may still finish; check the state again)"
                    .into(),
            ))
        };
        let _ = CloseHandle(h);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYS: &str = r"C:\Windows\System32";

    #[test]
    fn reg_command_line() {
        assert_eq!(
            reg_add_command(SYS, &parameters_key(BT_SERVICE_KEY)),
            r#""C:\Windows\System32\reg.exe" add "HKLM\SYSTEM\CurrentControlSet\Services\AmtPtpHidFilter\Parameters" /v AllowNonAdminAccess /t REG_DWORD /d 1 /f /reg:64"#
        );
    }

    #[test]
    fn elevated_params_chain_regs_then_restarts() {
        let keys = vec![
            parameters_key(USB_SERVICE_KEY),
            parameters_key(BT_SERVICE_KEY),
        ];
        let ids = vec![r"USB\VID_05AC&PID_0265\5&1&0&1".to_string()];
        let p = build_elevated_params(SYS, &keys, &ids);
        assert!(p.starts_with(r#"/c ""#) && p.ends_with('"'));
        assert_eq!(p.matches("reg.exe").count(), 2);
        assert!(p.contains(r#" /reg:64 && ("#));
        assert!(p.contains(
            r#"("C:\Windows\System32\pnputil.exe" /restart-device "USB\VID_05AC&PID_0265\5&1&0&1")"#
        ));
        assert!(p.contains(r"WUDF\Services\AmtPtpDeviceUsbUm\Parameters"));
    }

    #[test]
    fn no_ids_means_no_pnputil() {
        let p = build_elevated_params(SYS, &[parameters_key(BT_SERVICE_KEY)], &[]);
        assert!(!p.contains("pnputil"));
    }

    #[test]
    fn malicious_ids_are_dropped() {
        let ids = vec![
            r#"USB\VID_05AC&PID_0265\x" & calc & ""#.to_string(),
            r"USB\VID_05AC&PID_0265\5&1&0&1".to_string(),
            r"USB\VID_05AC&PID_0265\a%b".to_string(),
        ];
        let p = build_elevated_params(SYS, &[parameters_key(BT_SERVICE_KEY)], &ids);
        assert!(!p.contains("calc"));
        assert!(!p.contains('%'));
        assert_eq!(p.matches("pnputil.exe").count(), 1);
        assert!(!is_safe_instance_id(r#"a"b"#));
        assert!(!is_safe_instance_id("a b"));
        assert!(!is_safe_instance_id(""));
        assert!(is_safe_instance_id(
            r"HID\{00001124-0000-1000-8000-00805F9B34FB}_VID&0205AC_PID&0265\9&1&0&0000"
        ));
    }

    #[test]
    fn instance_filter() {
        assert!(is_trackpad_instance(r"USB\VID_05AC&PID_0265\5&2A1B&0&3"));
        assert!(!is_trackpad_instance(
            r"USB\VID_05AC&PID_0265&MI_01\6&1&0&0001"
        ));
        assert!(!is_trackpad_instance(r"USB\VID_046D&PID_C52B\5&1&0&1"));
        assert!(is_trackpad_instance(
            r"HID\{00001124-0000-1000-8000-00805F9B34FB}_VID&0205AC_PID&0265\9&1&0&0000"
        ));
        assert!(!is_trackpad_instance(
            r"HID\VID_05AC&PID_0265&COL01\7&1&0&0000"
        ));
    }

    #[test]
    fn enabled_logic() {
        let s = |usb, bluetooth| PermissionState { usb, bluetooth };
        assert!(s(Some(true), Some(true)).is_enabled());
        assert!(s(Some(true), None).is_enabled());
        assert!(!s(Some(true), Some(false)).is_enabled());
        assert!(!s(None, None).is_enabled());
    }

    #[test]
    fn read_state_does_not_panic() {
        let _ = read_state();
    }
}
