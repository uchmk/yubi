//! Top-level window lookup, moving / resizing / snapping, elevation check.
//!
//! Coordinates are physical screen pixels. The process must be per-monitor DPI
//! aware, otherwise Windows virtualises the values. The yubi app gets this from winit
//! (`EventLoop::new` calls `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`, checked against
//! winit 0.30); a binary or test that does not create a winit event loop is DPI-unaware and sees
//! virtualised values unless it sets the awareness itself.
//!
//! Threading: every function here may be called from any thread (no thread affinity), but one
//! `WindowMover` must be driven from one thread at a time. Nothing waits on the target window's
//! message loop except two 50 ms `SendMessageTimeoutW` calls: `WindowMover::begin`
//! (`WM_GETMINMAXINFO`) and `caption_at` (`WM_NCHITTEST`); restores and moves are posted asynchronously (`ShowWindowAsync`,
//! `SWP_ASYNCWINDOWPOS`), so a hung target cannot block the caller.
//!
//! Which rectangle? `SetWindowPos` works in `GetWindowRect` space, which on
//! Windows 10/11 includes invisible resize borders (about 7 px left/right/
//! bottom). The *visible* frame is `DWMWA_EXTENDED_FRAME_BOUNDS`. Moving and
//! resizing by deltas uses `GetWindowRect` (insets do not matter for deltas);
//! `place` takes a rect in visible-frame terms and compensates by the insets,
//! so snapped windows touch the work-area edge exactly.

#![allow(
    dead_code,
    reason = "corner resize, work-area helpers and placement arrive in M3b/M3c"
)]

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow,
};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_UNAWARE, GetAwarenessFromDpiAwarenessContext, GetDpiForMonitor, GetDpiForWindow,
    GetWindowDpiAwarenessContext, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_LEFT, VK_RIGHT, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetClassNameW, GetCursorPos, GetDesktopWindow, GetForegroundWindow,
    GetShellWindow, GetWindowPlacement, GetWindowRect, GetWindowThreadProcessId, HTCAPTION,
    IsWindow, IsZoomed, MINMAXINFO, SMTO_ABORTIFHUNG, SMTO_BLOCK, SW_MAXIMIZE, SW_RESTORE,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SendMessageTimeoutW,
    SetForegroundWindow, SetWindowPos, ShowWindowAsync, WINDOWPLACEMENT, WM_GETMINMAXINFO,
    WM_NCHITTEST, WindowFromPoint,
};

use super::keys::{self, KeyCombo, VK_LWIN, Vk};

/// A window handle as a plain integer (Send + Copy, no raw pointers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hwnd(pub isize);

impl Hwnd {
    pub fn to_win(self) -> HWND {
        HWND(self.0 as *mut c_void)
    }

    pub fn from_win(h: HWND) -> Option<Self> {
        (!h.0.is_null()).then_some(Self(h.0 as isize))
    }

    pub fn is_valid(self) -> bool {
        // SAFETY: IsWindow accepts any handle value.
        unsafe { IsWindow(Some(self.to_win())).as_bool() }
    }
}

/// Pixel rectangle (`right`/`bottom` exclusive, like `RECT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    fn from_win(r: RECT) -> Self {
        Self::new(r.left, r.top, r.right, r.bottom)
    }

    /// Sub-rectangle given as fractions of `self` (e.g. left half = 0,0,0.5,1).
    pub fn fraction(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        let (ww, hh) = (self.width() as f32, self.height() as f32);
        let l = self.left + (x * ww).round() as i32;
        let t = self.top + (y * hh).round() as i32;
        Rect::new(
            l,
            t,
            self.left + ((x + w) * ww).round() as i32,
            self.top + ((y + h) * hh).round() as i32,
        )
    }
}

/// Scale a pixel value between DPI settings (96 = 100 %).
pub fn scale_px(value: i32, from_dpi: u32, to_dpi: u32) -> i32 {
    if from_dpi == 0 {
        return value;
    }
    (value as i64 * to_dpi as i64 / from_dpi as i64) as i32
}

/// DPI of the monitor `hwnd` is on (96 if unknown).
pub fn dpi_of(hwnd: Hwnd) -> u32 {
    // SAFETY: any handle value is accepted; 0 is returned on failure.
    let d = unsafe { GetDpiForWindow(hwnd.to_win()) };
    if d == 0 { 96 } else { d }
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: buffer is valid for its length.
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Desktop, shell (wallpaper) and taskbar windows must never be moved.
fn is_shell_class(name: &str) -> bool {
    matches!(
        name,
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "SysListView32"
    )
}

fn accept_top_level(root: HWND) -> Option<Hwnd> {
    if root.0.is_null() {
        return None;
    }
    // SAFETY: plain queries.
    unsafe {
        if root == GetDesktopWindow() || root == GetShellWindow() {
            return None;
        }
    }
    if is_shell_class(&class_name(root)) {
        return None;
    }
    Hwnd::from_win(root)
}

/// Top-level window under the screen point (physical px), skipping the
/// desktop, wallpaper host and taskbars.
pub fn top_level_at(point: (i32, i32)) -> Option<Hwnd> {
    // SAFETY: plain queries.
    let root = unsafe {
        let h = WindowFromPoint(POINT {
            x: point.0,
            y: point.1,
        });
        if h.0.is_null() {
            return None;
        }
        GetAncestor(h, GA_ROOT)
    };
    accept_top_level(root)
}

/// Whether the point (physical screen pixels) is on a window's title bar: the top-level window
/// there answers `WM_NCHITTEST` with `HTCAPTION`. Any failure (no window, hung window, timeout)
/// is `false`.
pub fn caption_at(x: i32, y: i32) -> bool {
    // SAFETY: plain queries; the message carries no pointers and the call times out.
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.0.is_null() {
            return false;
        }
        let root = GetAncestor(h, GA_ROOT);
        if root.0.is_null() {
            return false;
        }
        // yubi's own windows (settings, overlays) are not something to flick away from.
        let mut pid = 0u32;
        GetWindowThreadProcessId(root, Some(&mut pid));
        if pid == std::process::id() {
            return false;
        }
        let lparam = LPARAM(((y as i16 as u16 as isize) << 16) | (x as i16 as u16 as isize));
        let mut result = 0usize;
        let sent = SendMessageTimeoutW(
            root,
            WM_NCHITTEST,
            WPARAM(0),
            lparam,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            50,
            Some(&mut result),
        );
        sent.0 != 0 && result == HTCAPTION as usize
    }
}

