//! Start with Windows: one value, `yubi`, under HKCU `...\CurrentVersion\Run`. No other value is touched.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Name of the value under the Run key.
pub const VALUE_NAME: &str = "yubi";

/// The command line stored in the Run value: the quoted exe path and `--background`.
pub fn run_command_line(exe: &Path) -> String {
    format!("\"{}\" --background", exe.display())
}

/// Whether startup should (re)write the Run value: only when the toggle is on and the stored value
/// is missing or points somewhere else (the exe moved). Windows paths compare case-insensitively.
pub fn needs_rewrite(enabled: bool, current: Option<&str>, desired: &str) -> bool {
    enabled && !current.is_some_and(|c| c.eq_ignore_ascii_case(desired))
}

/// The exe path inside a Run command line (`"<exe>" --background` or a bare path).
pub fn command_exe(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    let exe = match cmd.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => cmd.split(" --").next()?,
    };
    (!exe.is_empty()).then(|| PathBuf::from(exe))
}

/// Builds on the RAM disk (`R:\`) vanish on reboot, so they cannot be used for autostart.
pub fn is_ram_disk_build(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    s.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("R:\\"))
}

/// What the 全般 page tells the user about autostart besides the toggle.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Status {
    /// This exe lives on the RAM disk.
    pub ram_disk_build: bool,
    /// The Run value exists but its exe is gone.
    pub target_missing: bool,
    /// The toggle is on but the Run value is absent or points elsewhere.
    pub not_registered: bool,
    /// The last write to the Run value failed.
    pub error: Option<String>,
}

static CACHE: Mutex<Option<(Instant, bool, Status)>> = Mutex::new(None);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

fn set_error(e: Option<String>) {
    if let Ok(mut l) = LAST_ERROR.lock() {
        *l = e;
    }
    if let Ok(mut c) = CACHE.lock() {
        *c = None;
    }
}

/// Look at the registry and disk, at most every 2 seconds. Only the 全般 page calls this (while
/// it is shown), so it never runs on the UI thread otherwise.
pub fn status(enabled: bool) -> Status {
    if let Ok(c) = CACHE.lock()
        && let Some((at, on, st)) = c.as_ref()
        && *on == enabled
        && at.elapsed() < Duration::from_secs(2)
    {
        return st.clone();
    }
    let exe = std::env::current_exe().ok();
    let current = reg::read().ok().flatten();
    let st = Status {
        ram_disk_build: exe.as_deref().is_some_and(is_ram_disk_build),
        target_missing: current
            .as_deref()
            .and_then(command_exe)
            .is_some_and(|p| !p.exists()),
        not_registered: exe
            .as_deref()
            .is_some_and(|e| needs_rewrite(enabled, current.as_deref(), &run_command_line(e))),
        error: LAST_ERROR.lock().ok().and_then(|l| l.clone()),
    };
    if let Ok(mut c) = CACHE.lock() {
        *c = Some((Instant::now(), enabled, st.clone()));
    }
    st
}

/// Apply the toggle: write or delete the value. The error is also kept for `status`.
pub fn apply(enabled: bool) -> std::io::Result<()> {
    let r = try_apply(enabled);
    match &r {
        Ok(()) => set_error(None),
        Err(e) => {
            eprintln!("yubi: cannot update the autostart entry: {e}");
            set_error(Some(e.to_string()));
        }
    }
    r
}

/// On startup, repair the value when the toggle is on (see `needs_rewrite`). A failure is kept
/// for `status`.
pub fn sync(enabled: bool) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let desired = run_command_line(&exe);
    let current = reg::read().unwrap_or(None);
    if needs_rewrite(enabled, current.as_deref(), &desired)
        && let Err(e) = reg::write(&desired)
    {
        eprintln!("yubi: cannot repair the autostart entry: {e}");
        set_error(Some(e.to_string()));
    }
}

