//! Executable names of the apps that currently have a visible window, for the per-app picker.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextLengthW, IsWindowVisible};
use windows::core::BOOL;

use super::process::{exe_name_of_pid, exe_path_of_pid, pid_of};
use super::window::Hwnd;

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<String>` passed by `running_exe_names`, alive for the call.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<String>) };
    // SAFETY: both calls accept any window handle.
    let shown = unsafe { IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 };
    if shown
        && let Some(name) = Hwnd::from_win(hwnd)
            .and_then(pid_of)
            .and_then(exe_name_of_pid)
    {
        out.push(name);
    }
    BOOL(1)
}

/// Lowercase exe names of the apps with a visible, titled top-level window; sorted, no repeats.
pub fn running_exe_names() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    // SAFETY: the callback only touches the vector behind the pointer, which outlives the call.
    let _ = unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut names as *mut Vec<String> as isize),
        )
    };
    names.sort();
    names.dedup();
    names
}

unsafe extern "system" fn collect_paths(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<String>` passed by `running_exe_paths`, alive for the call.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<String>) };
    // SAFETY: both calls accept any window handle.
    let shown = unsafe { IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 };
    if shown
        && let Some(path) = Hwnd::from_win(hwnd)
            .and_then(pid_of)
            .and_then(exe_path_of_pid)
    {
        out.push(path);
    }
    BOOL(1)
}

/// Full paths of the apps with a visible, titled top-level window; sorted by file name, no
/// repeats. For the "launch an app" picker.
pub fn running_exe_paths() -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    // SAFETY: the callback only touches the vector behind the pointer, which outlives the call.
    let _ = unsafe {
        EnumWindows(
            Some(collect_paths),
            LPARAM(&mut paths as *mut Vec<String> as isize),
        )
    };
    let key = |p: &String| p.rsplit(['\\', '/']).next().unwrap_or(p).to_lowercase();
    paths.sort_by_key(|p| (key(p), p.to_lowercase()));
    paths.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    paths
}
