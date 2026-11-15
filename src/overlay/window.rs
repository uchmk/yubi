//! Win32 side of the overlay: window, message loop, GDI surface, fades.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FF_DONTCARE, GdiFlush, GetDC, GetMonitorInfoW,
    HBITMAP, HDC, HFONT, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    OUT_DEFAULT_PRECIS, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetMessageW, HTTRANSPARENT, HWND_TOPMOST, KillTimer, MA_NOACTIVATE, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, SPI_GETCLIENTAREAANIMATION, SW_HIDE, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetTimer, SetWindowPos, ShowWindow,
    SystemParametersInfoW, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WM_APP, WM_DESTROY,
    WM_MOUSEACTIVATE, WM_NCHITTEST, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::geometry::{self, Area, Layout};
use super::model::{Effect, Model, OverlayCmd};
use super::paint::{self, Highlight, PALETTE};
use super::{OverlayState, Shared};

const WM_WAKE: u32 = WM_APP + 1;
const WM_STOP: u32 = WM_APP + 2;
const TIMER_ID: usize = 1;
const TICK_MS: u32 = 15;

const FADE_IN_MS: u128 = 80;
const FADE_OUT_MS: u128 = 100;
const FLASH_MS: u128 = 120;

fn hwnd_of(v: isize) -> HWND {
    HWND(v as *mut c_void)
}

fn post(h: isize, msg: u32) -> bool {
    // SAFETY: posting to a window handle that may be gone is harmless (it just fails).
    h != 0 && unsafe { PostMessageW(Some(hwnd_of(h)), msg, WPARAM(0), LPARAM(0)).is_ok() }
}

/// False if the window is gone or the post failed.
pub(super) fn post_wake(h: isize) -> bool {
    post(h, WM_WAKE)
}

pub(super) fn post_stop(h: isize) -> bool {
    post(h, WM_STOP)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Hidden,
    FadeIn,
    Shown,
    Flash,
    FadeOut,
}

/// A top-down 32-bit DIB selected into a memory DC.
struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    size: i32,
}

impl Dib {
    fn new(size: i32) -> Result<Dib, String> {
        // SAFETY: standard DIB-section creation; handles are released in Drop.
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            if dc.is_invalid() {
                return Err("CreateCompatibleDC failed".into());
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(dc);
                    return Err("CreateDIBSection failed".into());
                }
            };
            let old = SelectObject(dc, bmp.into());
            Ok(Dib {
                dc,
                bmp,
                old,
                bits: bits as *mut u32,
                size,
            })
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: `bits` points to size*size u32 owned by the DIB section, alive while `self` is.
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.size * self.size) as usize) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: undoing the selection made in `new`.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Fonts {
    label: HFONT,
    icon: HFONT,
    dpi_scale: f32,
}