/// Cursor position in physical screen pixels.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY: valid out pointer.
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

/// The foreground window's root, with the same exclusions as `top_level_at`.
pub fn foreground() -> Option<Hwnd> {
    // SAFETY: plain queries.
    let root = unsafe {
        let h = GetForegroundWindow();
        if h.0.is_null() {
            return None;
        }
        GetAncestor(h, GA_ROOT)
    };
    accept_top_level(root)
}

fn window_rect(hwnd: Hwnd) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: valid out pointer.
    unsafe { GetWindowRect(hwnd.to_win(), &mut r).ok()? };
    Some(Rect::from_win(r))
}

/// Visible frame (DWM extended frame bounds), falling back to `GetWindowRect`.
pub fn visible_rect(hwnd: Hwnd) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: out buffer is exactly one RECT.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd.to_win(),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
    };
    if ok {
        Some(Rect::from_win(r))
    } else {
        window_rect(hwnd)
    }
}

/// Invisible-border insets: how far the visible frame lies inside the
/// `GetWindowRect` rectangle (left, top, right, bottom; all >= 0).
fn frame_insets(hwnd: Hwnd) -> (i32, i32, i32, i32) {
    match (window_rect(hwnd), visible_rect(hwnd)) {
        (Some(w), Some(v)) => (
            (v.left - w.left).max(0),
            (v.top - w.top).max(0),
            (w.right - v.right).max(0),
            (w.bottom - v.bottom).max(0),
        ),
        _ => (0, 0, 0, 0),
    }
}

/// Work area (monitor minus taskbar) of the monitor nearest to `hwnd`.
pub fn work_area_of(hwnd: Hwnd) -> Option<Rect> {
    // SAFETY: MONITORINFO.cbSize is set as required.
    unsafe {
        let m = MonitorFromWindow(hwnd.to_win(), MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(m, &mut mi)
            .as_bool()
            .then(|| Rect::from_win(mi.rcWork))
    }
}

fn is_maximized(hwnd: Hwnd) -> bool {
    // SAFETY: plain query.
    unsafe { IsZoomed(hwnd.to_win()).as_bool() }
}

/// Ask the window to restore itself. Asynchronous: the window state does not change before this
/// returns, so callers must not read the new geometry back (see `restored_rect`).
fn restore(hwnd: Hwnd) {
    // SAFETY: plain call; posts the request instead of waiting for the target.
    unsafe {
        let _ = ShowWindowAsync(hwnd.to_win(), SW_RESTORE);
    }
}

/// Rectangle (`GetWindowRect` space, screen px) the window will have once restored, from its
/// `WINDOWPLACEMENT::rcNormalPosition`. That rectangle is in workspace coordinates (relative to
/// the work area), so the work-area offset of the nearest monitor is added back.
fn restored_rect(hwnd: Hwnd) -> Option<Rect> {
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: `wp.length` is set; valid out pointer.
    unsafe { GetWindowPlacement(hwnd.to_win(), &mut wp).ok()? };
    let n = wp.rcNormalPosition;
    let (ox, oy) = unsafe {
        let m = MonitorFromWindow(hwnd.to_win(), MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(m, &mut mi).as_bool() {
            (
                mi.rcWork.left - mi.rcMonitor.left,
                mi.rcWork.top - mi.rcMonitor.top,
            )
        } else {
            (0, 0)
        }
    };
    Some(Rect::new(
        n.left + ox,
        n.top + oy,
        n.right + ox,
        n.bottom + oy,
    ))
}

/// Which corner of the window is being dragged in `resize_from`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Fallback minimum size (logical px at 96 DPI) when the window does not state one.
pub const MIN_SIZE_FLOOR: (i32, i32) = (200, 150);

/// `MIN_SIZE_FLOOR` in physical pixels at `dpi`.
pub fn scaled_floor(dpi: u32) -> (i32, i32) {
    let dpi = if dpi == 0 { 96 } else { dpi };
    (
        scale_px(MIN_SIZE_FLOOR.0, 96, dpi),
        scale_px(MIN_SIZE_FLOOR.1, 96, dpi),
    )
}

/// Effective DPI of the monitor `hwnd` is on (the real scale, also for DPI-unaware windows,
/// for which `GetDpiForWindow` reports 96).
fn monitor_dpi(hwnd: Hwnd) -> u32 {
    // SAFETY: plain queries with valid out pointers.
    unsafe {
        let m = MonitorFromWindow(hwnd.to_win(), MONITOR_DEFAULTTONEAREST);
        let (mut x, mut y) = (0u32, 0u32);
        if GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_ok() && x != 0 {
            return x;
        }
    }
    dpi_of(hwnd)
}

/// True if the target process treats itself as DPI-unaware (Windows virtualises its coordinates).
fn is_dpi_unaware(hwnd: Hwnd) -> bool {
    // SAFETY: plain queries.
    unsafe {
        let ctx = GetWindowDpiAwarenessContext(hwnd.to_win());
        GetAwarenessFromDpiAwarenessContext(ctx) == DPI_AWARENESS_UNAWARE
    }
}

/// Pure resize math: move `corner` of `r` by (dx, dy), keeping the opposite
/// corner fixed and the size within `min`..`max`.
pub fn resize_rect(
    r: Rect,
    corner: Corner,
    dx: i32,
    dy: i32,
    min: (i32, i32),
    max: (i32, i32),
) -> Rect {
    let (hx, hy) = match corner {
        Corner::TopLeft => (-1, -1),
        Corner::TopRight => (1, -1),
        Corner::BottomLeft => (-1, 1),
        Corner::BottomRight => (1, 1),
    };
    resize_rect_by(r, (hx, hy), dx, dy, min, max)
}

/// Which handle of a window a point picks on the 3x3 grid laid over it: each axis is -1 (left /
/// top third), 0 (middle) or 1 (right / bottom third). (0, 0) is the centre: a move.
pub fn grid_handle(r: Rect, point: (i32, i32)) -> (i8, i8) {
    let axis = |p: i32, lo: i32, size: i32| -> i8 {
        let size = size.max(1);
        let third = ((p - lo) * 3 / size).clamp(0, 2);
        (third - 1) as i8
    };
    (
        axis(point.0, r.left, r.width()),
        axis(point.1, r.top, r.height()),
    )
}

/// Resize math for any handle: an axis of -1 moves the left / top edge, 1 the right / bottom
/// edge and 0 leaves that axis alone. The size stays within `min`..`max`.
pub fn resize_rect_by(
    r: Rect,
    handle: (i8, i8),
    dx: i32,
    dy: i32,
    min: (i32, i32),
    max: (i32, i32),
) -> Rect {
    let mut o = r;
    match handle.0 {
        -1 => o.left = (r.left + dx).clamp(r.right - max.0, r.right - min.0),
        1 => o.right = (r.right + dx).clamp(r.left + min.0, r.left + max.0),
        _ => {}
    }
    match handle.1 {
        -1 => o.top = (r.top + dy).clamp(r.bottom - max.1, r.bottom - min.1),
        1 => o.bottom = (r.bottom + dy).clamp(r.top + min.1, r.top + max.1),
        _ => {}
    }
    o
}

/// (min, max) track size in physical screen pixels. `WM_GETMINMAXINFO` answers in the *target's*
/// coordinate space: physical px when the target is per-monitor aware, but 96-DPI logical px when
/// it is DPI-unaware, so those values are scaled by the monitor DPI. The minimum is raised to
/// `scaled_floor` of the target's monitor DPI.
fn track_limits(hwnd: Hwnd) -> ((i32, i32), (i32, i32)) {
    let dpi = monitor_dpi(hwnd);
    let floor = scaled_floor(dpi);
    let mut mmi = MINMAXINFO {
        ptMaxTrackSize: POINT { x: 16384, y: 16384 },
        ..Default::default()
    };
    // SAFETY: WM_GETMINMAXINFO is marshalled across processes by the system;
    // timeout guards against hung windows.
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd.to_win(),
            WM_GETMINMAXINFO,
            WPARAM(0),
            LPARAM(&mut mmi as *mut MINMAXINFO as isize),
            SMTO_ABORTIFHUNG,
            50,
            None,
        )
        .0 != 0
    };
    let (mut min, mut max) = (floor, (16384, 16384));
    if ok {
        let k = if is_dpi_unaware(hwnd) { dpi } else { 96 };
        let sc = |v: i32| scale_px(v, 96, k);
        min = (
            sc(mmi.ptMinTrackSize.x).max(floor.0),
            sc(mmi.ptMinTrackSize.y).max(floor.1),
        );
        let (mx, my) = (sc(mmi.ptMaxTrackSize.x), sc(mmi.ptMaxTrackSize.y));
        if mx >= min.0 && my >= min.1 {
            max = (mx, my);
        }
    }
    (min, max)
}

