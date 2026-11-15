//! Apps as the ring shows and starts them: the app's own icon (cached per path), its file
//! description for a label, and the launch itself.
//!
//! The shell calls run on short-lived threads with their own COM apartment, and nothing here
//! waits for them: `icon_for` and `label_for` answer from the cache at once (an uncached app
//! shows a generic glyph and its file name, and the lookup starts; the next ring has the icon),
//! and `prefetch` warms the cache ahead of time. Only `shell_open` is a plain blocking call, to be
//! run on a thread of its own.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

/// The side of the bitmap asked from the shell (the overlay scales it down).
const ICON_SIDE: i32 = 48;
/// A failed lookup is tried again after this long (the app may be installed meanwhile).
const RETRY_AFTER: Duration = Duration::from_secs(30);
const MAX_ENTRIES: usize = 64;

/// A square premultiplied BGRA picture.
#[derive(Clone, Debug)]
pub struct AppIcon {
    pub side: u32,
    pub px: Arc<Vec<u32>>,
}

#[derive(Clone)]
struct Found {
    icon: Option<AppIcon>,
    description: Option<String>,
}

enum State {
    Loading,
    Done(Found, Instant),
}

static CACHE: Mutex<Option<HashMap<String, State>>> = Mutex::new(None);

fn key_of(path: &str) -> String {
    path.trim().trim_matches('"').to_lowercase()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Start the lookup of `path` on its own thread unless it is cached or running.
fn begin(path: &str) {
    let key = key_of(path);
    {
        let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let map = g.get_or_insert_with(HashMap::new);
        match map.get(&key) {
            Some(State::Loading) => return,
            Some(State::Done(f, at)) if f.icon.is_some() || at.elapsed() < RETRY_AFTER => return,
            _ => {}
        }
        if map.len() >= MAX_ENTRIES {
            // Drop the oldest finished entries (a quarter of the cache), not everything.
            let mut done: Vec<(Instant, String)> = map
                .iter()
                .filter_map(|(k, v)| match v {
                    State::Done(_, at) => Some((*at, k.clone())),
                    State::Loading => None,
                })
                .collect();
            done.sort();
            for (_, k) in done.into_iter().take(MAX_ENTRIES / 4) {
                map.remove(&k);
            }
        }
        map.insert(key.clone(), State::Loading);
    }
    let path = path.trim().trim_matches('"').to_owned();
    let spawned = std::thread::Builder::new()
        .name("yubi-appicon".into())
        .spawn({
            let key = key.clone();
            move || {
                let found = lookup(&path);
                let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
                g.get_or_insert_with(HashMap::new)
                    .insert(key, State::Done(found, Instant::now()));
            }
        });
    if spawned.is_err() {
        let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashMap::new).remove(&key);
    }
}

/// What is known of `path` right now, waiting up to `wait` for a lookup in flight.
fn found(path: &str, wait: Duration) -> Option<Found> {
    begin(path);
    let key = key_of(path);
    let deadline = Instant::now() + wait;
    loop {
        {
            let g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            match g.as_ref().and_then(|m| m.get(&key)) {
                Some(State::Done(f, _)) => return Some(f.clone()),
                Some(State::Loading) => {}
                None => return None,
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

/// Warm the cache for these paths (config load, settings change).
pub fn prefetch<'a>(paths: impl IntoIterator<Item = &'a str>) {
    for p in paths {
        if !p.trim().is_empty() {
            begin(p);
        }
    }
}

/// The app's icon, from the cache only; the first request starts the lookup and gets `None`.
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "only the overlay path, which tests stub out, uses it"
    )
)]
pub fn icon_for(path: &str) -> Option<AppIcon> {
    found(path, Duration::ZERO)?.icon
}

/// The app's file description ("Visual Studio Code"), when it is cached and it has one.
pub fn description_for(path: &str) -> Option<String> {
    found(path, Duration::ZERO)?.description
}