impl Fonts {
    fn new(scale: f32) -> Fonts {
        let make = |px: f32, weight: i32, face: PCWSTR| {
            // SAFETY: plain font creation; deleted in Drop.
            unsafe {
                CreateFontW(
                    -(px * scale).round() as i32,
                    0,
                    0,
                    0,
                    weight,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    ANTIALIASED_QUALITY,
                    FF_DONTCARE.0 as u32,
                    face,
                )
            }
        };
        Fonts {
            label: make(13.0, 600, w!("Segoe UI")),
            icon: make(20.0, 400, w!("Segoe Fluent Icons")),
            dpi_scale: scale,
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        // SAFETY: fonts were created in `new`.
        unsafe {
            let _ = DeleteObject(self.label.into());
            let _ = DeleteObject(self.icon.into());
        }
    }
}

struct Surface {
    main: Dib,
    scratch: Dib,
    fonts: Fonts,
}

struct App {
    hwnd: HWND,
    rx: Receiver<OverlayCmd>,
    shared: Arc<Shared>,
    model: Model,
    phase: Phase,
    phase_start: Instant,
    animate: bool,
    layout: Layout,
    origin: (i32, i32),
    surface: Option<Surface>,
    failed: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn animations_enabled() -> bool {
    let mut on: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut i32 as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_err() || on != 0
}

/// DPI and work area of the monitor nearest to `p`.
fn monitor_at(p: (i32, i32)) -> (u32, Area) {
    // SAFETY: plain queries with valid out pointers.
    unsafe {
        let m = MonitorFromPoint(POINT { x: p.0, y: p.1 }, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() || dx == 0 {
            dx = 96;
        }
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let area = if GetMonitorInfoW(m, &mut mi).as_bool() {
            let r = mi.rcWork;
            Area {
                left: r.left,
                top: r.top,
                right: r.right,
                bottom: r.bottom,
            }
        } else {
            Area {
                left: p.0 - 960,
                top: p.1 - 540,
                right: p.0 + 960,
                bottom: p.1 + 540,
            }
        };
        (dx, area)
    }
}

impl App {
    fn fail(&mut self, msg: String) {
        self.failed = true;
        self.phase = Phase::Hidden;
        self.kill_timer();
        // SAFETY: hiding our own window.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        if let Ok(mut g) = self.shared.state.lock() {
            *g = OverlayState::Failed(msg);
        }
    }

    fn set_phase(&mut self, p: Phase) {
        self.phase = p;
        self.phase_start = Instant::now();
        match p {
            Phase::FadeIn | Phase::Flash | Phase::FadeOut => {
                // SAFETY: timer on our own window.
                let ok = unsafe { SetTimer(Some(self.hwnd), TIMER_ID, TICK_MS, None) } != 0;
                if !ok {
                    // Without a timer nothing would end the animation: skip it.
                    match p {
                        Phase::FadeIn => {
                            self.phase = Phase::Shown;
                            let _ = self.present(1.0);
                        }
                        _ => self.hide(),
                    }
                }
            }
            _ => self.kill_timer(),
        }
    }

    fn kill_timer(&self) {
        // SAFETY: as above.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID);
        }
    }

    fn drain(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            if self.failed {
                continue;
            }
            let anchor_open = matches!(cmd, OverlayCmd::Open { .. });
            let effect = self.model.apply(cmd);
            let r = match effect {
                Effect::None => Ok(()),
                Effect::Open => self.on_open(anchor_open),
                Effect::Redraw => self.redraw(1.0),
                Effect::Flash => {
                    self.set_phase(Phase::Flash);
                    self.redraw(1.0)
                }
                Effect::Hide => {
                    self.begin_fade_out();
                    Ok(())
                }
            };
            if let Err(e) = r {
                self.fail(e);
            }
        }
    }

    fn begin_fade_out(&mut self) {
        if self.animate && self.phase != Phase::Hidden {
            self.set_phase(Phase::FadeOut);
        } else {
            self.hide();
        }
    }

    fn hide(&mut self) {
        self.set_phase(Phase::Hidden);
        // SAFETY: hiding our own window.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.shared.set_state(OverlayState::Hidden);
    }

