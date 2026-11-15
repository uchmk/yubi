//! Notification-area icon on its own thread: a hidden top-level window (not message-only, because
//! `TaskbarCreated` is only broadcast to top-level windows) with a message loop.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use eframe::egui;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject, HGDIOBJ};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey,
};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR, w};

use super::Event;

/// Window class of the tray window; the second launch finds the first through it.
const CLASS: PCWSTR = w!("YubiTrayWindow");
/// Tray icon callback.
const WM_TRAY: u32 = WM_APP + 1;
/// Ask the running instance to show its window (posted by a second launch).
pub const WM_SHOW_MAIN: u32 = WM_APP + 2;
const WM_REFRESH: u32 = WM_APP + 3;
const WM_BALLOON: u32 = WM_APP + 4;
const WM_TOAST: u32 = WM_APP + 5;
const WM_SET_SHORTCUT: u32 = WM_APP + 6;
/// Id of the pause shortcut among this window's hot keys.
const HOTKEY_PAUSE: i32 = 1;
/// Retry timer for `NIM_ADD` (Explorer may not be up yet at sign-in): 1 s, up to 30 tries.
const RETRY_TIMER: usize = 1;
const RETRY_MAX: u32 = 30;
static RETRIES: AtomicU32 = AtomicU32::new(0);

const ID_OPEN: usize = 1;
const ID_TOGGLE: usize = 2;
const ID_QUIT: usize = 3;
const ID_PAUSE_15: usize = 4;
const ID_PAUSE_60: usize = 5;
/// The timed pauses of the tray menu, in minutes.
const PAUSE_MINUTES: [(usize, u32); 2] = [(ID_PAUSE_15, 15), (ID_PAUSE_60, 60)];

/// Window class winit gives the main window. Matched by class, not title: `GetWindowText` on a
/// window of another thread of this process sends a message and could deadlock.
const MAIN_CLASS: &str = "Window Class";
/// How long the tray thread waits for the app to finish its exit path on logoff/shutdown.
const END_SESSION_WAIT: Duration = Duration::from_secs(2);

pub struct Shared {
    pub ctx: egui::Context,
    pub tx: Sender<Event>,
    /// Nothing is recognized right now (pause or master switch): icon and tooltip.
    pub paused: AtomicBool,
    /// The runtime pause is on: menu text.
    pub pause_on: AtomicBool,
    pub toast: Mutex<Option<String>>,
    /// The wanted pause shortcut (a key combo string, empty = none).
    pub shortcut: Mutex<String>,
    /// A `ShortcutStatus` as a number.
    pub shortcut_status: AtomicU8,
    pub hwnd: AtomicIsize,
    /// The notification-area icon is in place.
    pub icon_ok: AtomicBool,
    pub balloon: Mutex<Option<String>>,
    /// Written only after the balloon was really shown.
    pub hint_marker: Option<PathBuf>,
    /// Set by the app once it has saved and stopped the engine for a session end.
    pub end_ack: (Mutex<bool>, Condvar),
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// The two icons of the process (`HICON` as isize: running, paused), each created on first use.
static ICONS: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];

pub fn install(shared: Arc<Shared>) {
    let _ = SHARED.set(shared);
}

fn shared() -> Option<&'static Arc<Shared>> {
    SHARED.get()
}

fn tray_hwnd() -> Option<HWND> {
    let h = shared()?.hwnd.load(Ordering::SeqCst);
    (h != 0).then_some(HWND(h as *mut _))
}

