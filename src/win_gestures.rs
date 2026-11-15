//! Windows' own 3- and 4-finger touchpad gestures, which compete with yubi's.
//!
//! The values live under `HKCU\Software\Microsoft\Windows\CurrentVersion\PrecisionTouchPad`.
//! A missing value means "Windows default" (enabled). Nothing here runs by itself: the 全般 page
//! reads the values, and only a button press backs them up (including "missing") and writes 0,
//! or restores exactly what was backed up. The planning is pure so it can be tested.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const NAMES: [&str; 4] = [
    "ThreeFingerSlideEnabled",
    "ThreeFingerTapEnabled",
    "FourFingerSlideEnabled",
    "FourFingerTapEnabled",
];

/// Japanese label for each entry of [`NAMES`].
pub const LABELS: [&str; 4] = [
    "3本指のスワイプ",
    "3本指のタップ",
    "4本指のスワイプ",
    "4本指のタップ",
];

pub const BACKUP_FILE: &str = "windows-gestures-backup.toml";

/// Current values in [`NAMES`] order; `None` = the value does not exist (Windows default).
pub type Values = [Option<u32>; 4];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegOp {
    Set(&'static str, u32),
    Delete(&'static str),
}

/// Exact copy of what was there before yubi changed anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Backup {
    /// Values that existed.
    #[serde(default)]
    pub present: BTreeMap<String, u32>,
    /// Values that did not exist.
    #[serde(default)]
    pub missing: Vec<String>,
}

pub fn backup_of(values: &Values) -> Backup {
    let mut b = Backup::default();
    for (name, v) in NAMES.iter().zip(values) {
        match v {
            Some(v) => {
                b.present.insert((*name).to_owned(), *v);
            }
            None => b.missing.push((*name).to_owned()),
        }
    }
    b
}

/// Writes needed to switch every gesture off (values already 0 are left alone).
pub fn plan_disable(values: &Values) -> Vec<RegOp> {
    NAMES
        .iter()
        .zip(values)
        .filter(|(_, v)| **v != Some(0))
        .map(|(n, _)| RegOp::Set(n, 0))
        .collect()
}

/// Writes that bring the registry back to `backup`: existing values are set, missing ones deleted.
pub fn plan_restore(backup: &Backup) -> Vec<RegOp> {
    NAMES
        .iter()
        .filter_map(|name| {
            if let Some(v) = backup.present.get(*name) {
                Some(RegOp::Set(name, *v))
            } else if backup.missing.iter().any(|m| m == name) {
                Some(RegOp::Delete(name))
            } else {
                None
            }
        })
        .collect()
}

/// True when yubi's gestures would still be fighting Windows (any value on or missing).
pub fn any_enabled(values: &Values) -> bool {
    values.iter().any(|v| *v != Some(0))
}

/// How a value reads on the page.
pub fn describe(v: Option<u32>) -> &'static str {
    match v {
        None => "既定（有効）",
        Some(0) => "無効",
        Some(_) => "有効",
    }
}