    fn on_open(&mut self, _fresh: bool) -> Result<(), String> {
        self.animate = animations_enabled();
        let (dpi, work) = monitor_at(self.model.anchor);
        let layout = Layout::new(self.model.items.len(), dpi);
        self.origin = geometry::clamp_origin(self.model.anchor, layout.size, work);
        self.layout = layout;
        let need_new = match &self.surface {
            Some(s) => {
                s.main.size != layout.size || (s.fonts.dpi_scale - layout.scale).abs() > 0.01
            }
            None => true,
        };
        if need_new {
            self.surface = None;
            self.surface = Some(Surface {
                main: Dib::new(layout.size)?,
                scratch: Dib::new(layout.size)?,
                fonts: Fonts::new(layout.scale),
            });
        }
        let was_visible = matches!(self.phase, Phase::Shown | Phase::FadeIn);
        let alpha = if self.animate && !was_visible {
            0.0
        } else {
            1.0
        };
        self.redraw(alpha)?;
        // SAFETY: showing our own window without activating it, keeping it topmost.
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                self.origin.0,
                self.origin.1,
                layout.size,
                layout.size,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .map_err(|e| format!("SetWindowPos: {e}"))?;
        }
        self.present(alpha)?;
        self.shared.set_state(OverlayState::Shown);
        if self.animate && !was_visible {
            self.set_phase(Phase::FadeIn);
        } else {
            self.set_phase(Phase::Shown);
        }
        Ok(())
    }

    /// Re-render the ring and show it with `alpha`.
    fn redraw(&mut self, alpha: f32) -> Result<(), String> {
        if self.phase == Phase::Hidden && !self.model.open {
            return Ok(());
        }
        self.render()?;
        self.present(alpha)
    }

    fn render(&mut self) -> Result<(), String> {
        let layout = self.layout;
        let hl = match (self.model.selected, self.phase) {
            (Some(i), Phase::Flash | Phase::FadeOut) => Highlight::Flash(i),
            (Some(i), _) if !self.model.open => Highlight::Flash(i),
            (Some(i), _) => Highlight::Selected(i),
            (None, _) => Highlight::None,
        };
        let s = self.surface.as_mut().ok_or("no surface")?;
        paint::render_ring(s.main.pixels(), &layout, hl);
        draw_labels(s, &layout, &self.model, hl);
        Ok(())
    }

    fn present(&mut self, alpha: f32) -> Result<(), String> {
        let s = self.surface.as_ref().ok_or("no surface")?;
        let size = SIZE {
            cx: self.layout.size,
            cy: self.layout.size,
        };
        let dst = POINT {
            x: self.origin.0,
            y: self.origin.1,
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: the DIB DC is valid; all pointers outlive the call.
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&dst),
                Some(&size),
                Some(s.main.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
            .map_err(|e| format!("UpdateLayeredWindow: {e}"))
        }
    }

    fn tick(&mut self) {
        let ms = self.phase_start.elapsed().as_millis();
        let r = match self.phase {
            Phase::FadeIn => {
                if ms >= FADE_IN_MS {
                    self.set_phase(Phase::Shown);
                    self.present(1.0)
                } else {
                    self.present(ms as f32 / FADE_IN_MS as f32)
                }
            }
            Phase::Flash => {
                if ms >= FLASH_MS {
                    self.begin_fade_out();
                }
                Ok(())
            }
            Phase::FadeOut => {
                if ms >= FADE_OUT_MS {
                    self.hide();
                    Ok(())
                } else {
                    self.present(1.0 - ms as f32 / FADE_OUT_MS as f32)
                }
            }
            _ => {
                self.kill_timer();
                Ok(())
            }
        };
        if let Err(e) = r {
            self.fail(e);
        }
    }
}