fn post(msg: u32) {
    if let Some(h) = tray_hwnd() {
        unsafe {
            let _ = PostMessageW(Some(h), msg, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn refresh() {
    post(WM_REFRESH);
}

pub fn balloon() {
    post(WM_BALLOON);
}

pub fn toast() {
    post(WM_TOAST);
}

/// Ask the tray thread to (re)register the shortcut stored in `Shared`.
pub fn apply_shortcut_later() {
    post(WM_SET_SHORTCUT);
}

pub fn close() {
    post(WM_CLOSE);
}

/// Second launch: find the first instance's tray window and ask it to show the main window.
pub fn signal_existing() -> bool {
    for _ in 0..30 {
        unsafe {
            let mut prev = None;
            while let Ok(h) = FindWindowExW(None, prev, CLASS, PCWSTR::null()) {
                prev = Some(h);
                // The class name alone is not proof: make sure the owner is also a yubi exe.
                if !same_exe_as_ours(h) {
                    continue;
                }
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = PostMessageW(Some(h), WM_SHOW_MAIN, WPARAM(0), LPARAM(0));
                return true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

/// Does the process owning `h` run an exe with the same file name as this one?
fn same_exe_as_ours(h: HWND) -> bool {
    let Some(ours) = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_ascii_lowercase()))
    else {
        return false;
    };
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .is_ok();
        let _ = CloseHandle(proc);
        ok && std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))
            .file_name()
            .is_some_and(|n| n.to_ascii_lowercase() == ours)
    }
}

/// Create the window and icon, then run the message loop until the window is destroyed.
pub fn run(ready: Sender<bool>) {
    unsafe {
        let Some(hwnd) = create_window() else {
            let _ = ready.send(false);
            return;
        };
        if let Some(s) = shared() {
            s.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
        }
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::SeqCst,
        );
        // The window exists, so a second launch can reach us even if the icon cannot be added yet.
        if !add_icon(hwnd) {
            SetTimer(Some(hwnd), RETRY_TIMER, 1000, None);
        }
        let _ = ready.send(true);
        let mut msg = MSG::default();
        // 0 is WM_QUIT, -1 is an error: either way fall through to the cleanup.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // WM_DESTROY removes the icon and the icon handle; make sure it ran.
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
    }
}

unsafe fn create_window() -> Option<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None).ok()?.into();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return None;
        }
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CLASS,
            w!("yubi-tray"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        )
        .ok()
    }
}

fn is_paused() -> bool {
    shared().is_some_and(|s| s.paused.load(Ordering::Relaxed))
}

fn tip() -> &'static str {
    if is_paused() {
        "yubi（一時停止中）"
    } else {
        "yubi（動作中）"
    }
}

fn fill(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for u in s.encode_utf16().take(dst.len() - 1) {
        dst[n] = u;
        n += 1;
    }
    dst[n] = 0;
}

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

unsafe fn add_icon(hwnd: HWND) -> bool {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon(is_paused());
    fill(&mut nid.szTip, tip());
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid).as_bool() };
    if let Some(s) = shared() {
        s.icon_ok.store(ok, Ordering::SeqCst);
    }
    ok
}

unsafe fn update_tip(hwnd: HWND) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_TIP | NIF_ICON;
    nid.hIcon = icon(is_paused());
    fill(&mut nid.szTip, tip());
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

unsafe fn show_balloon(hwnd: HWND) {
    let Some(text) = shared().and_then(|s| s.balloon.lock().ok()?.take()) else {
        return;
    };
    let mut nid = base(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    fill(&mut nid.szInfoTitle, "yubi");
    fill(&mut nid.szInfo, &text);
    let shown = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() };
    if shown && let Some(marker) = shared().and_then(|s| s.hint_marker.as_ref()) {
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(marker, "shown\n");
    }
}