/// An in-progress interactive move / resize of one window. All position
/// updates are asynchronous (`SWP_ASYNCWINDOWPOS`), so the mover tracks the
/// rectangle itself instead of reading it back.
pub struct WindowMover {
    hwnd: Hwnd,
    /// The rectangle when the interaction began (the base of a zoom).
    base: Rect,
    rect: Rect,
    min: (i32, i32),
    max: (i32, i32),
    /// A maximised window of a zoom is restored only by the first zoom step that changes its
    /// size, so a pinch that ends at ratio 1 leaves it maximised.
    pending_restore: bool,
}

impl WindowMover {
    /// Start moving `hwnd`. `cursor` is the cursor position in screen px; if
    /// the window is maximized it is restored (asynchronously) and repositioned so the cursor
    /// keeps its relative place on the title bar; the restored size comes from
    /// `GetWindowPlacement`, not from reading the window back. Returns `None` if the window is
    /// gone or unreadable. Waits at most 50 ms for the target (`WM_GETMINMAXINFO`).
    pub fn begin(hwnd: Hwnd, cursor: (i32, i32)) -> Option<Self> {
        let mut before = window_rect(hwnd)?;
        if is_maximized(hwnd) {
            let restored = restored_rect(hwnd)?;
            restore(hwnd);
            let frac = if before.width() > 0 {
                (cursor.0 - before.left) as f32 / before.width() as f32
            } else {
                0.5
            };
            let new_left =
                cursor.0 - (frac.clamp(0.0, 1.0) * restored.width() as f32).round() as i32;
            let dy = (cursor.1 - before.top).clamp(0, restored.height());
            let new_top = cursor.1 - dy;
            // SAFETY: posted after the restore request, so it is handled after it by the same
            // window thread. Size is passed explicitly because the restore is not done yet.
            unsafe {
                SetWindowPos(
                    hwnd.to_win(),
                    None,
                    new_left,
                    new_top,
                    restored.width(),
                    restored.height(),
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                )
                .ok()?;
            }
            before = Rect::new(
                new_left,
                new_top,
                new_left + restored.width(),
                new_top + restored.height(),
            );
        }
        let (min, max) = track_limits(hwnd);
        Some(Self {
            hwnd,
            base: before,
            rect: before,
            min,
            max,
            pending_restore: false,
        })
    }

    /// Start a zoom of `hwnd`. Unlike [`Self::begin`] a maximised window stays maximised until
    /// the first step that actually changes its size.
    pub fn begin_zoom(hwnd: Hwnd, cursor: (i32, i32)) -> Option<Self> {
        if !is_maximized(hwnd) {
            return Self::begin(hwnd, cursor);
        }
        let base = restored_rect(hwnd)?;
        let (min, max) = track_limits(hwnd);
        Some(Self {
            hwnd,
            base,
            rect: base,
            min,
            max,
            pending_restore: true,
        })
    }

    pub fn hwnd(&self) -> Hwnd {
        self.hwnd
    }

