//! Win32 side of the overlay: window, message loop, GDI surface, fades.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    DEFAULT_CHARSET, DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW,
    FF_DONTCARE, GdiFlush, GetDC, GetMonitorInfoW, GetTextFaceW, HBITMAP, HDC, HFONT, HGDIOBJ,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, OUT_DEFAULT_PRECIS, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetMessageW, HTTRANSPARENT, HWND_TOPMOST, KillTimer, MA_NOACTIVATE, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, SPI_GETCLIENTAREAANIMATION, SW_HIDE,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetTimer, SetWindowPos,
    ShowWindow, SystemParametersInfoW, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WM_APP,
    WM_DESTROY, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::geometry::{self, Area, BandAnim, Layout, PointerFollow};
use super::level::{LevelInfo, LevelLayout, LevelModel, NoticeModel};
use super::model::{Effect, Model, OverlayCmd};
use super::paint::{self, Band, PALETTE, Pointer};
use super::{OverlayState, Shared};

const WM_WAKE: u32 = WM_APP + 1;
const WM_STOP: u32 = WM_APP + 2;
/// One frame of the frame clock.
const WM_FRAME: u32 = WM_APP + 3;
/// The timer that ends the level HUD / notice (it needs no frames, only a late wake-up).
const TIMER_ID: usize = 1;
const TICK_MS: u32 = 15;

const FADE_IN_MS: f32 = 80.0;
const FADE_OUT_MS: f32 = 100.0;
const FLASH_MS: f32 = 120.0;
/// The fade of the band and of the centre disc's light (the slide time comes from the style).
const BAND_FADE_MS: f32 = 100.0;

/// Paces the animation: while `on`, a thread waits for the display's vertical blank
/// (`DwmFlush`, else a short sleep) and posts one `WM_FRAME`. It parks when `on` is cleared, and
/// a frame is posted only after the previous one was handled (`pending`), so frames never queue.
struct Clock {
    on: Mutex<bool>,
    cv: Condvar,
    stop: AtomicBool,
    pending: AtomicBool,
}

impl Clock {
    fn new() -> Arc<Clock> {
        Arc::new(Clock {
            on: Mutex::new(false),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
            pending: AtomicBool::new(false),
        })
    }

    fn set(&self, on: bool) {
        if let Ok(mut g) = self.on.lock()
            && *g != on
        {
            *g = on;
            if on {
                self.cv.notify_all();
            }
        }
    }

    fn stop(&self) {
        if let Ok(_g) = self.on.lock() {
            self.stop.store(true, Ordering::SeqCst);
            self.cv.notify_all();
        }
    }

    fn run(&self, hwnd: isize) {
        loop {
            {
                let Ok(mut g) = self.on.lock() else { return };
                while !*g && !self.stop.load(Ordering::SeqCst) {
                    g = match self.cv.wait(g) {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                }
            }
            if self.stop.load(Ordering::SeqCst) {
                return;
            }
            // SAFETY: plain wait for the next composition; no pointers.
            if unsafe { DwmFlush() }.is_err() {
                std::thread::sleep(Duration::from_millis(8));
            }
            if !self.on.lock().map(|g| *g).unwrap_or(false) {
                continue;
            }
            if !self.pending.swap(true, Ordering::SeqCst) && !post(hwnd, WM_FRAME) {
                // The window is gone.
                return;
            }
        }
    }
}

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
    /// The level HUD is up; the timer waits for it to expire.
    Level,
}

/// What the window currently shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Ring,
    Level,
    Notice,
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
    /// The second line of a message card.
    hint: HFONT,
    icon: HFONT,
    /// The centre disc's caption and its cross.
    small: HFONT,
    cross: HFONT,
    /// False when neither icon face exists: glyphs are then left out.
    icon_ok: bool,
    dpi_scale: f32,
    /// The (label, icon) em sizes it was built for.
    key: (i32, i32),
}

impl Fonts {
    fn new(scale: f32, label_px: f32, icon_px: f32) -> Fonts {
        let (icon, face_ok) = make_icon_font(icon_px, scale);
        let (cross, _) = make_icon_font(18.0, scale);
        Fonts {
            hint: make_font(12.0, 400, scale, w!("Segoe UI")),
            label: make_font(label_px, 600, scale, w!("Segoe UI")),
            icon,
            small: make_font(11.0, 600, scale, w!("Segoe UI")),
            cross,
            icon_ok: face_ok,
            dpi_scale: scale,
            key: (label_px.round() as i32, icon_px.round() as i32),
        }
    }