/// Show the queued toast text as a silent balloon (Windows 11 presents it as a toast).
unsafe fn show_toast(hwnd: HWND) {
    let Some(text) = shared().and_then(|s| s.toast.lock().ok()?.take()) else {
        return;
    };
    let mut nid = base(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO | NIIF_NOSOUND;
    fill(&mut nid.szInfoTitle, "yubi");
    fill(&mut nid.szInfo, &text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Modifier flags and virtual key of a shortcut text; `None` unless it has Ctrl or Alt and a key
/// (a bare or Shift-only key would be taken from every app, like the capture field requires).
fn hotkey_parts(text: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    use crate::os::keys::{KeyCombo, VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT};
    let combo: KeyCombo = text.parse().ok()?;
    if !combo
        .modifiers
        .iter()
        .any(|m| *m == VK_CONTROL || *m == VK_MENU)
    {
        return None;
    }
    let mut mods = MOD_NOREPEAT;
    for m in &combo.modifiers {
        mods |= match *m {
            VK_CONTROL => MOD_CONTROL,
            VK_MENU => MOD_ALT,
            VK_SHIFT => MOD_SHIFT,
            VK_LWIN => MOD_WIN,
            _ => return None,
        };
    }
    Some((mods, u32::from(combo.key.0)))
}

/// (Re)register the pause shortcut and publish how that went.
unsafe fn apply_shortcut(hwnd: HWND) {
    let Some(s) = shared() else {
        return;
    };
    let text = s.shortcut.lock().map(|t| t.clone()).unwrap_or_default();
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_PAUSE);
    }
    let status = if text.trim().is_empty() {
        0
    } else if let Some((mods, vk)) = hotkey_parts(&text) {
        // Fails when another app already owns the combination.
        if unsafe { RegisterHotKey(Some(hwnd), HOTKEY_PAUSE, mods, vk).is_ok() } {
            1
        } else {
            2
        }
    } else {
        3
    };
    s.shortcut_status.store(status, Ordering::SeqCst);
    s.ctx.request_repaint();
}

fn icon(paused: bool) -> HICON {
    let slot = &ICONS[usize::from(paused)];
    let cur = slot.load(Ordering::SeqCst);
    if cur != 0 {
        return HICON(cur as *mut _);
    }
    let h = make_icon(paused);
    slot.store(h.0 as isize, Ordering::SeqCst);
    h
}

fn destroy_icon() {
    for slot in &ICONS {
        let cur = slot.swap(0, Ordering::SeqCst);
        if cur != 0 {
            unsafe {
                let _ = DestroyIcon(HICON(cur as *mut _));
            }
        }
    }
}

/// Disc colour (B, G, R) of the icon: blue while running, grey while paused.
fn disc_bgr(paused: bool) -> (f32, f32, f32) {
    if paused {
        (128.0, 128.0, 128.0)
    } else {
        (212.0, 120.0, 0.0)
    }
}

/// A generated 32x32 icon: a blue (grey when paused) disc with a white ring (a fingertip),
/// anti-aliased.
fn make_icon(paused: bool) -> HICON {
    const N: i32 = 32;
    let mut px = vec![0u8; (N * N * 4) as usize];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let disc = (15.5 - d + 0.5).clamp(0.0, 1.0);
            let ring = (9.5 - d + 0.5).clamp(0.0, 1.0) - (6.5 - d + 0.5).clamp(0.0, 1.0);
            // White over blue (BGRA, straight alpha).
            let (db, dg, dr) = disc_bgr(paused);
            let b = db * (1.0 - ring) + 255.0 * ring;
            let g = dg * (1.0 - ring) + 255.0 * ring;
            let r = dr * (1.0 - ring) + 255.0 * ring;
            let i = ((y * N + x) * 4) as usize;
            px[i] = b as u8;
            px[i + 1] = g as u8;
            px[i + 2] = r as u8;
            px[i + 3] = (disc * 255.0) as u8;
        }
    }
    let mask = vec![0u8; (N * 4) as usize];
    unsafe {
        let color = CreateBitmap(N, N, 1, 32, Some(px.as_ptr().cast()));
        let m = CreateBitmap(N, N, 1, 1, Some(mask.as_ptr().cast()));
        let info = ICONINFO {
            fIcon: BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: m,
            hbmColor: color,
        };
        // Fall back to the stock application icon rather than a blank tray slot.
        let icon = CreateIconIndirect(&info)
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default();
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(m.0));
        icon
    }
}

unsafe extern "system" fn enum_cb(h: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            let mut buf = [0u16; 32];
            let n = GetClassNameW(h, &mut buf).max(0) as usize;
            if String::from_utf16_lossy(&buf[..n]) == MAIN_CLASS {
                *(lparam.0 as *mut isize) = h.0 as isize;
                return BOOL(0);
            }
        }
    }
    BOOL(1)
}

fn find_main() -> Option<HWND> {
    let mut found: isize = 0;
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut found as *mut isize as isize));
    }
    (found != 0).then_some(HWND(found as *mut _))
}

/// Show and focus the settings window, directly through Win32 (a hidden eframe window may not
/// run its update loop) and through egui so its own visibility state stays in step.
fn show_main() {
    if let Some(h) = find_main() {
        unsafe {
            // Async: ShowWindow on another thread's window waits for that thread, which may be
            // waiting for this one (exit path).
            if IsIconic(h).as_bool() {
                let _ = ShowWindowAsync(h, SW_RESTORE);
            } else {
                let _ = ShowWindowAsync(h, SW_SHOW);
            }
            let _ = SetForegroundWindow(h);
        }
    }
    if let Some(s) = shared() {
        s.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
        s.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        s.ctx.request_repaint();
    }
}

fn send(ev: Event) {
    if let Some(s) = shared() {
        let _ = s.tx.send(ev);
        s.ctx.request_repaint();
    }
}