    /// Current tracked rectangle (`GetWindowRect` space).
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Move by (dx, dy) physical pixels relative to the tracked position.
    pub fn move_by(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        self.rect = Rect::new(
            self.rect.left + dx,
            self.rect.top + dy,
            self.rect.right + dx,
            self.rect.bottom + dy,
        );
        // SAFETY: plain call; failures (window closed) are ignored.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd.to_win(),
                None,
                self.rect.left,
                self.rect.top,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
        }
    }

    /// Drag `corner` by (dx, dy), clamped to the window's min/max track size
    /// (or a 200x150 floor).
    pub fn resize_from(&mut self, corner: Corner, dx: i32, dy: i32) {
        let next = resize_rect(self.rect, corner, dx, dy, self.min, self.max);
        if next == self.rect {
            return;
        }
        self.rect = next;
        // SAFETY: plain call; failures are ignored.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd.to_win(),
                None,
                next.left,
                next.top,
                next.width(),
                next.height(),
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
        }
    }

    /// Resize by a grid handle (see [`grid_handle`]); the centre handle moves the window.
    pub fn resize_handle(&mut self, handle: (i8, i8), dx: i32, dy: i32) {
        if handle == (0, 0) {
            self.move_by(dx, dy);
            return;
        }
        let next = resize_rect_by(self.rect, handle, dx, dy, self.min, self.max);
        if next == self.rect {
            return;
        }
        self.rect = next;
        // SAFETY: plain call; failures are ignored.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd.to_win(),
                None,
                next.left,
                next.top,
                next.width(),
                next.height(),
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
        }
    }

    /// Scale the window to `factor` times its size at the start, around its centre, kept inside
    /// the work area of its monitor and within its size limits.
    pub fn zoom_to(&mut self, factor: f32) {
        let bounds = zoom_bounds(self.hwnd);
        let next = zoom_rect(self.base, factor, self.min, self.max, bounds);
        if self.pending_restore {
            if next == self.base {
                return;
            }
            restore(self.hwnd);
            self.pending_restore = false;
        } else if next == self.rect {
            return;
        }
        self.rect = next;
        // SAFETY: plain call; failures are ignored.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd.to_win(),
                None,
                next.left,
                next.top,
                next.width(),
                next.height(),
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
        }
    }

    /// Finish the interaction (nothing is pending to flush; consumes self).
    pub fn end(self) {}
}

/// The work area of the window's monitor in `GetWindowRect` space: grown by the invisible
/// borders, so the visible frame can reach the work-area edge.
fn zoom_bounds(hwnd: Hwnd) -> Rect {
    let work = work_area_of(hwnd).unwrap_or(Rect::new(0, 0, 16384, 16384));
    let (il, it, ir, ib) = frame_insets(hwnd);
    Rect::new(
        work.left - il,
        work.top - it,
        work.right + ir,
        work.bottom + ib,
    )
}

/// Pure zoom math: `base` scaled by `factor` around its centre. The factor is limited so that
/// both sides stay within `min`..`max` and the window fits `bounds`; the result is then moved
/// inside `bounds`. The minimum wins over the other limits.
pub fn zoom_rect(base: Rect, factor: f32, min: (i32, i32), max: (i32, i32), bounds: Rect) -> Rect {
    let (bw, bh) = (base.width().max(1) as f32, base.height().max(1) as f32);
    // The limits are relative to the base: a factor of 1 is always allowed, so a window that is
    // already beyond a limit (bigger than the work area, smaller than its minimum) does not jump
    // at the first step.
    let lo = (min.0 as f32 / bw).max(min.1 as f32 / bh).min(1.0);
    let hi = (max.0 as f32 / bw)
        .min(max.1 as f32 / bh)
        .min(bounds.width() as f32 / bw)
        .min(bounds.height() as f32 / bh)
        .max(1.0);
    let f = if factor.is_finite() { factor } else { 1.0 }.clamp(lo, hi);
    if f == 1.0 {
        return base;
    }
    let w = ((bw * f).round() as i32).max(1);
    let h = ((bh * f).round() as i32).max(1);
    // Twice the centre, so an odd size keeps it exact.
    let (cx2, cy2) = (base.left + base.right, base.top + base.bottom);
    let left = (cx2 - w).div_euclid(2);
    let top = (cy2 - h).div_euclid(2);
    let left = left.clamp(bounds.left, (bounds.right - w).max(bounds.left));
    let top = top.clamp(bounds.top, (bounds.bottom - h).max(bounds.top));
    Rect::new(left, top, left + w, top + h)
}

/// Direction for `snap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapDir {
    Left,
    Right,
    /// Maximize.
    Up,
    /// Restore, or minimize if already restored.
    Down,
}

impl SnapDir {
    fn vk(self) -> Vk {
        Vk(match self {
            SnapDir::Left => VK_LEFT.0,
            SnapDir::Right => VK_RIGHT.0,
            SnapDir::Up => VK_UP.0,
            SnapDir::Down => VK_DOWN.0,
        })
    }
}

/// Snap with the OS's own behaviour: bring `hwnd` to the foreground, then
/// inject Win+Arrow. Returns false if the window could not be activated or the
/// keys were not accepted. Skip elevated windows first (`is_elevated_window`).
pub fn snap(hwnd: Hwnd, dir: SnapDir) -> bool {
    // SAFETY: plain call; may be refused by the foreground lock.
    unsafe {
        if GetForegroundWindow() != hwnd.to_win() && !SetForegroundWindow(hwnd.to_win()).as_bool() {
            return false;
        }
    }
    keys::send_combo(&KeyCombo::new([VK_LWIN], dir.vk()))
}

/// Insets assumed for a maximized window, whose real insets cannot be read before the
/// asynchronous restore has happened: the usual 7 px side/bottom resize border at 96 DPI.
fn default_insets(dpi: u32) -> (i32, i32, i32, i32) {
    let b = scale_px(7, 96, dpi);
    (b, 0, b, b)
}

/// Place the window so its *visible* frame is exactly `rect` (restoring it first if maximized).
/// Asynchronous (`ShowWindowAsync` + `SWP_ASYNCWINDOWPOS`): returns true when the request was
/// posted, and the window moves a moment later. For a maximized window the invisible-border
/// insets cannot be measured yet, so DPI-scaled defaults are used.
pub fn place(hwnd: Hwnd, rect: Rect) -> bool {
    let (il, it, ir, ib) = if is_maximized(hwnd) {
        restore(hwnd);
        default_insets(monitor_dpi(hwnd))
    } else {
        frame_insets(hwnd)
    };
    let x = rect.left - il;
    let y = rect.top - it;
    let w = rect.width() + il + ir;
    let h = rect.height() + it + ib;
    // SAFETY: plain call.
    unsafe {
        SetWindowPos(
            hwnd.to_win(),
            None,
            x,
            y,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        )
        .is_ok()
    }
}

