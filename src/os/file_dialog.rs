//! Native Open / Save dialogs (`IFileOpenDialog` / `IFileSaveDialog`) for settings import/export.
//!
//! Blocking: call from the UI thread (or a dedicated thread), never from the engine thread.
//! `Ok(None)` means the user cancelled. COM is initialised once per thread, apartment-threaded
//! (dialogs need an STA); a thread already in another mode is tolerated.
#![allow(dead_code)]

use std::cell::Cell;
use std::path::PathBuf;

use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FileOpenDialog, FileSaveDialog,
    IFileDialog, IFileOpenDialog, IFileSaveDialog, SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, PCWSTR, Result, w};

/// HRESULT_FROM_WIN32(ERROR_CANCELLED).
const CANCELLED: HRESULT = HRESULT(0x8007_04C7_u32 as i32);

thread_local! {
    static COM_INIT: Cell<bool> = const { Cell::new(false) };
}

fn init_com_once() {
    COM_INIT.with(|done| {
        if !done.get() {
            // S_FALSE / RPC_E_CHANGED_MODE just mean COM is already set up on this thread.
            // SAFETY: plain call.
            let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            done.set(true);
        }
    });
}

fn configure(
    dlg: &IFileDialog,
    title: PCWSTR,
    extra: windows::Win32::UI::Shell::FILEOPENDIALOGOPTIONS,
) -> Result<()> {
    let filters = [
        COMDLG_FILTERSPEC {
            pszName: w!("yubi settings (*.toml)"),
            pszSpec: w!("*.toml"),
        },
        COMDLG_FILTERSPEC {
            pszName: w!("All files (*.*)"),
            pszSpec: w!("*.*"),
        },
    ];
    // SAFETY: valid interface; the filter strings are 'static.
    unsafe {
        dlg.SetFileTypes(&filters)?;
        dlg.SetDefaultExtension(w!("toml"))?;
        dlg.SetTitle(title)?;
        let opts = dlg.GetOptions()?;
        dlg.SetOptions(opts | FOS_FORCEFILESYSTEM | extra)?;
    }
    Ok(())
}

fn result_path(dlg: &IFileDialog) -> Result<Option<PathBuf>> {
    // SAFETY: the dialog returned success; the display name is freed with CoTaskMemFree.
    unsafe {
        let item = dlg.GetResult()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let s = name.to_string();
        CoTaskMemFree(Some(name.0 as *const _));
        let s = s.map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
        Ok(Some(PathBuf::from(s)))
    }
}

fn show(dlg: &IFileDialog) -> Result<Option<PathBuf>> {
    // SAFETY: valid interface; no owner window.
    match unsafe { dlg.Show(None) } {
        Ok(()) => result_path(dlg),
        Err(e) if e.code() == CANCELLED => Ok(None),
        Err(e) => Err(e),
    }
}

/// "Save as" dialog proposing `default_name` (for example `yubi-settings.toml`).
pub fn save_toml(default_name: &str) -> Result<Option<PathBuf>> {
    init_com_once();
    // SAFETY: standard COM activation.
    unsafe {
        let dlg: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL)?;
        let base: IFileDialog = windows::core::Interface::cast(&dlg)?;
        configure(&base, w!("Export settings"), FOS_OVERWRITEPROMPT)?;
        let name: Vec<u16> = default_name.encode_utf16().chain(Some(0)).collect();
        base.SetFileName(PCWSTR(name.as_ptr()))?;
        show(&base)
    }
}

/// "Open" dialog for an existing `.toml` file.
pub fn open_toml() -> Result<Option<PathBuf>> {
    init_com_once();
    // SAFETY: standard COM activation.
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL)?;
        let base: IFileDialog = windows::core::Interface::cast(&dlg)?;
        configure(&base, w!("Import settings"), FOS_FILEMUSTEXIST)?;
        show(&base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_code() {
        assert_eq!(CANCELLED.0 as u32, 0x8007_04C7);
    }

    /// Manual: `cargo test -- --ignored dialogs_manual --nocapture`. Shows both dialogs; cancel them.
    #[test]
    #[ignore]
    fn dialogs_manual() {
        println!("save: {:?}", save_toml("yubi-settings.toml"));
        println!("open: {:?}", open_toml());
    }
}