/// The label a ring shows for `path`: the description, else the file name without extension.
pub fn label_for(path: &str) -> String {
    pick_label(description_for(path), path)
}

fn pick_label(description: Option<String>, path: &str) -> String {
    description
        .map(|d| d.trim().to_owned())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| crate::config::file_stem_of(path))
}

fn lookup(path: &str) -> Found {
    // A shell call needs a COM apartment of its own on this short-lived thread.
    // SAFETY: balanced with `CoUninitialize` below when it succeeded.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
    let found = Found {
        icon: extract_icon(path),
        description: file_description(path),
    };
    if com {
        // SAFETY: matches the successful `CoInitializeEx` above.
        unsafe { CoUninitialize() };
    }
    found
}

/// Premultiplied BGRA pixels of the app's icon via `IShellItemImageFactory`.
fn extract_icon(path: &str) -> Option<AppIcon> {
    let name = wide(path);
    // SAFETY: standard shell item creation; the bitmap and the DCs are released below.
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(name.as_ptr()), None).ok()?;
        let bmp = factory
            .GetImage(
                SIZE {
                    cx: ICON_SIDE,
                    cy: ICON_SIDE,
                },
                SIIGBF_ICONONLY,
            )
            .ok()?;
        let mut info = BITMAP::default();
        let got = GetObjectW(
            bmp.into(),
            size_of::<BITMAP>() as i32,
            Some(&mut info as *mut BITMAP as *mut c_void),
        );
        let (w, h) = (info.bmWidth, info.bmHeight.abs());
        let mut out = None;
        if got != 0 && w > 0 && w == h {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut px = vec![0u32; (w * h) as usize];
            let lines = GetDIBits(
                dc,
                bmp,
                0,
                h as u32,
                Some(px.as_mut_ptr() as *mut c_void),
                &mut bmi,
                DIB_RGB_COLORS,
            );
            if lines != 0 {
                fix_alpha(&mut px);
                out = Some(AppIcon {
                    side: w as u32,
                    px: Arc::new(px),
                });
            }
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
        }
        let _ = DeleteObject(bmp.into());
        out
    }
}

/// A bitmap without an alpha channel comes back with alpha 0 everywhere: make it opaque.
fn fix_alpha(px: &mut [u32]) {
    if px.iter().all(|p| p >> 24 == 0) {
        for p in px.iter_mut() {
            *p |= 0xFF00_0000;
        }
    }
}