/// The rectangle a preset gives a window on `work` (visible-frame terms). `None` for the presets
/// that are not a plain rectangle (maximise, restore, next monitor).
pub fn preset_rect(work: Rect, preset: crate::config::WindowPreset) -> Option<Rect> {
    use crate::config::WindowPreset as P;
    let (w, h) = (work.width(), work.height());
    let (mx, my) = (work.left + w / 2, work.top + h / 2);
    Some(match preset {
        P::LeftHalf => Rect::new(work.left, work.top, mx, work.bottom),
        P::RightHalf => Rect::new(mx, work.top, work.right, work.bottom),
        P::TopHalf => Rect::new(work.left, work.top, work.right, my),
        P::BottomHalf => Rect::new(work.left, my, work.right, work.bottom),
        P::CenterTwoThirds => Rect::new(
            work.left + w / 6,
            work.top + h / 6,
            work.right - w / 6,
            work.bottom - h / 6,
        ),
        P::Maximize | P::Restore | P::NextMonitor => return None,
    })
}

/// `r` moved to the same relative place and share of `to` as it has on `from`, so a window on
/// the left half of one monitor lands on the left half of the next, whatever their sizes and
/// DPI. The result is kept inside `to`.
pub fn map_rect(r: Rect, from: Rect, to: Rect) -> Rect {
    let (fw, fh) = (from.width().max(1) as i64, from.height().max(1) as i64);
    let (tw, th) = (to.width() as i64, to.height() as i64);
    let map_x = |x: i32| to.left + ((x - from.left) as i64 * tw).div_euclid(fw) as i32;
    let map_y = |y: i32| to.top + ((y - from.top) as i64 * th).div_euclid(fh) as i32;
    let w = (r.width() as i64 * tw).div_euclid(fw).clamp(1, tw.max(1)) as i32;
    let h = (r.height() as i64 * th).div_euclid(fh).clamp(1, th.max(1)) as i32;
    let left = map_x(r.left).clamp(to.left, (to.right - w).max(to.left));
    let top = map_y(r.top).clamp(to.top, (to.bottom - h).max(to.top));
    Rect::new(left, top, left + w, top + h)
}

/// Index of the monitor after `current` in `works` ordered left to right, then top to bottom
/// (wrapping around). `None` with fewer than two monitors or when `current` is not among them.
pub fn next_monitor(works: &[Rect], current: Rect) -> Option<usize> {
    if works.len() < 2 {
        return None;
    }
    let mut order: Vec<usize> = (0..works.len()).collect();
    order.sort_by_key(|&i| (works[i].left, works[i].top));
    let at = order.iter().position(|&i| works[i] == current)?;
    Some(order[(at + 1) % order.len()])
}

unsafe extern "system" fn collect_work_area(
    m: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> windows::core::BOOL {
    // SAFETY: `data` is the `Vec<Rect>` that `monitor_work_areas` passes in, alive for the call.
    let list = unsafe { &mut *(data.0 as *mut Vec<Rect>) };
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: cbSize is set.
    if unsafe { GetMonitorInfoW(m, &mut mi) }.as_bool() {
        list.push(Rect::from_win(mi.rcWork));
    }
    true.into()
}

/// The work area of every monitor.
fn monitor_work_areas() -> Vec<Rect> {
    let mut list: Vec<Rect> = Vec::new();
    // SAFETY: the callback only runs during this call and gets a pointer to `list`.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect_work_area),
            LPARAM(&mut list as *mut Vec<Rect> as isize),
        );
    }
    list
}

/// What tells one window from another that reuses its handle: the process and the class.
type WinIdentity = (u32, String);

/// Where each window was before 「ウィンドウを並べる」 first moved it (visible-frame terms), for
/// 「元に戻す」, kept by window handle together with an identity so that a handle that was
/// reused by another window is not restored to a stranger's place.
#[derive(Default)]
struct SavedRects {
    map: std::collections::HashMap<isize, (Rect, WinIdentity)>,
}

impl SavedRects {
    /// Remember `rect` for `h` unless this very window already has one (the first position is
    /// the one 「元に戻す」 returns to).
    fn remember(&mut self, h: isize, id: WinIdentity, rect: Rect) {
        match self.map.get(&h) {
            Some((_, old)) if *old == id => {}
            _ => {
                self.map.insert(h, (rect, id));
            }
        }
    }

    /// Take the saved rect of `h`; an entry of a different window is dropped and gives nothing.
    fn take(&mut self, h: isize, id: &WinIdentity) -> Option<Rect> {
        let (rect, old) = self.map.remove(&h)?;
        (old == *id).then_some(rect)
    }

    fn contains(&self, h: isize) -> bool {
        self.map.contains_key(&h)
    }

    /// Forget the windows `alive` does not know any more.
    fn retain_alive(&mut self, alive: impl Fn(isize) -> bool) {
        self.map.retain(|h, _| alive(*h));
    }
}