unsafe fn context_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let paused = shared().is_some_and(|s| s.pause_on.load(Ordering::Relaxed));
        let toggle = HSTRING::from(if paused { "再開" } else { "一時停止" });
        let open = HSTRING::from("設定を開く");
        let quit = HSTRING::from("終了");
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN, &open);
        let _ = AppendMenuW(menu, MF_STRING, ID_TOGGLE, &toggle);
        if !paused {
            for (id, minutes) in PAUSE_MINUTES {
                let text = HSTRING::from(format!("一時停止（{minutes} 分後に再開）"));
                let _ = AppendMenuW(menu, MF_STRING, id, &text);
            }
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, &quit);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            ID_OPEN => show_main(),
            ID_TOGGLE => send(Event::TogglePause),
            ID_QUIT => send(Event::Quit),
            id => {
                if let Some((_, minutes)) = PAUSE_MINUTES.iter().find(|(i, _)| *i == id) {
                    send(Event::PauseForMinutes(*minutes));
                }
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => match (lparam.0 as u32) & 0xFFFF {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => show_main(),
            WM_RBUTTONUP => unsafe { context_menu(hwnd) },
            _ => {}
        },
        WM_SHOW_MAIN => show_main(),
        WM_TIMER if wparam.0 == RETRY_TIMER => unsafe {
            let tries = RETRIES.fetch_add(1, Ordering::SeqCst) + 1;
            if add_icon(hwnd) || tries >= RETRY_MAX {
                let _ = KillTimer(Some(hwnd), RETRY_TIMER);
            }
        },
        WM_REFRESH => unsafe { update_tip(hwnd) },
        WM_BALLOON => unsafe { show_balloon(hwnd) },
        WM_TOAST => unsafe { show_toast(hwnd) },
        WM_SET_SHORTCUT => unsafe { apply_shortcut(hwnd) },
        WM_HOTKEY if wparam.0 == HOTKEY_PAUSE as usize => send(Event::TogglePause),
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                end_session();
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            unsafe {
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_PAUSE);
            }
            if let Some(s) = shared() {
                s.hwnd.store(0, Ordering::SeqCst);
            }
            let nid = base(hwnd);
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                PostQuitMessage(0);
            }
            destroy_icon();
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => unsafe {
            add_icon(hwnd);
        },
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Logoff/shutdown: winit does not handle `WM_ENDSESSION`, so the app would be killed with held
/// keys and an unsaved config. Ask the app to run its exit path and wait a bounded time for it.
fn end_session() {
    let Some(s) = shared() else {
        return;
    };
    let _ = s.tx.send(Event::EndSession);
    s.ctx.request_repaint();
    let (lock, cv) = &s.end_ack;
    if let Ok(done) = lock.lock() {
        let _ = cv.wait_timeout_while(done, END_SESSION_WAIT, |d| !*d);
    }
}

/// The app finished its end-of-session work.
pub fn ack_end_session() {
    if let Some(s) = shared() {
        let (lock, cv) = &s.end_ack;
        if let Ok(mut d) = lock.lock() {
            *d = true;
        }
        cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_text_maps_to_modifiers_and_key() {
        let (mods, vk) = hotkey_parts("Ctrl+Alt+Y").unwrap();
        assert_eq!(mods, MOD_NOREPEAT | MOD_CONTROL | MOD_ALT);
        assert_eq!(vk, u32::from(b'Y'));
        let (mods, vk) = hotkey_parts("Ctrl+Win+Shift+F5").unwrap();
        assert_eq!(mods, MOD_NOREPEAT | MOD_CONTROL | MOD_WIN | MOD_SHIFT);
        assert_eq!(vk, 0x74);
    }

    #[test]
    fn a_bare_key_or_nonsense_is_not_a_shortcut() {
        assert!(hotkey_parts("Y").is_none());
        assert!(hotkey_parts("F5").is_none());
        assert!(hotkey_parts("Shift+Y").is_none());
        assert!(hotkey_parts("Win+Shift+F5").is_none());
        assert!(hotkey_parts("").is_none());
        assert!(hotkey_parts("Ctrl+Alt+").is_none());
        assert!(hotkey_parts("Ctrl+NoSuchKey").is_none());
    }

    #[test]
    fn the_paused_icon_is_grey_and_the_running_one_blue() {
        let (b, g, r) = disc_bgr(true);
        assert!(b == g && g == r);
        let (b, _, r) = disc_bgr(false);
        assert!(b > r);
    }
}