/// `FileDescription` of an executable's version resource.
fn file_description(path: &str) -> Option<String> {
    if !path.trim().to_lowercase().ends_with(".exe") {
        return None;
    }
    let name = wide(path.trim().trim_matches('"'));
    // SAFETY: the buffer is sized by the first call; the query results point into it while it
    // lives.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(name.as_ptr()), None);
        if size == 0 {
            return None;
        }
        // u32 elements keep the block aligned for the u16 reads below.
        let mut buf = vec![0u32; (size as usize).div_ceil(4)];
        GetFileVersionInfoW(
            PCWSTR(name.as_ptr()),
            None,
            size,
            buf.as_mut_ptr() as *mut c_void,
        )
        .ok()?;
        let query = |sub: &str| -> Option<(*const c_void, u32)> {
            let sub = wide(sub);
            let mut p: *mut c_void = std::ptr::null_mut();
            let mut len = 0u32;
            let ok = VerQueryValueW(
                buf.as_ptr() as *const c_void,
                PCWSTR(sub.as_ptr()),
                &mut p,
                &mut len,
            );
            (ok.as_bool() && !p.is_null() && len > 0).then_some((p as *const c_void, len))
        };
        // The first (language, code page) pair, else English / Unicode and Japanese.
        let mut langs: Vec<String> = Vec::new();
        if let Some((p, len)) = query("\\VarFileInfo\\Translation")
            && len >= 4
        {
            let pair = std::slice::from_raw_parts(p as *const u16, 2);
            langs.push(format!("{:04x}{:04x}", pair[0], pair[1]));
        }
        langs.extend(["040904b0".into(), "041104b0".into(), "040904e4".into()]);
        for lang in langs {
            if let Some((p, len)) = query(&format!("\\StringFileInfo\\{lang}\\FileDescription")) {
                let s = std::slice::from_raw_parts(p as *const u16, len as usize);
                let text = String::from_utf16_lossy(s);
                let text = text.trim_end_matches('\0').trim().to_owned();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
}

/// Start `path` (an exe, a shortcut or anything the shell can open) with `args`, in the app's own
/// folder. We open no window of our own; a console app gets its console as it would from Explorer.
/// Blocks for the shell call: run it on a thread. Returns whether the shell accepted it.
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "only the overlay path, which tests stub out, uses it"
    )
)]
pub fn shell_open(path: &str, args: &str) -> bool {
    let file = wide(path.trim().trim_matches('"'));
    let params = (!args.trim().is_empty()).then(|| wide(args.trim()));
    // Run from the app's own folder, like Explorer does.
    let dir = std::path::Path::new(path.trim().trim_matches('"'))
        .parent()
        .filter(|p| p.is_dir())
        .map(|p| wide(&p.to_string_lossy()));
    // SAFETY: balanced with `CoUninitialize`; every string outlives the call.
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let r = ShellExecuteW(
            None,
            windows::core::w!("open"),
            PCWSTR(file.as_ptr()),
            params
                .as_ref()
                .map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
            dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            SW_SHOWNORMAL,
        );
        if com {
            CoUninitialize();
        }
        // ShellExecute reports success with a value above 32.
        r.0 as usize > 32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_prefer_the_description() {
        assert_eq!(
            pick_label(Some(" Visual Studio Code ".into()), r"C:\x\Code.exe"),
            "Visual Studio Code"
        );
        assert_eq!(pick_label(None, r"C:\x\Code.exe"), "Code");
        assert_eq!(pick_label(Some("  ".into()), r"C:\x\Code.exe"), "Code");
        assert_eq!(pick_label(None, r"C:\Menu\メモ帳.lnk"), "メモ帳");
    }

    #[test]
    fn opaque_bitmaps_get_an_alpha() {
        let mut px = vec![0x0010_2030u32; 4];
        fix_alpha(&mut px);
        assert!(px.iter().all(|p| p >> 24 == 0xFF));
        // A bitmap that has alpha is left alone.
        let mut px = vec![0x8010_2030u32, 0, 0, 0];
        fix_alpha(&mut px);
        assert_eq!(px[0], 0x8010_2030);
        assert_eq!(px[1], 0);
    }

    #[test]
    fn keys_ignore_case_and_quotes() {
        assert_eq!(key_of(r#" "C:\A\B.EXE" "#), r"c:\a\b.exe");
    }

    #[test]
    fn an_uncached_path_answers_at_once() {
        let t = Instant::now();
        let path = r"C:\yubi-test\never-cached-app.exe";
        assert!(icon_for(path).is_none());
        // The label falls back to the file name without waiting.
        assert_eq!(label_for(path), "never-cached-app");
        assert!(t.elapsed() < Duration::from_millis(50), "{:?}", t.elapsed());
    }

    #[test]
    fn own_exe_has_an_icon_and_a_label() {
        let exe = std::env::current_exe().expect("exe");
        let path = exe.to_string_lossy().into_owned();
        // The first lookup runs on its own thread; give it time instead of the short wait.
        let f = found(&path, Duration::from_secs(10)).expect("found");
        let icon = f.icon.expect("icon");
        assert!(icon.side >= 16 && icon.px.len() == (icon.side * icon.side) as usize);
        assert!(
            icon.px.iter().any(|p| p >> 24 != 0),
            "not fully transparent"
        );
        assert!(!label_for(&path).is_empty());
        // A missing file has no icon, and still a label (its name).
        let none = found(r"C:\no\such\app.exe", Duration::from_secs(10)).expect("found");
        assert!(none.icon.is_none());
    }
}