/// Write the backup only if no file exists yet. Returns whether it was written.
/// The text goes to a temp file in the same folder, is flushed to disk, then renamed into place,
/// so a crash never leaves a half-written backup behind.
pub fn write_backup_new(path: &Path, backup: &Backup) -> io::Result<bool> {
    let text = toml::to_string_pretty(backup).map_err(io::Error::other)?;
    if path.exists() {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if path.exists() {
        let _ = std::fs::remove_file(&tmp);
        return Ok(false);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(true)
}

pub fn read_backup(path: &Path) -> io::Result<Backup> {
    let text = std::fs::read_to_string(path)?;
    toml::from_str(&text).map_err(io::Error::other)
}

/// Back up (never overwriting an existing backup), then switch the gestures off.
/// Nothing is written to the registry unless the values were read and the backup is usable.
pub fn disable(backup_path: &Path) -> Result<(), String> {
    disable_with(backup_path, read_values, apply)
}

fn disable_with(
    backup_path: &Path,
    read: impl FnOnce() -> io::Result<Values>,
    write: impl FnOnce(&[RegOp]) -> io::Result<()>,
) -> Result<(), String> {
    let values = read().map_err(|e| format!("レジストリを読めません: {e}"))?;
    if backup_path.exists() {
        read_backup(backup_path).map_err(|e| {
            format!("既存のバックアップを読めないため、設定を変更しませんでした: {e}")
        })?;
    } else {
        write_backup_new(backup_path, &backup_of(&values))
            .map_err(|e| format!("バックアップを保存できません: {e}"))?;
    }
    write(&plan_disable(&values)).map_err(|e| format!("レジストリに書き込めません: {e}"))
}

/// Put the registry back exactly as the backup says, then move the used backup aside so the next
/// "disable" takes a fresh one.
pub fn restore(backup_path: &Path) -> Result<(), String> {
    restore_with(backup_path, apply)
}

fn restore_with(
    backup_path: &Path,
    write: impl FnOnce(&[RegOp]) -> io::Result<()>,
) -> Result<(), String> {
    let backup = read_backup(backup_path).map_err(|e| format!("バックアップを読めません: {e}"))?;
    write(&plan_restore(&backup)).map_err(|e| format!("レジストリに書き込めません: {e}"))?;
    move_backup_aside(backup_path)
        .map_err(|e| format!("元に戻しましたが、使用済みバックアップを移せません: {e}"))
}

fn move_backup_aside(path: &Path) -> io::Result<()> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("backup");
    let mut n = 0u32;
    loop {
        let name = if n == 0 {
            format!("{stem}.restored-{secs}.toml")
        } else {
            format!("{stem}.restored-{secs}-{n}.toml")
        };
        let dest = path.with_file_name(name);
        if !dest.exists() {
            return std::fs::rename(path, dest);
        }
        n += 1;
    }
}

#[cfg(windows)]
pub use reg::{apply, read_values};

#[cfg(windows)]
mod reg {
    use std::io;

    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
    use windows::Win32::System::Registry::*;
    use windows::core::{HSTRING, PCWSTR, w};

    use super::{NAMES, RegOp, Values};

    const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\PrecisionTouchPad");

    fn check(e: WIN32_ERROR) -> io::Result<()> {
        if e == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(e.0 as i32))
        }
    }

    pub fn read_values() -> io::Result<Values> {
        let mut key = HKEY::default();
        let e = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, KEY, None, KEY_QUERY_VALUE, &mut key) };
        if e == ERROR_FILE_NOT_FOUND {
            return Ok([None; 4]);
        }
        check(e)?;
        let mut out: Values = [None; 4];
        for (slot, name) in out.iter_mut().zip(NAMES) {
            let mut ty = REG_VALUE_TYPE(0);
            let mut data = 0u32;
            let mut len = size_of::<u32>() as u32;
            let e = unsafe {
                RegQueryValueExW(
                    key,
                    &HSTRING::from(name),
                    None,
                    Some(&mut ty),
                    Some((&mut data as *mut u32).cast()),
                    Some(&mut len),
                )
            };
            if e == ERROR_FILE_NOT_FOUND {
                continue;
            }
            if e != ERROR_SUCCESS {
                unsafe {
                    let _ = RegCloseKey(key);
                }
                return Err(io::Error::other(format!(
                    "{name} を読めません（エラー {}）",
                    e.0
                )));
            }
            if ty != REG_DWORD {
                unsafe {
                    let _ = RegCloseKey(key);
                }
                return Err(io::Error::other(format!(
                    "{name} の型が想定（DWORD）と違います"
                )));
            }
            *slot = Some(data);
        }
        unsafe {
            let _ = RegCloseKey(key);
        }
        Ok(out)
    }

    pub fn apply(ops: &[RegOp]) -> io::Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
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
        let mut result = Ok(());
        for op in ops {
            let r = match *op {
                RegOp::Set(name, v) => check(unsafe {
                    RegSetValueExW(
                        key,
                        &HSTRING::from(name),
                        None,
                        REG_DWORD,
                        Some(&v.to_le_bytes()),
                    )
                }),
                RegOp::Delete(name) => {
                    let e = unsafe { RegDeleteValueW(key, &HSTRING::from(name)) };
                    if e == ERROR_FILE_NOT_FOUND {
                        Ok(())
                    } else {
                        check(e)
                    }
                }
            };
            if r.is_err() && result.is_ok() {
                result = r;
            }
        }
        unsafe {
            let _ = RegCloseKey(key);
        }
        result
    }
}