/// Draw each slot's icon and label with GDI into the scratch DIB, then composite the coverage.
fn draw_labels(s: &mut Surface, l: &Layout, model: &Model, hl: Highlight) {
    let sel = match hl {
        Highlight::None => None,
        Highlight::Selected(i) | Highlight::Flash(i) => Some(i),
    };
    let size = l.size;
    let bw = (88.0 * l.scale) as i32;
    for (i, slot) in model.items.iter().enumerate() {
        let (lx, ly) = l.label_pos(i);
        let (lx, ly) = (lx.round() as i32, ly.round() as i32);
        let has_icon = slot.icon.is_some();
        // (rect, font, text) rows for this slot.
        let mut rows: Vec<(RECT, HFONT, Vec<u16>)> = Vec::new();
        if let Some(c) = slot.icon {
            let ih = (24.0 * l.scale) as i32;
            let cy = ly - (10.0 * l.scale) as i32;
            rows.push((
                RECT {
                    left: lx - bw / 2,
                    top: cy - ih / 2,
                    right: lx + bw / 2,
                    bottom: cy + ih / 2,
                },
                s.fonts.icon,
                c.to_string().encode_utf16().collect(),
            ));
        }
        let lh = (20.0 * l.scale) as i32;
        let cy = if has_icon {
            ly + (13.0 * l.scale) as i32
        } else {
            ly
        };
        rows.push((
            RECT {
                left: lx - bw / 2,
                top: cy - lh / 2,
                right: lx + bw / 2,
                bottom: cy + lh / 2,
            },
            s.fonts.label,
            slot.label.encode_utf16().collect(),
        ));
        let color = if sel == Some(i) {
            PALETTE.text_sel
        } else {
            PALETTE.text
        };
        for (mut rect, font, mut text) in rows {
            // Clear the scratch rectangle, draw white text, read back the coverage.
            let (x0, x1) = (rect.left.max(0), rect.right.min(size));
            let (y0, y1) = (rect.top.max(0), rect.bottom.min(size));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let sp = s.scratch.pixels();
            for y in y0..y1 {
                sp[(y * size + x0) as usize..(y * size + x1) as usize].fill(0);
            }
            // SAFETY: drawing into our own memory DC.
            unsafe {
                let old = SelectObject(s.scratch.dc, font.into());
                SetBkMode(s.scratch.dc, TRANSPARENT);
                SetTextColor(s.scratch.dc, COLORREF(0x00FF_FFFF));
                DrawTextW(
                    s.scratch.dc,
                    &mut text,
                    &mut rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
                SelectObject(s.scratch.dc, old);
                let _ = GdiFlush();
            }
            let sp = s.scratch.pixels();
            let mp = s.main.pixels();
            for y in y0..y1 {
                for x in x0..x1 {
                    let k = (y * size + x) as usize;
                    // Grey-scale AA: any channel is the coverage.
                    paint::blend_text(&mut mp[k], (sp[k] & 0xFF) as u8, color);
                }
            }
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_WAKE | WM_TIMER | WM_STOP => {
            // Re-entrant calls (messages sent while we call Win32) fall through harmlessly.
            APP.with(|a| {
                if let Ok(mut g) = a.try_borrow_mut()
                    && let Some(app) = g.as_mut()
                {
                    match msg {
                        WM_WAKE => app.drain(),
                        WM_TIMER => app.tick(),
                        _ => {
                            app.surface = None;
                            // SAFETY: destroying our own window from its thread.
                            unsafe {
                                let _ = DestroyWindow(h);
                            }
                        }
                    }
                }
            });
            return LRESULT(0);
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: default handling.
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

fn create_window() -> Result<HWND, String> {
    // SAFETY: class registration and window creation on this thread.
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("YubiRingOverlay");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        // Registering twice (a second Overlay in one process) fails harmlessly.
        RegisterClassW(&wc);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            class,
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW: {e}"))
    }
}

pub(super) fn run(
    rx: Receiver<OverlayCmd>,
    shared: Arc<Shared>,
    ready: Sender<Result<(), String>>,
) {
    // SAFETY: thread-wide DPI mode, so every coordinate here is a physical pixel.
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let hwnd = match create_window() {
        Ok(h) => h,
        Err(e) => {
            if let Ok(mut g) = shared.state.lock() {
                *g = OverlayState::Failed(e.clone());
            }
            let _ = ready.send(Err(e));
            return;
        }
    };
    shared.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            hwnd,
            rx,
            shared: shared.clone(),
            model: Model::default(),
            phase: Phase::Hidden,
            phase_start: Instant::now(),
            animate: true,
            layout: Layout::new(8, 96),
            origin: (0, 0),
            surface: None,
            failed: false,
        });
    });
    let _ = ready.send(Ok(()));
    let mut msg = MSG::default();
    // SAFETY: standard message loop; ends on WM_QUIT (posted from WM_DESTROY).
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    shared.hwnd.store(0, Ordering::SeqCst);
    APP.with(|a| *a.borrow_mut() = None);
}