    fn matches(&self, scale: f32, label_px: f32, icon_px: f32) -> bool {
        (self.dpi_scale - scale).abs() <= 0.01
            && self.key == (label_px.round() as i32, icon_px.round() as i32)
    }
}

fn make_font(px: f32, weight: i32, scale: f32, face: PCWSTR) -> HFONT {
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
}

/// The face GDI really picked for `font` (CreateFontW silently substitutes a missing face).
fn face_of(font: HFONT) -> String {
    // SAFETY: a throw-away memory DC; the font is selected and restored again.
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return String::new();
        }
        let old = SelectObject(dc, font.into());
        let mut buf = [0u16; 64];
        let n = GetTextFaceW(dc, Some(&mut buf)).max(0) as usize;
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        String::from_utf16_lossy(&buf[..n.min(buf.len())])
            .trim_end_matches('\0')
            .to_owned()
    }
}

/// Segoe Fluent Icons (Windows 11), else Segoe MDL2 Assets (Windows 10: the same code points).
/// The bool tells whether either face exists.
fn make_icon_font(px: f32, scale: f32) -> (HFONT, bool) {
    let fluent = make_font(px, 400, scale, w!("Segoe Fluent Icons"));
    if face_of(fluent).eq_ignore_ascii_case("Segoe Fluent Icons") {
        return (fluent, true);
    }
    // SAFETY: the font was just created above.
    unsafe {
        let _ = DeleteObject(fluent.into());
    }
    let mdl2 = make_font(px, 400, scale, w!("Segoe MDL2 Assets"));
    let ok = face_of(mdl2).eq_ignore_ascii_case("Segoe MDL2 Assets");
    (mdl2, ok)
}