#[cfg(not(windows))]
pub fn read_values() -> io::Result<Values> {
    Ok([None; 4])
}

#[cfg(not(windows))]
pub fn apply(_ops: &[RegOp]) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_skips_values_already_zero() {
        let v: Values = [Some(1), None, Some(0), Some(5)];
        assert_eq!(
            plan_disable(&v),
            vec![
                RegOp::Set("ThreeFingerSlideEnabled", 0),
                RegOp::Set("ThreeFingerTapEnabled", 0),
                RegOp::Set("FourFingerTapEnabled", 0),
            ]
        );
        assert!(plan_disable(&[Some(0); 4]).is_empty());
    }

    #[test]
    fn backup_keeps_missing_distinct_from_zero() {
        let v: Values = [Some(1), None, Some(0), None];
        let b = backup_of(&v);
        assert_eq!(b.present.get("FourFingerSlideEnabled"), Some(&0));
        assert_eq!(
            b.missing,
            vec!["ThreeFingerTapEnabled", "FourFingerTapEnabled"]
        );
    }

    #[test]
    fn restore_returns_exactly_the_original() {
        let v: Values = [Some(1), None, Some(0), None];
        assert_eq!(
            plan_restore(&backup_of(&v)),
            vec![
                RegOp::Set("ThreeFingerSlideEnabled", 1),
                RegOp::Delete("ThreeFingerTapEnabled"),
                RegOp::Set("FourFingerSlideEnabled", 0),
                RegOp::Delete("FourFingerTapEnabled"),
            ]
        );
        assert!(plan_restore(&Backup::default()).is_empty());
    }

    #[test]
    fn backup_survives_toml_and_is_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("yubi-test-{}", std::process::id()));
        let path = dir.join(BACKUP_FILE);
        let _ = std::fs::remove_file(&path);
        let first = backup_of(&[Some(1), None, Some(0), Some(1)]);
        assert!(write_backup_new(&path, &first).unwrap());
        // A second backup (taken after yubi wrote zeros) must not replace the original.
        let second = backup_of(&[Some(0); 4]);
        assert!(!write_backup_new(&path, &second).unwrap());
        assert_eq!(read_backup(&path).unwrap(), first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("yubi-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn restore_moves_backup_aside() {
        let dir = temp_dir("restore");
        let path = dir.join(BACKUP_FILE);
        write_backup_new(&path, &backup_of(&[Some(1), None, Some(0), Some(1)])).unwrap();
        restore_with(&path, |_| Ok(())).unwrap();
        assert!(!path.exists());
        let moved: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(moved.len(), 1);
        assert!(moved[0].starts_with("windows-gestures-backup.restored-"));
        // The next disable takes a fresh backup.
        assert!(write_backup_new(&path, &backup_of(&[Some(0); 4])).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_restore_keeps_backup() {
        let dir = temp_dir("restore-fail");
        let path = dir.join(BACKUP_FILE);
        write_backup_new(&path, &backup_of(&[Some(1); 4])).unwrap();
        assert!(restore_with(&path, |_| Err(io::Error::other("x"))).is_err());
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disable_aborts_before_writing() {
        let dir = temp_dir("disable");
        let path = dir.join(BACKUP_FILE);
        let mut wrote = false;
        // Read failure: no backup, no write.
        let r = disable_with(
            &path,
            || Err(io::Error::other("x")),
            |_| {
                wrote = true;
                Ok(())
            },
        );
        assert!(r.is_err() && !wrote && !path.exists());
        // Unparsable existing backup: no write.
        std::fs::write(&path, "not = [valid").unwrap();
        let r = disable_with(
            &path,
            || Ok([Some(1); 4]),
            |_| {
                wrote = true;
                Ok(())
            },
        );
        assert!(r.is_err() && !wrote);
        // Good case writes and backs up.
        std::fs::remove_file(&path).unwrap();
        disable_with(
            &path,
            || Ok([Some(1); 4]),
            |_| {
                wrote = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(wrote && path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn describes_values() {
        assert_eq!(describe(None), "既定（有効）");
        assert_eq!(describe(Some(0)), "無効");
        assert_eq!(describe(Some(3)), "有効");
        assert!(any_enabled(&[Some(0), None, Some(0), Some(0)]));
        assert!(!any_enabled(&[Some(0); 4]));
    }
}