fn saved_rects() -> std::sync::MutexGuard<'static, SavedRects> {
    use std::sync::{Mutex, OnceLock};
    static SAVED: OnceLock<Mutex<SavedRects>> = OnceLock::new();
    SAVED
        .get_or_init(|| Mutex::new(SavedRects::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn identity_of(hwnd: Hwnd) -> WinIdentity {
    let mut pid = 0u32;
    // SAFETY: valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd.to_win(), Some(&mut pid)) };
    (pid, class_name(hwnd.to_win()))
}

/// The visible frame the window has now, or the one it will have once restored if it is
/// maximised (taken from the placement, with the usual invisible borders taken off).
fn current_visible(hwnd: Hwnd) -> Option<Rect> {
    if is_maximized(hwnd) {
        let r = restored_rect(hwnd)?;
        let (il, it, ir, ib) = default_insets(monitor_dpi(hwnd));
        Some(Rect::new(
            r.left + il,
            r.top + it,
            r.right - ir,
            r.bottom - ib,
        ))
    } else {
        visible_rect(hwnd)
    }
}

/// Remember where `hwnd` is, unless it was already moved by a preset.
fn remember(hwnd: Hwnd) {
    let id = identity_of(hwnd);
    let mut saved = saved_rects();
    // Do not let handles of closed windows pile up.
    saved.retain_alive(|h| Hwnd(h).is_valid());
    if let Some(r) = current_visible(hwnd) {
        saved.remember(hwnd.0, id, r);
    }
}

/// Carry out a 「ウィンドウを並べる」 preset on `hwnd` (asynchronous, like `place`). The error is
/// a short Japanese text for the notes.
pub fn apply_preset(hwnd: Hwnd, preset: crate::config::WindowPreset) -> Result<(), String> {
    use crate::config::WindowPreset as P;
    const FAILED: &str = "ウィンドウを動かせませんでした";
    match preset {
        P::Restore => {
            let saved = saved_rects().take(hwnd.0, &identity_of(hwnd));
            if let Some(r) = saved {
                return if place(hwnd, r) {
                    Ok(())
                } else {
                    Err(FAILED.to_owned())
                };
            }
            if is_maximized(hwnd) {
                restore(hwnd);
                return Ok(());
            }
            Err("元に戻せる位置がありません".to_owned())
        }
        P::Maximize => {
            remember(hwnd);
            // SAFETY: plain call; posts the request instead of waiting for the target.
            let posted = unsafe { ShowWindowAsync(hwnd.to_win(), SW_MAXIMIZE).as_bool() };
            if posted {
                Ok(())
            } else {
                Err(FAILED.to_owned())
            }
        }
        P::NextMonitor => {
            let works = monitor_work_areas();
            let from = work_area_of(hwnd).ok_or_else(|| FAILED.to_owned())?;
            let Some(i) = next_monitor(&works, from) else {
                return Err("モニターが 1 つしかありません".to_owned());
            };
            let r = current_visible(hwnd).ok_or_else(|| FAILED.to_owned())?;
            let maximised = is_maximized(hwnd);
            let target = map_rect(r, from, works[i]);
            remember(hwnd);
            if !place(hwnd, target) {
                return Err(FAILED.to_owned());
            }
            if maximised {
                // SAFETY: as above; handled after the move by the same window thread.
                let posted = unsafe { ShowWindowAsync(hwnd.to_win(), SW_MAXIMIZE).as_bool() };
                if !posted {
                    return Err(FAILED.to_owned());
                }
            }
            Ok(())
        }
        rect => {
            let work = work_area_of(hwnd).ok_or_else(|| FAILED.to_owned())?;
            let target = preset_rect(work, rect).ok_or_else(|| FAILED.to_owned())?;
            remember(hwnd);
            if place(hwnd, target) {
                Ok(())
            } else {
                Err(FAILED.to_owned())
            }
        }
    }
}

fn integrity_of_process(pid: u32) -> Option<u32> {
    // SAFETY: handles are closed; buffer is sized from the first call.
    unsafe {
        let ph = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut th = Default::default();
        let r = OpenProcessToken(ph, TOKEN_QUERY, &mut th);
        let _ = CloseHandle(ph);
        r.ok()?;
        let level = integrity_of_token(th);
        let _ = CloseHandle(th);
        level
    }
}

/// # Safety
/// `th` must be a valid token handle with TOKEN_QUERY.
unsafe fn integrity_of_token(th: windows::Win32::Foundation::HANDLE) -> Option<u32> {
    unsafe {
        let mut needed = 0u32;
        let _ = GetTokenInformation(th, TokenIntegrityLevel, None, 0, &mut needed);
        if needed == 0 {
            return None;
        }
        // u64 buffer keeps the TOKEN_MANDATORY_LABEL aligned.
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        GetTokenInformation(
            th,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr() as *mut c_void),
            needed,
            &mut needed,
        )
        .ok()?;
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(sid, count as u32 - 1))
    }
}

/// Integrity RID of our own process (0x2000 medium, 0x3000 high ...).
fn own_integrity() -> u32 {
    use std::sync::OnceLock;
    static OWN: OnceLock<u32> = OnceLock::new();
    *OWN.get_or_init(|| {
        // SAFETY: pseudo handle needs no close.
        unsafe {
            let mut th = Default::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut th).is_err() {
                return 0x2000;
            }
            let l = integrity_of_token(th).unwrap_or(0x2000);
            let _ = CloseHandle(th);
            l
        }
    })
}

/// True if the window's process runs at a higher integrity level than ours
/// (UIPI would block moving it / injecting input into it). If the level cannot
/// be read at all (access denied) the window is assumed elevated.
pub fn is_elevated_window(hwnd: Hwnd) -> bool {
    let mut pid = 0u32;
    // SAFETY: valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd.to_win(), Some(&mut pid)) };
    if pid == 0 {
        return false;
    }
    match integrity_of_process(pid) {
        Some(theirs) => theirs > own_integrity(),
        None => true,
    }
}

/// Is a full-screen exclusive (Direct3D) app in front? Windows answers this for the shell's own
/// notification logic (`SHQueryUserNotificationState`, `QUNS_RUNNING_D3D_FULL_SCREEN` only).
/// Whether borderless full-screen games report this is up to Windows (not checked on hardware).
pub fn fullscreen_exclusive() -> bool {
    // SAFETY: plain query with no arguments.
    unsafe {
        windows::Win32::UI::Shell::SHQueryUserNotificationState()
            .is_ok_and(counts_as_fullscreen_exclusive)
    }
}