fn try_apply(enabled: bool) -> std::io::Result<()> {
    if enabled {
        let exe = std::env::current_exe()?;
        reg::write(&run_command_line(&exe))
    } else {
        reg::delete()
    }
}

mod reg {
    use std::io;

    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
    use windows::Win32::System::Registry::*;
    use windows::core::{HSTRING, PCWSTR, w};

    use super::VALUE_NAME;

    const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

    fn check(e: WIN32_ERROR) -> io::Result<()> {
        if e == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(e.0 as i32))
        }
    }

    fn close(key: HKEY) {
        unsafe {
            let _ = RegCloseKey(key);
        }
    }

    pub fn read() -> io::Result<Option<String>> {
        let mut key = HKEY::default();
        let e = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, KEY, None, KEY_QUERY_VALUE, &mut key) };
        if e == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(e)?;
        let name = HSTRING::from(VALUE_NAME);
        let mut buf = vec![0u16; 1024];
        let mut ty = REG_VALUE_TYPE(0);
        let mut len = (buf.len() * 2) as u32;
        let e = unsafe {
            RegQueryValueExW(
                key,
                &name,
                None,
                Some(&mut ty),
                Some(buf.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        close(key);
        if e == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(e)?;
        if ty != REG_SZ {
            return Ok(None);
        }
        let n = (len as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        Ok(Some(s.trim_end_matches('\0').to_string()))
    }

    pub fn write(value: &str) -> io::Result<()> {
        let mut key = HKEY::default();
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                KEY,
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        })?;
        let mut wide: Vec<u16> = value.encode_utf16().collect();
        wide.push(0);
        let bytes: Vec<u8> = wide.iter().flat_map(|u| u.to_le_bytes()).collect();
        let r = check(unsafe {
            RegSetValueExW(key, &HSTRING::from(VALUE_NAME), None, REG_SZ, Some(&bytes))
        });
        close(key);
        r
    }

    pub fn delete() -> io::Result<()> {
        let mut key = HKEY::default();
        let e = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, KEY, None, KEY_SET_VALUE, &mut key) };
        if e == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(e)?;
        let e = unsafe { RegDeleteValueW(key, &HSTRING::from(VALUE_NAME)) };
        close(key);
        if e == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            check(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn command_line_quotes_the_path() {
        let p = PathBuf::from(r"C:\Program Files\yubi\yubi.exe");
        assert_eq!(
            run_command_line(&p),
            r#""C:\Program Files\yubi\yubi.exe" --background"#
        );
    }

    #[test]
    fn exe_from_command_line() {
        assert_eq!(
            command_exe(r#""C:\a b\yubi.exe" --background"#),
            Some(PathBuf::from(r"C:\a b\yubi.exe"))
        );
        assert_eq!(
            command_exe(r"C:\a\yubi.exe --background"),
            Some(PathBuf::from(r"C:\a\yubi.exe"))
        );
        assert_eq!(command_exe(""), None);
    }

    #[test]
    fn ram_disk_detection() {
        assert!(is_ram_disk_build(Path::new(
            r"R:\cargo-target\yubi\release\yubi.exe"
        )));
        assert!(is_ram_disk_build(Path::new(r"r:\x.exe")));
        assert!(!is_ram_disk_build(Path::new(r"C:\dev\yubi.exe")));
        assert!(!is_ram_disk_build(Path::new(r"R")));
    }

    #[test]
    fn rewrite_only_when_enabled_and_different() {
        let want = r#""C:\a\yubi.exe" --background"#;
        assert!(needs_rewrite(true, None, want));
        assert!(needs_rewrite(
            true,
            Some(r#""C:\b\yubi.exe" --background"#),
            want
        ));
        assert!(!needs_rewrite(true, Some(want), want));
        assert!(!needs_rewrite(true, Some(&want.to_uppercase()), want));
        assert!(!needs_rewrite(false, None, want));
        assert!(!needs_rewrite(false, Some("other"), want));
    }
}