impl Drop for Fonts {
    fn drop(&mut self) {
        // SAFETY: fonts were created in `new`.
        unsafe {
            let _ = DeleteObject(self.label.into());
            let _ = DeleteObject(self.hint.into());
            let _ = DeleteObject(self.icon.into());
            let _ = DeleteObject(self.small.into());
            let _ = DeleteObject(self.cross.into());
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
    /// The selection band and the centre disc's light.
    band: BandAnim,
    /// The pointer dot's direction.
    pointer: PointerFollow,
    /// The ring changed since it was last drawn.
    dirty: bool,
    /// The previous frame (for the step of the animations).
    last_frame: Instant,
    /// Frames were wanted at the last `sync_clock` (a clock that starts again begins its steps
    /// from now, not from the last frame before it stopped).
    frames_on: bool,
    /// Paces the frames; `None` if its thread could not start (the timer then does it).
    clock: Option<Arc<Clock>>,
    timer_on: bool,
    layout: Layout,
    origin: (i32, i32),
    surface: Option<Surface>,
    failed: bool,
    mode: Mode,
    level: LevelModel,
    notice: NoticeModel,
    level_layout: LevelLayout,
    /// Window size in pixels (the ring is square, the level HUD is not).
    win_size: (i32, i32),
    /// Zero of the millisecond clock the level model uses.
    epoch: Instant,
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
        self.sync_clock();
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
        self.last_frame = Instant::now();
        self.sync_clock();
    }

    /// The factor on the ring's fade times (the HUD is not scaled).
    fn fade_scale(&self) -> f32 {
        if self.mode == Mode::Ring {
            self.model.style.fade_scale
        } else {
            1.0
        }
    }

    /// The ring's animations (band, centre light, pointer) are all at rest.
    fn ring_settled(&self) -> bool {
        self.band.settled() && (!self.pointer_on() || self.pointer.settled())
    }

    fn pointer_on(&self) -> bool {
        self.model.open && self.model.style.pointer_dot
    }

    /// Whether frames are needed: a fade or flash is running, or the shown ring still moves.
    fn needs_frames(&self) -> bool {
        match self.phase {
            Phase::FadeIn | Phase::Flash | Phase::FadeOut => true,
            Phase::Shown => self.mode == Mode::Ring && !self.ring_settled(),
            Phase::Hidden | Phase::Level => false,
        }
    }

    /// Start or stop the frame clock and the HUD timer to match what is animating. The clock
    /// thread parks as soon as nothing needs frames.
    fn sync_clock(&mut self) {
        let frames = !self.failed && self.needs_frames();
        if frames && !self.frames_on {
            // The animations were at rest: the first step must not cover the idle time, or a
            // new slide would land in one frame.
            self.last_frame = Instant::now();
        }
        self.frames_on = frames;
        if let Some(c) = &self.clock {
            c.set(frames);
        }
        let want_timer =
            !self.failed && (self.phase == Phase::Level || (frames && self.clock.is_none()));
        if want_timer == self.timer_on {
            return;
        }
        if want_timer {
            // SAFETY: timer on our own window.
            let ok = unsafe { SetTimer(Some(self.hwnd), TIMER_ID, TICK_MS, None) } != 0;
            if ok {
                self.timer_on = true;
            } else if self.phase != Phase::Hidden {
                // Without a timer nothing would end the card: drop it.
                self.hide();
            }
        } else {
            // SAFETY: as above.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            }
            self.timer_on = false;
        }
    }

    /// The phase a finished fade-in ends in.
    fn settled(&self) -> Phase {
        match self.mode {
            Mode::Ring => Phase::Shown,
            Mode::Level | Mode::Notice => Phase::Level,
        }
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Take in every queued command. This only updates state; the ring is drawn once per frame
    /// (by the frame clock), or once here at the end if nothing is animating.
    fn drain(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            if self.failed {
                continue;
            }
            match cmd {
                OverlayCmd::Level(info) => {
                    // A ring that is open owns the window.
                    if !self.model.open
                        && let Err(e) = self.on_level(info)
                    {
                        self.fail(e);
                    }
                    continue;
                }
                OverlayCmd::Notice(text) => {
                    // The notice window never opens a ring, so this guard only protects a shared window.
                    if !self.model.open
                        && let Err(e) = self.on_notice(text)
                    {
                        self.fail(e);
                    }
                    continue;
                }
                OverlayCmd::LevelHide => {
                    if self.mode == Mode::Level && self.level.hide() {
                        self.hide();
                    }
                    continue;
                }
                _ => {}
            }
            let effect = self.model.apply(cmd);
            let r = match effect {
                Effect::None => Ok(()),
                Effect::Open => self.on_open(),
                Effect::Redraw => {
                    // The selection changed: the band slides, fades or jumps.
                    self.band.aim(
                        self.model.selected,
                        self.model.centre,
                        self.model.items.len(),
                        self.animate,
                    );
                    self.dirty = true;
                    Ok(())
                }
                Effect::Point => {
                    let (dx, dy) = self.model.point;
                    self.pointer.aim(dx, dy);
                    if !self.animate {
                        self.pointer.snap();
                    }
                    self.dirty = true;
                    Ok(())
                }
                Effect::Flash => {
                    // The flash on a commit is immediate.
                    self.band
                        .aim(self.model.selected, false, self.model.items.len(), false);
                    self.set_phase(Phase::Flash);
                    self.paint_frame(1.0)
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
        if self.failed {
            return;
        }
        // Nothing animates, so no frame is coming: draw what changed now.
        if self.dirty
            && self.phase == Phase::Shown
            && self.mode == Mode::Ring
            && let Err(e) = self.paint_frame(1.0)
        {
            return self.fail(e);
        }
        self.sync_clock();
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

    /// Show or update the level HUD on the monitor under the cursor.
    fn on_level(&mut self, info: LevelInfo) -> Result<(), String> {
        self.animate = animations_enabled();
        let was_visible =
            self.mode == Mode::Level && matches!(self.phase, Phase::Level | Phase::FadeIn);
        self.notice.hide();
        self.level.show(info, self.now_ms());
        self.mode = Mode::Level;
        self.show_card(was_visible)
    }

    /// Show a one-line notice card (pause / resume) on the monitor under the cursor.
    fn on_notice(&mut self, text: String) -> Result<(), String> {
        self.animate = animations_enabled();
        let was_visible =
            self.mode == Mode::Notice && matches!(self.phase, Phase::Level | Phase::FadeIn);
        self.level.hide();
        self.notice.show(text, self.now_ms());
        self.mode = Mode::Notice;
        self.show_card(was_visible)
    }

    /// Place, draw and show the HUD / notice card for the current mode.
    fn show_card(&mut self, was_visible: bool) -> Result<(), String> {
        let mut cur = POINT::default();
        // SAFETY: plain query with a valid out pointer.
        let have_cursor = unsafe { GetCursorPos(&mut cur).is_ok() };
        let at = if have_cursor { (cur.x, cur.y) } else { (0, 0) };
        let (dpi, work) = monitor_at(at);
        // A message card (two lines) is wider than the level HUD.
        let layout = match (self.mode, self.level.info()) {
            (Mode::Level, Some(info)) => LevelLayout::for_info(dpi, &info),
            _ => LevelLayout::new(dpi),
        };
        self.origin = layout.origin(work);
        self.level_layout = layout;
        self.win_size = (layout.width, layout.height);
        // The DIB is `width` wide and square; the HUD only uses its top rows.
        let side = layout.width;
        let need_new = match &self.surface {
            Some(s) => s.main.size != side || !s.fonts.matches(layout.scale, 13.0, 20.0),
            None => true,
        };
        if need_new {
            self.surface = None;
            self.surface = Some(Surface {
                main: Dib::new(side)?,
                scratch: Dib::new(side)?,
                fonts: Fonts::new(layout.scale, 13.0, 20.0),
            });
        }
        let alpha = if self.animate && !was_visible {
            0.0
        } else {
            1.0
        };
        self.render()?;
        // SAFETY: showing our own window without activating it, keeping it topmost.
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                self.origin.0,
                self.origin.1,
                layout.width,
                layout.height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .map_err(|e| format!("SetWindowPos: {e}"))?;
        }
        self.present(alpha)?;
        self.shared.set_state(OverlayState::Shown);
        if self.animate && !was_visible {
            self.set_phase(Phase::FadeIn);
        } else {
            self.set_phase(Phase::Level);
            // A fade-out in flight is cancelled by the new phase; show it fully.
            self.present(1.0)?;
        }
        Ok(())
    }

    fn on_open(&mut self) -> Result<(), String> {
        // The ring takes over the window from the level HUD.
        self.level.hide();
        self.notice.hide();
        self.mode = Mode::Ring;
        self.animate = animations_enabled();
        let (dpi, work) = monitor_at(self.model.anchor);
        let layout = Layout::new(self.model.items.len(), dpi);
        self.origin = geometry::clamp_origin(self.model.anchor, layout.size, work);
        self.layout = layout;
        self.win_size = (layout.size, layout.size);
        let m = geometry::slot_metrics(layout.n);
        let need_new = match &self.surface {
            Some(s) => {
                s.main.size != layout.size || !s.fonts.matches(layout.scale, m.label_px, m.icon_px)
            }
            None => true,
        };
        if need_new {
            self.surface = None;
            self.surface = Some(Surface {
                main: Dib::new(layout.size)?,
                scratch: Dib::new(layout.size)?,
                fonts: Fonts::new(layout.scale, m.label_px, m.icon_px),
            });
        }
        let was_visible = matches!(self.phase, Phase::Shown | Phase::FadeIn);
        // No band, and no carry-over from the previous ring.
        self.band = BandAnim::default();
        self.pointer = PointerFollow::default();
        let alpha = if self.animate && !was_visible {
            0.0
        } else {
            1.0
        };
        self.render()?;
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

    /// Draw the ring if it changed, and show it with `alpha`.
    fn paint_frame(&mut self, alpha: f32) -> Result<(), String> {
        if self.mode == Mode::Ring && (self.dirty || !self.ring_settled()) {
            self.render()?;
        }
        self.present(alpha)
    }

    fn render(&mut self) -> Result<(), String> {
        self.dirty = false;
        if self.mode == Mode::Notice {
            let text = self.notice.text().ok_or("no notice")?.to_owned();
            let l = self.level_layout;
            let s = self.surface.as_mut().ok_or("no surface")?;
            let stride = s.main.size as usize;
            paint::render_notice(s.main.pixels(), stride, &l);
            let rect = RECT {
                left: l.notice_rect().0,
                top: l.notice_rect().1,
                right: l.notice_rect().2,
                bottom: l.notice_rect().3,
            };
            let label = s.fonts.label;
            draw_text(
                s,
                rect,
                label,
                text.encode_utf16().collect(),
                DT_CENTER,
                PALETTE.text,
            );
            return Ok(());
        }
        if self.mode == Mode::Level {
            let info = self.level.info().ok_or("no level")?;
            let l = self.level_layout;
            let s = self.surface.as_mut().ok_or("no surface")?;
            let stride = s.main.size as usize;
            paint::render_level(s.main.pixels(), stride, &l, &info);
            draw_level_text(s, &l, &info);
            return Ok(());
        }
        let layout = self.layout;
        let flash = match (self.model.selected, self.phase) {
            (Some(i), Phase::Flash | Phase::FadeOut) => Some(i),
            (Some(i), _) if !self.model.open => Some(i),
            _ => None,
        };
        let band = Band {
            angle: self.band.angle,
            strength: self.band.strength,
        };
        let pointer = self.pointer_on().then(|| {
            let (dx, dy) = self.model.point;
            Pointer {
                angle: self.pointer.angle,
                alpha: geometry::pointer_alpha(dx.hypot(dy)),
            }
        });
        let centre = self.band.centre;
        let lit = flash.or_else(|| geometry::lit_slot(band.angle, band.strength, layout.n));
        let s = self.surface.as_mut().ok_or("no surface")?;
        paint::render_ring_band(s.main.pixels(), &layout, band, centre, flash, pointer);
        draw_labels(s, &layout, &self.model, lit, centre);
        Ok(())
    }

    fn present(&mut self, alpha: f32) -> Result<(), String> {
        let s = self.surface.as_ref().ok_or("no surface")?;
        let size = SIZE {
            cx: self.win_size.0,
            cy: self.win_size.1,
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

    /// One frame (from the clock) or one timer beat (the HUD): step the animations, draw, and
    /// see whether more frames are needed.
    fn tick(&mut self) {
        if self.failed {
            return;
        }
        let now = Instant::now();
        let dt = (now.duration_since(self.last_frame).as_secs_f32() * 1000.0).min(100.0);
        self.last_frame = now;
        if self.mode == Mode::Ring && self.phase != Phase::Hidden {
            let style = self.model.style;
            self.band
                .step(dt, style.slide_ms as f32, BAND_FADE_MS * style.fade_scale);
            if self.pointer_on() {
                self.pointer.step(dt, style.pointer_ms);
            }
        }
        let k = self.fade_scale();
        let ms = self.phase_start.elapsed().as_secs_f32() * 1000.0;
        let r = match self.phase {
            Phase::FadeIn => {
                if ms >= FADE_IN_MS * k {
                    let next = self.settled();
                    self.set_phase(next);
                    self.paint_frame(1.0)
                } else {
                    self.paint_frame(ms / (FADE_IN_MS * k))
                }
            }
            Phase::Flash => {
                if ms >= FLASH_MS * k {
                    self.begin_fade_out();
                }
                Ok(())
            }
            Phase::FadeOut => {
                if ms >= FADE_OUT_MS * k {
                    self.hide();
                    Ok(())
                } else {
                    self.paint_frame(1.0 - ms / (FADE_OUT_MS * k))
                }
            }
            Phase::Level => {
                let now = self.now_ms();
                if self.level.expired(now) || self.notice.expired(now) {
                    self.level.hide();
                    self.notice.hide();
                    self.begin_fade_out();
                }
                Ok(())
            }
            Phase::Shown if self.mode == Mode::Ring => {
                if self.dirty || !self.ring_settled() {
                    self.paint_frame(1.0)
                } else {
                    Ok(())
                }
            }
            Phase::Shown | Phase::Hidden => Ok(()),
        };
        match r {
            Ok(()) => self.sync_clock(),
            Err(e) => self.fail(e),
        }
    }
}

/// Draw each slot's picture and label, and the centre's cross and caption, with GDI into the
/// scratch DIB, then composite the coverage.
fn draw_labels(s: &mut Surface, l: &Layout, model: &Model, lit_slot: Option<usize>, centre: f32) {
    let lit = |i: usize| lit_slot == Some(i);
    let m = geometry::slot_metrics(l.n);
    let px = |v: f32| (v * l.scale).round() as i32;
    let bw = px(m.box_w);
    let lh = px(m.line_h());
    for (i, slot) in model.items.iter().enumerate() {
        let (lx, ly) = l.label_pos(i);
        let (lx, ly) = (lx.round() as i32, ly.round() as i32);
        let image = slot.image.as_ref().filter(|im| im.side > 0);
        let glyph = slot.icon.filter(|_| s.fonts.icon_ok && image.is_none());
        let visual = if image.is_some() {
            Some(m.image_px)
        } else {
            glyph.map(|_| m.icon_px)
        };
        let (icon_dy, label_dy) = m.rows(visual);
        let color = if lit(i) {
            PALETTE.text_sel
        } else {
            PALETTE.text
        };
        if let Some(im) = image {
            let side = px(m.image_px);
            let cy = ly + px(icon_dy);
            let stride = s.main.size as usize;
            let (x0, y0) = (lx - side / 2, cy - side / 2);
            paint::draw_image(s.main.pixels(), stride, x0, y0, side, &im.px, im.side);
        } else if let Some(c) = glyph {
            let ih = px(m.icon_px * 1.3);
            let cy = ly + px(icon_dy);
            let rect = RECT {
                left: lx - bw / 2,
                top: cy - ih / 2,
                right: lx + bw / 2,
                bottom: cy + ih / 2,
            };
            let font = s.fonts.icon;
            let text = c.to_string().encode_utf16().collect();
            draw_text(s, rect, font, text, DT_CENTER, color);
        }
        let cy = ly + px(label_dy);
        let rect = RECT {
            left: lx - bw / 2,
            top: cy - lh / 2,
            right: lx + bw / 2,
            bottom: cy + lh / 2,
        };
        let text = geometry::fit_label(&slot.label, m.box_w, m.label_px);
        let font = s.fonts.label;
        draw_text(
            s,
            rect,
            font,
            text.encode_utf16().collect(),
            DT_CENTER,
            color,
        );
    }
    draw_centre(s, l, centre >= 0.5);
}

/// The cancel disc: a cross over the caption, dim until the hand is back in it.
fn draw_centre(s: &mut Surface, l: &Layout, lit: bool) {
    let (cx, cy) = l.centre();
    let (cx, cy) = (cx.round() as i32, cy.round() as i32);
    let px = |v: f32| (v * l.scale).round() as i32;
    let color = if lit {
        PALETTE.text_sel
    } else {
        PALETTE.text_dim
    };
    let w = px(60.0);
    let line = px(15.0);
    let (cross, cross_font) = if s.fonts.icon_ok {
        ('\u{E711}', s.fonts.cross)
    } else {
        ('\u{00D7}', s.fonts.label)
    };
    let rect = RECT {
        left: cx - w / 2,
        top: cy - px(28.0),
        right: cx + w / 2,
        bottom: cy - px(2.0),
    };
    let text = cross.to_string().encode_utf16().collect();
    draw_text(s, rect, cross_font, text, DT_CENTER, color);
    let rect = RECT {
        left: cx - w / 2,
        top: cy + px(6.0) - line / 2,
        right: cx + w / 2,
        bottom: cy + px(6.0) + line / 2,
    };
    let small = s.fonts.small;
    let text = "取り消し".encode_utf16().collect();
    draw_text(s, rect, small, text, DT_CENTER, color);
}

/// Draw `text` into `rect` with GDI in the scratch DIB (white on black), then composite the
/// coverage of it over the main DIB in `color`.
fn draw_text(
    s: &mut Surface,
    mut rect: RECT,
    font: HFONT,
    mut text: Vec<u16>,
    align: DRAW_TEXT_FORMAT,
    color: paint::Rgba,
) {
    let size = s.main.size;
    // Clear the scratch rectangle, draw white text, read back the coverage.
    let (x0, x1) = (rect.left.max(0), rect.right.min(size));
    let (y0, y1) = (rect.top.max(0), rect.bottom.min(size));
    if x0 >= x1 || y0 >= y1 {
        return;
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
            align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
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

/// The level HUD's icon, label and number.
fn draw_level_text(s: &mut Surface, l: &LevelLayout, info: &LevelInfo) {
    let rect = |(left, top, right, bottom): (i32, i32, i32, i32)| RECT {
        left,
        top,
        right,
        bottom,
    };
    let icon = s.fonts.icon;
    let label = s.fonts.label;
    draw_text(
        s,
        rect(l.icon_rect()),
        icon,
        info.icon().to_string().encode_utf16().collect(),
        DT_CENTER,
        PALETTE.text,
    );
    let color = if info.muted {
        PALETTE.text_dim
    } else {
        PALETTE.text
    };
    draw_text(
        s,
        rect(l.label_rect()),
        label,
        info.label().encode_utf16().collect(),
        DT_LEFT,
        color,
    );
    if let Some(h) = info.hint() {
        let small = s.fonts.hint;
        draw_text(
            s,
            rect(l.hint_rect()),
            small,
            h.encode_utf16().collect(),
            DT_LEFT,
            PALETTE.text_dim,
        );
    }
    if let Some(n) = info.number() {
        draw_text(
            s,
            rect(l.number_rect()),
            label,
            n.encode_utf16().collect(),
            DT_RIGHT,
            PALETTE.text,
        );
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_WAKE | WM_TIMER | WM_STOP | WM_FRAME => {
            if msg == WM_FRAME {
                // Handled (or dropped) either way: the clock may post the next frame.
                APP.with(|a| {
                    if let Ok(g) = a.try_borrow()
                        && let Some(c) = g.as_ref().and_then(|app| app.clock.clone())
                    {
                        c.pending.store(false, Ordering::SeqCst);
                    }
                });
            }
            // Re-entrant calls (messages sent while we call Win32) fall through harmlessly.
            APP.with(|a| {
                if let Ok(mut g) = a.try_borrow_mut()
                    && let Some(app) = g.as_mut()
                {
                    match msg {
                        WM_WAKE => app.drain(),
                        WM_TIMER | WM_FRAME => app.tick(),
                        _ => {
                            if let Some(c) = &app.clock {
                                c.stop();
                            }
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
    let clock = Clock::new();
    let clock_thread = {
        let c = clock.clone();
        let h = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("yubi-overlay-frames".into())
            .spawn(move || c.run(h))
            .ok()
    };
    let clock = clock_thread.is_some().then_some(clock);
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            hwnd,
            rx,
            shared: shared.clone(),
            model: Model::default(),
            phase: Phase::Hidden,
            phase_start: Instant::now(),
            animate: true,
            band: BandAnim::default(),
            pointer: PointerFollow::default(),
            dirty: false,
            last_frame: Instant::now(),
            frames_on: false,
            clock,
            timer_on: false,
            layout: Layout::new(8, 96),
            origin: (0, 0),
            surface: None,
            failed: false,
            mode: Mode::Ring,
            level: LevelModel::default(),
            notice: NoticeModel::default(),
            level_layout: LevelLayout::new(96),
            win_size: (1, 1),
            epoch: Instant::now(),
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
    // The window is gone: the clock thread ends on its next beat.
    APP.with(|a| {
        if let Some(c) = a.borrow().as_ref().and_then(|app| app.clock.as_ref()) {
            c.stop();
        }
        *a.borrow_mut() = None;
    });
    if let Some(t) = clock_thread {
        let _ = t.join();
    }
}

/// Render one ring into memory (no window), for the screenshot test.
#[cfg(test)]
pub(super) fn snapshot(
    items: Vec<super::model::RingSlot>,
    hl: paint::Highlight,
    dpi: u32,
) -> Result<(i32, Vec<u32>), String> {
    let layout = Layout::new(items.len(), dpi);
    let m = geometry::slot_metrics(layout.n);
    let mut s = Surface {
        main: Dib::new(layout.size)?,
        scratch: Dib::new(layout.size)?,
        fonts: Fonts::new(layout.scale, m.label_px, m.icon_px),
    };
    paint::render_ring(s.main.pixels(), &layout, hl);
    let (_, centre, _) = paint::look_for(hl, layout.n);
    let mut model = Model::default();
    model.items = items;
    draw_labels(&mut s, &layout, &model, paint::lit_for(hl), centre);
    Ok((layout.size, s.main.pixels().to_vec()))
}