fn counts_as_fullscreen_exclusive(
    state: windows::Win32::UI::Shell::QUERY_USER_NOTIFICATION_STATE,
) -> bool {
    state == windows::Win32::UI::Shell::QUNS_RUNNING_D3D_FULL_SCREEN
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect {
        left: 100,
        top: 100,
        right: 500,
        bottom: 400,
    };
    const MIN: (i32, i32) = (200, 150);
    const MAX: (i32, i32) = (1000, 800);

    #[test]
    fn grid_picks_corner_edge_or_centre() {
        // R is 400 x 300: thirds at x 233 / 367 and y 200 / 300.
        assert_eq!(grid_handle(R, (110, 110)), (-1, -1));
        assert_eq!(grid_handle(R, (490, 110)), (1, -1));
        assert_eq!(grid_handle(R, (110, 390)), (-1, 1));
        assert_eq!(grid_handle(R, (490, 390)), (1, 1));
        assert_eq!(grid_handle(R, (300, 110)), (0, -1));
        assert_eq!(grid_handle(R, (300, 390)), (0, 1));
        assert_eq!(grid_handle(R, (110, 250)), (-1, 0));
        assert_eq!(grid_handle(R, (490, 250)), (1, 0));
        assert_eq!(grid_handle(R, (300, 250)), (0, 0));
        // Outside the window picks the nearest cell.
        assert_eq!(grid_handle(R, (-50, 9999)), (-1, 1));
    }

    #[test]
    fn handle_resize_moves_only_its_edges() {
        let right = resize_rect_by(R, (1, 0), 60, 99, MIN, MAX);
        assert_eq!(right, Rect::new(100, 100, 560, 400));
        let top = resize_rect_by(R, (0, -1), 99, -40, MIN, MAX);
        assert_eq!(top, Rect::new(100, 60, 500, 400));
        let corner = resize_rect_by(R, (-1, 1), -30, 20, MIN, MAX);
        assert_eq!(corner, Rect::new(70, 100, 500, 420));
        // The minimum holds from any handle, and the far edge stays put.
        let small = resize_rect_by(R, (-1, -1), 1000, 1000, MIN, MAX);
        assert_eq!((small.width(), small.height()), MIN);
        assert_eq!((small.right, small.bottom), (500, 400));
        assert_eq!(resize_rect_by(R, (0, 0), 10, 10, MIN, MAX), R);
    }

    #[test]
    fn resize_bottom_right_grows() {
        let o = resize_rect(R, Corner::BottomRight, 50, 30, MIN, MAX);
        assert_eq!(o, Rect::new(100, 100, 550, 430));
    }

    #[test]
    fn resize_top_left_keeps_far_corner() {
        let o = resize_rect(R, Corner::TopLeft, -40, -20, MIN, MAX);
        assert_eq!(o, Rect::new(60, 80, 500, 400));
    }

    #[test]
    fn resize_respects_min_and_max() {
        let o = resize_rect(R, Corner::BottomRight, -1000, -1000, MIN, MAX);
        assert_eq!((o.width(), o.height()), MIN);
        assert_eq!((o.left, o.top), (100, 100));
        let o = resize_rect(R, Corner::TopLeft, 1000, 1000, MIN, MAX);
        assert_eq!((o.width(), o.height()), MIN);
        assert_eq!((o.right, o.bottom), (500, 400));
        let o = resize_rect(R, Corner::TopRight, 5000, -5000, MIN, MAX);
        assert_eq!((o.width(), o.height()), MAX);
        assert_eq!((o.left, o.bottom), (100, 400));
    }

    #[test]
    fn zoom_scales_around_the_centre_within_limits() {
        let bounds = Rect::new(0, 0, 1920, 1040);
        let base = Rect::new(500, 300, 900, 600); // 400 x 300, centre (700, 450)
        let big = zoom_rect(base, 1.5, MIN, (4000, 4000), bounds);
        assert_eq!((big.width(), big.height()), (600, 450));
        assert_eq!((big.left + big.right, big.top + big.bottom), (1400, 900));
        // A limit that bites applies to both sides: the height reaches its minimum first.
        let small = zoom_rect(base, 0.1, MIN, (4000, 4000), bounds);
        assert_eq!((small.width(), small.height()), (200, 150));
        // Never beyond the work area, even from far out.
        let huge = zoom_rect(base, 50.0, MIN, (4000, 4000), bounds);
        assert_eq!(huge.height(), 1040);
        assert!(huge.left >= 0 && huge.right <= 1920 && huge.top >= 0 && huge.bottom <= 1040);
        // A window at the edge grows inward.
        let edge = Rect::new(1600, 0, 1920, 240);
        let grown = zoom_rect(edge, 1.5, MIN, (4000, 4000), bounds);
        assert!(grown.right <= 1920 && grown.top >= 0);
        assert_eq!((grown.width(), grown.height()), (480, 360));
        // The maximum track size of the window limits the growth.
        let capped = zoom_rect(base, 10.0, MIN, (800, 600), bounds);
        assert_eq!((capped.width(), capped.height()), (800, 600));
        // A base already beyond the limits is left alone at factor 1 (no jump on the first step).
        let over = Rect::new(-50, -50, 2000, 1100);
        assert_eq!(zoom_rect(over, 1.0, MIN, (1500, 1500), bounds), over);
        // 1.0 gives the base back; nonsense factors do not panic.
        assert_eq!(zoom_rect(base, 1.0, MIN, MAX, bounds), base);
        let _ = zoom_rect(base, f32::NAN, MIN, MAX, bounds);
        let _ = zoom_rect(base, -3.0, MIN, MAX, bounds);
    }

    #[test]
    fn zoom_works_on_a_monitor_left_of_the_primary() {
        let bounds = Rect::new(-1920, 0, 0, 1040);
        let base = Rect::new(-500, 100, -100, 400);
        let grown = zoom_rect(base, 1.5, MIN, (4000, 4000), bounds);
        assert!(grown.right <= 0 && grown.left >= -1920);
        assert_eq!((grown.width(), grown.height()), (600, 450));
        assert_eq!(grown.left + grown.right, -600);
    }

    #[test]
    fn presets_fill_the_work_area_parts() {
        use crate::config::WindowPreset as P;
        let work = Rect::new(0, 0, 1920, 1040);
        assert_eq!(
            preset_rect(work, P::LeftHalf),
            Some(Rect::new(0, 0, 960, 1040))
        );
        assert_eq!(
            preset_rect(work, P::RightHalf),
            Some(Rect::new(960, 0, 1920, 1040))
        );
        assert_eq!(
            preset_rect(work, P::TopHalf),
            Some(Rect::new(0, 0, 1920, 520))
        );
        assert_eq!(
            preset_rect(work, P::BottomHalf),
            Some(Rect::new(0, 520, 1920, 1040))
        );
        assert_eq!(
            preset_rect(work, P::CenterTwoThirds),
            Some(Rect::new(320, 173, 1600, 867))
        );
        for p in [P::Maximize, P::Restore, P::NextMonitor] {
            assert_eq!(preset_rect(work, p), None);
        }
    }

    #[test]
    fn presets_follow_a_monitor_at_negative_coordinates() {
        use crate::config::WindowPreset as P;
        let work = Rect::new(-2561, -1000, -1, 400);
        let l = preset_rect(work, P::LeftHalf).unwrap();
        let r = preset_rect(work, P::RightHalf).unwrap();
        assert_eq!(l.left, -2561);
        assert_eq!(l.right, r.left, "the halves meet without a gap");
        assert_eq!(r.right, -1);
        assert_eq!((l.top, l.bottom), (-1000, 400));
        let t = preset_rect(work, P::TopHalf).unwrap();
        let b = preset_rect(work, P::BottomHalf).unwrap();
        assert_eq!(t.bottom, b.top);
        assert_eq!((t.top, b.bottom), (-1000, 400));
        let odd = Rect::new(-3, -3, 98, 98);
        let (a, c) = (
            preset_rect(odd, P::LeftHalf).unwrap(),
            preset_rect(odd, P::RightHalf).unwrap(),
        );
        assert_eq!((a.left, a.right, c.right), (-3, c.left, 98));
        let mid = preset_rect(work, P::CenterTwoThirds).unwrap();
        assert!(mid.left > work.left && mid.right < work.right);
        assert!(mid.top > work.top && mid.bottom < work.bottom);
    }

    #[test]
    fn saved_rects_remember_the_first_place_and_drop_strangers() {
        let id = |pid: u32, c: &str| (pid, c.to_owned());
        let first = Rect::new(10, 10, 410, 310);
        let moved = Rect::new(0, 0, 960, 1040);
        let mut s = SavedRects::default();
        s.remember(7, id(1, "A"), first);
        // A second preset (or a next-monitor move) does not replace the original place.
        s.remember(7, id(1, "A"), moved);
        assert_eq!(s.take(7, &id(1, "A")), Some(first));
        assert_eq!(s.take(7, &id(1, "A")), None);
        // Next monitor, then restore: the place before the move comes back.
        s.remember(7, id(1, "A"), first);
        s.remember(7, id(1, "A"), Rect::new(1920, 0, 2320, 300));
        assert_eq!(s.take(7, &id(1, "A")), Some(first));
        // A handle reused by another window is not restored to the old place.
        s.remember(8, id(1, "A"), first);
        assert_eq!(s.take(8, &id(2, "B")), None);
        assert!(!s.contains(8));
        s.remember(9, id(1, "A"), first);
        s.remember(9, id(3, "C"), moved);
        assert_eq!(s.take(9, &id(3, "C")), Some(moved));
        s.remember(1, id(1, "A"), first);
        s.retain_alive(|_| false);
        assert!(!s.contains(1));
    }

    #[test]
    fn next_monitor_goes_left_to_right_and_wraps() {
        let left = Rect::new(-1920, 0, 0, 1040);
        let main = Rect::new(0, 0, 2560, 1400);
        let right = Rect::new(2560, -200, 4480, 880);
        // Enumeration order is arbitrary.
        let works = [main, right, left];
        assert_eq!(next_monitor(&works, left), Some(0));
        assert_eq!(next_monitor(&works, main), Some(1));
        assert_eq!(next_monitor(&works, right), Some(2));
        assert_eq!(next_monitor(&works[..1], main), None);
        assert_eq!(next_monitor(&works, Rect::new(1, 1, 2, 2)), None);
    }

    #[test]
    fn mapping_a_window_keeps_its_place_and_share() {
        let main = Rect::new(0, 0, 1920, 1040);
        let other = Rect::new(-2560, -300, 0, 1100);
        let half = Rect::new(960, 0, 1920, 1040);
        let m = map_rect(half, main, other);
        assert_eq!(m, Rect::new(-1280, -300, 0, 1100));
        assert_eq!(map_rect(m, other, main), half);
        // Never leaves the target, even for a window that stuck out of the source.
        let out = Rect::new(1800, 900, 2200, 1300);
        let m = map_rect(out, main, other);
        assert!(m.left >= other.left && m.right <= other.right);
        assert!(m.top >= other.top && m.bottom <= other.bottom);
    }

    #[test]
    fn fraction_and_scale() {
        let work = Rect::new(0, 0, 1920, 1040);
        assert_eq!(
            work.fraction(0.0, 0.0, 0.5, 1.0),
            Rect::new(0, 0, 960, 1040)
        );
        assert_eq!(
            work.fraction(0.5, 0.0, 0.5, 1.0),
            Rect::new(960, 0, 1920, 1040)
        );
        assert_eq!(scale_px(100, 96, 144), 150);
        assert_eq!(scale_px(100, 0, 144), 100);
    }

    #[test]
    fn floor_scales_with_dpi() {
        assert_eq!(scaled_floor(96), (200, 150));
        assert_eq!(scaled_floor(144), (300, 225));
        assert_eq!(scaled_floor(192), (400, 300));
        assert_eq!(scaled_floor(0), (200, 150));
        assert_eq!(default_insets(96), (7, 0, 7, 7));
        assert_eq!(default_insets(192), (14, 0, 14, 14));
    }

    #[test]
    fn hwnd_round_trip() {
        assert_eq!(Hwnd::from_win(HWND(std::ptr::null_mut())), None);
        let h = Hwnd(0x1234);
        assert_eq!(Hwnd::from_win(h.to_win()), Some(h));
        assert!(!Hwnd(0x1234).is_valid());
    }

    #[test]
    fn shell_classes_are_skipped() {
        assert!(is_shell_class("Shell_TrayWnd"));
        assert!(is_shell_class("Progman"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn own_integrity_is_readable() {
        assert!(own_integrity() >= 0x1000);
    }

    /// Read-only smoke test against the live desktop.
    #[test]
    fn foreground_queries_do_not_crash() {
        if let Some(h) = foreground() {
            let _ = work_area_of(h);
            let _ = visible_rect(h);
            let _ = dpi_of(h);
            let _ = is_elevated_window(h);
        }
        let _ = top_level_at((10, 10));
    }

    #[test]
    fn only_exclusive_full_screen_counts() {
        use windows::Win32::UI::Shell::*;
        assert!(counts_as_fullscreen_exclusive(QUNS_RUNNING_D3D_FULL_SCREEN));
        for other in [
            QUNS_NOT_PRESENT,
            QUNS_BUSY,
            QUNS_PRESENTATION_MODE,
            QUNS_ACCEPTS_NOTIFICATIONS,
            QUNS_QUIET_TIME,
            QUNS_APP,
        ] {
            assert!(!counts_as_fullscreen_exclusive(other));
        }
    }
}
