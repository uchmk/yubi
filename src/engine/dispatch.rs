//! Dispatch: turns the recognizer's actions into effects.
//!
//! Mouse buttons and keys go through the executor (which owns everything held down). Windows are
//! moved by a dedicated worker thread fed by a channel, so a slow or hung target window can never
//! stall the engine thread. The per-app filter is a pure function over the profile list.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use super::executor::{Executor, InputSink, Key, MouseButton};
use super::recognizer::{Action, GestureEvent, SnapDir, Thresholds};
use super::tick::{LevelStep, level_changed};
use super::types::{EdgeSide, MultiKind, Suppress};
use crate::config;
use crate::os::keys::KeyCombo;
use crate::overlay::{LevelInfo, LevelKind};

pub const ELEVATED_NOTE: &str = "管理者のウィンドウは動かせません";
const SEND_FAILED_NOTE: &str = "入力を送れませんでした（管理者のウィンドウが前面かもしれません）";
pub const RELEASE_FAILED_NOTE: &str = "ボタンを離せませんでした（再試行します）";
const RELEASED_LATER_NOTE: &str = "ボタンを離しました";
pub const WORKER_NOTE: &str = "ウィンドウ操作を開始できませんでした";
pub const RING_FAILED_NOTE: &str = "リングを表示できません";
#[cfg(all(windows, not(test)))]
const CURSOR_FAILED_NOTE: &str = "カーソルを固定できませんでした";
#[cfg(all(windows, not(test)))]
const LEVEL_FAILED_NOTE: &str = "レベル表示を出せません";
pub const NO_CURSOR_NOTE: &str = "カーソル位置を取得できませんでした";
const LEFT: Key = Key::Mouse(MouseButton::Left);

/// Turns fractional pixel movement into whole pixels, carrying the remainder to the next call.
#[derive(Debug, Default, Clone, Copy)]
pub struct SubPixel {
    rx: f32,
    ry: f32,
}

impl SubPixel {
    pub fn take(&mut self, dx_px: f32, dy_px: f32) -> (i32, i32) {
        self.rx += dx_px;
        self.ry += dy_px;
        let (ix, iy) = (self.rx.trunc(), self.ry.trunc());
        self.rx -= ix;
        self.ry -= iy;
        (ix as i32, iy as i32)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// What a configured action does when triggered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Press these in order, release in reverse.
    Keys(Vec<Key>),
    Click(MouseButton),
    /// Change the master volume by this many percentage points.
    Volume(i8),
    /// Change the display brightness by this many percentage points.
    Brightness(i8),
    ToggleMute,
    /// Start an app (an exe or a shortcut) with optional arguments.
    Launch {
        path: String,
        args: String,
    },
    /// Put the foreground window in place (「ウィンドウを並べる」).
    Arrange(config::WindowPreset),
    /// Not implemented yet (ring menu) or "none".
    Nothing,
    /// The action cannot run; the text is for the diagnostics page.
    Invalid(String),
}

fn keys_of(combo: &KeyCombo) -> Vec<Key> {
    combo
        .modifiers
        .iter()
        .chain(std::iter::once(&combo.key))
        .map(|v| Key::Vk(v.0))
        .collect()
}

fn keys_plan(text: &str) -> Plan {
    match text.parse::<KeyCombo>() {
        Ok(c) => Plan::Keys(keys_of(&c)),
        Err(e) => Plan::Invalid(format!("キー「{text}」を送れません: {e}")),
    }
}

/// Name of a multi-finger gesture for the note shown after it fires.
pub fn multi_label(fingers: usize, kind: MultiKind) -> String {
    kind.label(fingers)
}

/// Name of a press for the note shown after it fires.
pub fn press_label(fingers: usize, deep: bool) -> String {
    format!(
        "{fingers}本指で{}押し込み",
        if deep { "深く" } else { "軽く" }
    )
}

/// Name of a corner trigger for the note shown after it fires ("左上・斜め", "右下・タップ").
pub fn corner_swipe_label(corner: usize, slot: config::CornerSlot) -> String {
    format!("{}・{}", corner_label(corner), slot.label())
}

fn corner_label(corner: usize) -> &'static str {
    config::Corners::NAMES[corner % 4]
}

/// The window corner a swipe out of pad corner `corner` drags: the opposite one, so a swipe toward
/// the middle of the pad grows the window.
fn opposite_corner(corner: usize) -> crate::os::window::Corner {
    use crate::os::window::Corner;
    match corner % 4 {
        0 => Corner::BottomRight,
        1 => Corner::BottomLeft,
        2 => Corner::TopRight,
        _ => Corner::TopLeft,
    }
}

const SCRUB_ALT: Key = Key::Vk(0x12);
const SCRUB_TAB: Key = Key::Vk(0x09);
const SCRUB_SHIFT: Key = Key::Vk(0x10);
const SCRUB_ESC: Key = Key::Vk(0x1B);

pub fn plan_for(action: &config::Action) -> Plan {
    use config::Action as A;
    match action {
        A::TaskView => keys_plan("Win+Tab"),
        A::ShowDesktop => keys_plan("Win+D"),
        A::DesktopPrev => keys_plan("Ctrl+Win+Left"),
        A::DesktopNext => keys_plan("Ctrl+Win+Right"),
        // Anywhere but a swipe the scrubbing switch is one plain Alt+Tab (the swipe streams as
        // `Action::AppSwitch*`, not as a plan).
        A::AppSwitch | A::AppSwitchScrub => keys_plan("Alt+Tab"),
        A::TabSwitch => keys_plan("Ctrl+Tab"),
        A::Find => keys_plan("Ctrl+F"),
        A::MiddleClick => Plan::Click(MouseButton::Middle),
        A::RightClick => Plan::Click(MouseButton::Right),
        A::Keys(s) => keys_plan(s),
        A::VolumeStep(n) => Plan::Volume(*n),
        A::BrightnessStep(n) => Plan::Brightness(*n),
        A::ToggleMute => Plan::ToggleMute,
        A::Launch { path, args } => {
            if path.trim().is_empty() {
                Plan::Invalid("起動するアプリが選ばれていません".to_owned())
            } else {
                Plan::Launch {
                    path: path.trim().trim_matches('"').to_owned(),
                    args: args.trim().to_owned(),
                }
            }
        }
        A::WindowArrange(p) => Plan::Arrange(*p),
        // The pinch zoom is a live gesture of its own (`Action::WindowZoom*`), not a plan.
        A::WindowZoom => Plan::Invalid("ピンチ専用の動作です".to_owned()),
        // The ring menu comes with M3c.
        // The corner resize is a live gesture of its own (`Action::CornerResize*`), not a plan.
        A::RingMenu | A::ResizeFromOpposite | A::None => Plan::Nothing,
    }
}

/// Commands for the window-ops worker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WinCmd {
    /// Pick the window under `cursor` and start moving it.
    Begin {
        cursor: (i32, i32),
    },
    Move {
        dx: i32,
        dy: i32,
    },
    End {
        snap: Option<SnapDir>,
    },
    /// Resize the foreground window by dragging `corner` (the one opposite the pad corner).
    BeginResize {
        corner: crate::os::window::Corner,
        cursor: (i32, i32),
    },
    Resize {
        dx: i32,
        dy: i32,
    },
    EndResize,
    /// Resize the window under `cursor` from the 3x3 grid cell the cursor is in.
    BeginGrid {
        cursor: (i32, i32),
    },
    GridMove {
        dx: i32,
        dy: i32,
    },
    /// Pick the window under `cursor` and scale it with a pinch.
    BeginZoom {
        cursor: (i32, i32),
    },
    /// Scale the window to `ratio` times its size at the start of the pinch.
    Zoom {
        ratio: f32,
    },
    EndZoom,
    /// Put the foreground window in place.
    Arrange(config::WindowPreset),
    /// Drop the move without snapping (device gone, config changed, ...).
    Cancel,
}

pub struct Dispatcher {
    /// Per-app filter result, latched at the start of each touch session.
    blocked: bool,
    /// Latest per-app result from the foreground poll; takes effect at the next touch start.
    live_blocked: bool,
    /// Practice mode: gestures are recognized but nothing new is run (ends still run).
    dry_run: bool,
    /// The recognizer has a drag open (begun, not yet ended).
    drag_open: bool,
    /// The left button was already (re)tried this touch session.
    press_tried: bool,
    /// A release was refused and not yet reported as fixed.
    release_pending: bool,
    drag_px: SubPixel,
    win_px: SubPixel,
    resize_px: SubPixel,
    win_tx: Option<Sender<WinCmd>>,
    win_join: Option<JoinHandle<()>>,
    /// A window move was accepted and not yet ended.
    win_active: bool,
    /// A corner resize was accepted and not yet ended.
    resize_active: bool,
    /// A pinch zoom was accepted and not yet ended.
    zoom_active: bool,
    /// The app switch scrub holds Alt (begun, not yet ended).
    scrub_active: bool,
    /// The corner swipe whose action is about to trigger.
    last_corner: Option<(usize, config::CornerSlot)>,
    last_multi: Option<(usize, MultiKind)>,
    /// The press (finger count, deep) whose action is about to trigger.
    last_press: Option<(usize, bool)>,
    /// Volume and brightness, opened on first use (engine thread only).
    sys: SystemOps,
    /// What the mouse hook swallows right now (edge slider, ring).
    suppression: Suppression,
    ring: RingUi,
    /// Show the level HUD for volume and brightness steps (the edge page setting).
    level_hud: bool,
    /// An edge slider detent is running its outputs.
    in_edge: bool,
    /// The "brightness cannot be changed" card was shown in this slide (once per slide).
    unavailable_shown: bool,
    /// The volume / brightness step of the action just handled (the engine ticks for it).
    level_step: Option<LevelStep>,
    /// Name shown instead of "ジェスチャー" for the next trigger (the ring's slot).
    source_override: Option<String>,
    /// The drag in flight began on a window's title bar (decided at `DragBegin`).
    drag_on_caption: bool,
    /// Whether a point of the screen is a title bar (the OS call; tests swap it).
    caption_at: fn(i32, i32) -> bool,
}

/// Cursor freeze for the edge slider and the ring, through the mouse hook (opened on first use;
/// never in tests).
#[derive(Default)]
struct Suppression {
    mode: Suppress,
    #[cfg(all(windows, not(test)))]
    hook: Option<crate::hook::Hook>,
    /// The hook could not be installed: do not retry on every touch.
    #[cfg(all(windows, not(test)))]
    failed: bool,
}

impl Suppression {
    /// Switch the mode. Returns a note when the freeze is not available.
    fn set(&mut self, mode: Suppress) -> Option<String> {
        self.mode = mode;
        self.apply()
    }

    #[cfg(all(windows, not(test)))]
    fn apply(&mut self) -> Option<String> {
        use crate::hook::Hook;
        let mut note = None;
        if self.mode != Suppress::Off && self.hook.is_none() && !self.failed {
            match Hook::start() {
                Ok(h) => self.hook = Some(h),
                Err(_) => {
                    self.failed = true;
                    note = Some(CURSOR_FAILED_NOTE.to_owned());
                }
            }
        }
        if let Some(h) = &self.hook {
            h.set_mode(hook_mode(self.mode));
        }
        note
    }

    #[cfg(not(all(windows, not(test))))]
    fn apply(&mut self) -> Option<String> {
        None
    }

    /// True when the hook cancelled the freeze on its own (watchdog or emergency combo) while
    /// one was wanted. `set(Off)` clears it.
    fn lost(&self) -> bool {
        #[cfg(all(windows, not(test)))]
        if self.mode != Suppress::Off
            && let Some(h) = &self.hook
        {
            return h.watchdog_tripped();
        }
        false
    }

    /// Keep the hook's lease alive; called every engine loop while a freeze is wanted. `alive`:
    /// a finger is on the pad and a fresh frame just arrived, which restarts the hook's 10 s
    /// watchdog window (a slow slide may last longer; the hook keeps an absolute cap).
    fn renew(&self, alive: bool) {
        #[cfg(all(windows, not(test)))]
        if self.mode != Suppress::Off
            && let Some(h) = &self.hook
        {
            // The lease may have run out while this thread was stalled (the first overlay or
            // hook start blocks): the hook then dropped to Off on its own. Assert the mode
            // again (this renews too). A watchdog downgrade stays downgraded inside `set_mode`;
            // the engine learns of it through `lost` (`freeze_lost`) and ends the gesture.
            let want = hook_mode(self.mode);
            if h.mode() != want {
                h.set_mode(want);
            }
            h.renew();
            if alive {
                h.touch_alive();
            }
        }
        #[cfg(not(all(windows, not(test))))]
        let _ = alive;
    }
}

#[cfg(all(windows, not(test)))]
fn hook_mode(s: Suppress) -> crate::hook::Mode {
    use crate::hook::Mode;
    match s {
        Suppress::Off => Mode::Off,
        Suppress::Cursor => Mode::FreezeCursor,
        Suppress::CursorAndClicks => Mode::FreezeAndHold,
        Suppress::Clicks => Mode::HoldClicks,
    }
}

/// The ring overlay window (opened on first use; never in tests).
#[derive(Default)]
struct RingUi {
    #[cfg(all(windows, not(test)))]
    overlay: Option<crate::overlay::Overlay>,
    /// The overlay is being created on its own thread (the engine loop never waits for it).
    #[cfg(all(windows, not(test)))]
    pending: Option<std::sync::mpsc::Receiver<Result<crate::overlay::Overlay, String>>>,
    /// The overlay window could not be created: the ring cannot be shown.
    #[cfg(all(windows, not(test)))]
    failed: bool,
    /// The overlay could not be shown; the engine cancels the ring.
    failed_now: bool,
    generation: u64,
    open: bool,
    /// The window is gone for good: the HUD stops (the failure also cancels the ring, since both
    /// share the window). A single refused send does not set this.
    #[cfg(all(windows, not(test)))]
    level_failed: bool,
    /// A refused HUD send was already reported once.
    #[cfg(all(windows, not(test)))]
    level_noted: bool,
}

impl RingUi {
    /// Start creating the overlay window on its own thread (once). Never blocks.
    #[cfg(all(windows, not(test)))]
    fn warm(&mut self) {
        if self.overlay.is_some() || self.pending.is_some() || self.failed {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("yubi-overlay-start".into())
            .spawn(move || {
                let _ = tx.send(crate::overlay::Overlay::start());
            });
        match spawned {
            Ok(_) => self.pending = Some(rx),
            Err(_) => self.failed = true,
        }
    }

    #[cfg(not(all(windows, not(test))))]
    fn warm(&mut self) {}

    /// Pick up the overlay once its thread has made it. Returns whether it is ready.
    #[cfg(all(windows, not(test)))]
    fn ready(&mut self) -> bool {
        use std::sync::mpsc::TryRecvError;
        if self.overlay.is_none()
            && let Some(rx) = &self.pending
        {
            match rx.try_recv() {
                Ok(Ok(o)) => {
                    self.overlay = Some(o);
                    self.pending = None;
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    self.failed = true;
                    self.pending = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        // A window that failed after creation (its thread discards commands) is as good as none.
        if self
            .overlay
            .as_ref()
            .is_some_and(|o| matches!(o.state(), crate::overlay::OverlayState::Failed(_)))
        {
            self.failed = true;
        }
        self.overlay.is_some() && !self.failed
    }

    #[cfg(all(windows, not(test)))]
    fn send(&mut self, cmd: crate::overlay::OverlayCmd) {
        if let Some(o) = &self.overlay
            && (matches!(o.state(), crate::overlay::OverlayState::Failed(_))
                || o.send(cmd).is_err())
        {
            self.failed_now = true;
        }
    }

    #[cfg(all(windows, not(test)))]
    fn show(&mut self, anchor: (i32, i32), labels: &[String], actions: &[config::Action]) {
        use crate::overlay::{OverlayCmd, RingSlot};
        self.warm();
        if !self.ready() {
            self.failed_now = true;
            return;
        }
        self.generation += 1;
        self.open = true;
        let items = labels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let Some(action) = actions.get(i) else {
                    return RingSlot {
                        label: l.clone(),
                        icon: None,
                        image: None,
                    };
                };
                let look = crate::ringlook::look_of(action);
                RingSlot {
                    label: crate::ringlook::slot_label(action, l),
                    icon: look.glyph,
                    image: look.image,
                }
            })
            .collect();
        let generation = self.generation;
        self.send(OverlayCmd::Open {
            anchor_px: anchor,
            items,
            generation,
        });
    }

    /// Show or update the level HUD. A note (once) when it cannot be shown. Not while a ring is
    /// open: the ring owns the window.
    #[cfg(all(windows, not(test)))]
    fn level(&mut self, info: LevelInfo) -> Option<String> {
        use crate::overlay::OverlayCmd;
        if self.open || self.level_failed {
            return None;
        }
        self.warm();
        if !self.ready() {
            // Still being created: skip this one. A creation failure ends in `failed`.
            if !self.failed {
                return None;
            }
            // The window is gone for good: stop trying.
            self.level_failed = true;
            return Some(LEVEL_FAILED_NOTE.to_owned());
        }
        let ok = self
            .overlay
            .as_ref()
            .is_some_and(|o| o.send(OverlayCmd::Level(info)).is_ok());
        if ok {
            return None;
        }
        // A refused send (a full queue) may pass: keep trying, but say so only once.
        if self.level_noted {
            return None;
        }
        self.level_noted = true;
        Some(LEVEL_FAILED_NOTE.to_owned())
    }

    #[cfg(not(all(windows, not(test))))]
    fn level(&mut self, _info: LevelInfo) -> Option<String> {
        None
    }

    #[cfg(all(windows, not(test)))]
    fn level_hide(&mut self) {
        if let Some(o) = &self.overlay {
            let _ = o.send(crate::overlay::OverlayCmd::LevelHide);
        }
    }

    #[cfg(not(all(windows, not(test))))]
    fn level_hide(&mut self) {}

    #[cfg(not(all(windows, not(test))))]
    fn show(&mut self, _anchor: (i32, i32), _labels: &[String], _actions: &[config::Action]) {
        self.generation += 1;
        self.open = true;
    }

    #[cfg(all(windows, not(test)))]
    fn select(&mut self, index: Option<usize>) {
        let generation = self.generation;
        if self.open {
            self.send(crate::overlay::OverlayCmd::Select { index, generation });
        }
    }

    #[cfg(not(all(windows, not(test))))]
    fn select(&mut self, _index: Option<usize>) {}

    #[cfg(all(windows, not(test)))]
    fn close(&mut self, committed: Option<usize>) {
        let generation = self.generation;
        if self.open {
            self.send(crate::overlay::OverlayCmd::Close {
                committed,
                generation,
            });
        }
        self.open = false;
    }

    #[cfg(not(all(windows, not(test))))]
    fn close(&mut self, _committed: Option<usize>) {
        self.open = false;
    }
}

/// True while the Esc key is held or was pressed since the last call (any window; the ring
/// cannot take keyboard focus). The "pressed since" bit catches a tap shorter than the poll.
#[cfg(windows)]
pub fn escape_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
    // SAFETY: plain query of the key state, no pointers.
    unsafe { (GetAsyncKeyState(i32::from(VK_ESCAPE.0)) as u16 & 0x8001) != 0 }
}

#[cfg(not(windows))]
pub fn escape_down() -> bool {
    false
}

impl Dispatcher {
    /// Dispatcher with a window worker thread; `notify` receives messages the worker wants shown.
    pub fn with_worker(notify: impl Fn(String) + Send + 'static) -> Self {
        let (tx, rx) = channel();
        match std::thread::Builder::new()
            .name("yubi-window-ops".into())
            .spawn(move || run_worker(rx, notify))
        {
            Ok(join) => {
                let mut d = Self::new(Some(tx));
                d.win_join = Some(join);
                // Make the overlay window now, off the hot path.
                d.ring.warm();
                d
            }
            // No worker: window moves report that they cannot start.
            Err(_) => Self::new(None),
        }
    }

    /// Dispatcher that sends window commands to `tx` (tests read the other end).
    #[cfg(test)]
    pub fn with_channel(tx: Sender<WinCmd>) -> Self {
        Self::new(Some(tx))
    }

    fn new(win_tx: Option<Sender<WinCmd>>) -> Self {
        Self {
            blocked: false,
            live_blocked: false,
            dry_run: false,
            drag_open: false,
            press_tried: false,
            release_pending: false,
            drag_px: SubPixel::default(),
            win_px: SubPixel::default(),
            resize_px: SubPixel::default(),
            win_tx,
            win_join: None,
            win_active: false,
            resize_active: false,
            zoom_active: false,
            scrub_active: false,
            last_corner: None,
            last_multi: None,
            last_press: None,
            sys: SystemOps::default(),
            suppression: Suppression::default(),
            ring: RingUi::default(),
            level_hud: true,
            in_edge: false,
            unavailable_shown: false,
            level_step: None,
            source_override: None,
            drag_on_caption: false,
            caption_at: crate::os::window::caption_at,
        }
    }

    /// Keep the cursor freeze alive (the hook's lease is short). Call it every engine loop.
    pub fn renew_suppression(&self, alive: bool) {
        self.suppression.renew(alive);
    }

    /// True while the hook has cancelled the cursor freeze by itself (watchdog, emergency combo).
    /// The engine then ends the edge slide / ring.
    pub fn freeze_lost(&self) -> bool {
        self.suppression.lost()
    }

    /// True once if the ring overlay could not be shown (the engine then cancels the ring).
    pub fn take_overlay_failed(&mut self) -> bool {
        std::mem::take(&mut self.ring.failed_now)
    }

    /// The edge page setting 「レベルを画面に表示する」. Turning it off hides a HUD that is up.
    pub fn set_level_hud(&mut self, on: bool) {
        self.level_hud = on;
        if !on {
            self.ring.level_hide();
        }
    }

    /// Hide the level HUD now (pause).
    pub fn hide_level(&mut self) {
        self.ring.level_hide();
    }

    /// The volume / brightness step the last action ran, once.
    pub fn take_level_step(&mut self) -> Option<LevelStep> {
        self.level_step.take()
    }

    /// Look at what the brightness worker has finished since the last step: the answer for a
    /// monitor that cannot be controlled (the card, once per slide) or the real level after a
    /// step whose level was not known yet. Cheap; the engine calls it every loop.
    pub fn poll_level(&mut self) -> Option<String> {
        let report = self.sys.poll_brightness();
        self.report_level(report)
    }

    fn report_level(&mut self, report: BrightnessReport) -> Option<String> {
        match report {
            BrightnessReport::Unavailable if !self.unavailable_shown => {
                self.unavailable_shown = true;
                let extra = self
                    .level_hud
                    .then(|| self.ring.level(LevelInfo::brightness_unavailable()))
                    .flatten();
                Some(match extra {
                    Some(extra) => format!("{BRIGHTNESS_UNAVAILABLE_NOTE}（{extra}）"),
                    None => BRIGHTNESS_UNAVAILABLE_NOTE.to_owned(),
                })
            }
            BrightnessReport::Level(v) if !self.unavailable_shown => {
                let info = LevelInfo {
                    kind: LevelKind::Brightness,
                    percent: Some(percent(v) as u8),
                    muted: false,
                };
                if self.level_hud {
                    let _ = self.ring.level(info);
                }
                None
            }
            _ => None,
        }
    }

    /// Record a level step and show the HUD. Returns the note, with a HUD failure appended.
    fn level_done(&mut self, info: LevelInfo, changed: bool, note: String) -> String {
        let step = LevelStep {
            from_edge: self.in_edge,
            changed,
        };
        self.level_step = Some(match self.level_step {
            Some(p) => p.merge(step),
            None => step,
        });
        // A brightness step with no known level shows nothing yet: an empty bar would read as 0 %.
        // `poll_level` shows the level, or the "cannot be changed" card, once the worker answers.
        let unknown_brightness = info.kind == LevelKind::Brightness && info.percent.is_none();
        let show = self.level_hud && !unknown_brightness;
        match show.then(|| self.ring.level(info)).flatten() {
            Some(extra) => format!("{note}（{extra}）"),
            None => note,
        }
    }

    /// Record the foreground app filter result. It takes effect at the next touch start, so
    /// the app is decided once per touch session.
    pub fn set_blocked(&mut self, blocked: bool) {
        self.live_blocked = blocked;
    }

    /// Practice mode on or off. Starting nothing new is all it does: whatever already began
    /// (a drag, a window move) still ends cleanly.
    pub fn set_dry_run(&mut self, on: bool) {
        self.dry_run = on;
    }

    /// The per-app filter decision of the current touch session.
    pub fn is_blocked(&self) -> bool {
        self.blocked
    }

    /// False when the worker is missing or has died.
    fn win(&self, cmd: WinCmd) -> bool {
        self.win_tx.as_ref().is_some_and(|tx| tx.send(cmd).is_ok())
    }

    /// Try again to let go of buttons the executor still owns that no gesture holds (an up was
    /// refused earlier). Returns a note when the retry finally worked.
    pub fn retry_release<S: InputSink>(&mut self, ex: &mut Executor<S>) -> Option<String> {
        if self.drag_open || self.scrub_active || ex.held().is_empty() {
            return None;
        }
        if ex.release_all() == 0 && self.release_pending {
            self.release_pending = false;
            return Some(RELEASED_LATER_NOTE.to_owned());
        }
        None
    }

    /// Let go of everything the executor holds and forget gestures in flight. Returns a note if
    /// some up was refused (`retry_release` keeps trying).
    pub fn release_everything<S: InputSink>(&mut self, ex: &mut Executor<S>) -> Option<String> {
        // A scrub in flight backs out first (Esc while Alt is still down), so nothing switches.
        self.abort_scrub(ex);
        let stuck = ex.release_all();
        self.cancel();
        (stuck > 0).then(|| {
            self.release_pending = true;
            RELEASE_FAILED_NOTE.to_owned()
        })
    }

    /// Whether the app switch scrub holds Alt right now.
    pub fn scrub_active(&self) -> bool {
        self.scrub_active
    }

    /// Back out of a scrub that is still open: Esc (closes the switcher without switching), then
    /// Alt up. Does nothing when no scrub is open.
    pub fn abort_scrub<S: InputSink>(&mut self, ex: &mut Executor<S>) {
        if std::mem::take(&mut self.scrub_active) {
            ex.tap(&[SCRUB_ESC]);
            ex.release(SCRUB_ALT);
        }
    }

    /// Forget a half-done gesture (the executor lets go of buttons and keys on its own).
    pub fn cancel(&mut self) {
        if self.win_active || self.resize_active || self.zoom_active {
            self.win(WinCmd::Cancel);
        }
        self.win_active = false;
        self.resize_active = false;
        self.zoom_active = false;
        self.scrub_active = false;
        self.drag_open = false;
        self.drag_on_caption = false;
        self.drag_px.clear();
        self.win_px.clear();
        self.resize_px.clear();
        self.suppression.set(Suppress::Off);
        self.ring.close(None);
        self.source_override = None;
    }

    /// Carry out one action. Returns a note for the diagnostics page when something notable happened.
    pub fn handle<S: InputSink>(
        &mut self,
        action: &Action,
        ex: &mut Executor<S>,
        th: &Thresholds,
        cursor: impl FnOnce() -> Option<(i32, i32)>,
    ) -> Option<String> {
        match action {
            Action::Debug(GestureEvent::TouchStart { .. }) => {
                // The per-app decision is made once, here, and holds for the whole session.
                self.blocked = self.live_blocked;
                self.press_tried = false;
                None
            }
            Action::Debug(GestureEvent::Multi { fingers, kind }) => {
                self.last_multi = Some((*fingers, *kind));
                None
            }
            Action::Debug(GestureEvent::Press { fingers, deep }) => {
                self.last_press = Some((*fingers, *deep));
                None
            }
            Action::Debug(GestureEvent::Corner { corner, slot }) => {
                self.last_corner = Some((*corner, *slot));
                None
            }
            Action::Debug(_) | Action::PressTick { .. } => None,
            // Ends always run, so a drag or window move that began is always closed.
            Action::DragEnd => {
                self.drag_px.clear();
                self.drag_open = false;
                let held = ex.held().contains(&LEFT);
                if !ex.release(LEFT) {
                    // Still owned; the engine retries until the up goes through.
                    self.release_pending = true;
                    return Some(RELEASE_FAILED_NOTE.to_owned());
                }
                held.then(|| "ドラッグ終了".to_owned())
            }
            Action::WindowMoveEnd { snap } => {
                if !self.win_active {
                    return None;
                }
                self.win_active = false;
                self.win_px.clear();
                // The worker reports what really happened (moved, snapped, no window, ...).
                (!self.win(WinCmd::End { snap: *snap })).then(|| WORKER_NOTE.to_owned())
            }
            Action::CornerResizeEnd | Action::WindowResizeEnd => {
                if !self.resize_active {
                    return None;
                }
                self.resize_active = false;
                self.resize_px.clear();
                (!self.win(WinCmd::EndResize)).then(|| WORKER_NOTE.to_owned())
            }
            Action::AppSwitchEnd { cancel } => {
                if !std::mem::take(&mut self.scrub_active) {
                    return None;
                }
                if *cancel {
                    ex.tap(&[SCRUB_ESC]);
                }
                // A refused up stays owned by the executor; `retry_release` keeps trying.
                if ex.release(SCRUB_ALT) {
                    None
                } else {
                    self.release_pending = true;
                    Some(RELEASE_FAILED_NOTE.to_owned())
                }
            }
            Action::WindowZoomEnd => {
                if !self.zoom_active {
                    return None;
                }
                self.zoom_active = false;
                (!self.win(WinCmd::EndZoom)).then(|| WORKER_NOTE.to_owned())
            }
            // Letting go of the cursor and closing the ring always run.
            Action::Suppress(Suppress::Off) => {
                // A new slide may tell again that the monitor cannot be controlled.
                self.unavailable_shown = false;
                self.suppression.set(Suppress::Off)
            }
            Action::RingSelect(i) => {
                self.ring.select(*i);
                None
            }
            Action::RingClose { committed } => {
                self.ring.close(*committed);
                if committed.is_some() {
                    self.source_override = Some("リング".to_owned());
                }
                None
            }
            _ if self.blocked || self.dry_run => None,
            Action::Suppress(s) => self.suppression.set(*s),
            Action::EdgeStep { side, outs, .. } => self.edge_step(*side, outs, ex),
            Action::RingOpen { labels, actions } => {
                let Some(anchor) = cursor() else {
                    // No position: do not open the ring at the corner. Let go of the freeze and
                    // let the engine cancel the ring through the "overlay failed" path.
                    self.suppression.set(Suppress::Off);
                    self.ring.failed_now = true;
                    return Some(NO_CURSOR_NOTE.to_owned());
                };
                // Consume a stale "pressed since last call" Esc bit: an Esc from before the ring
                // must not cancel it.
                let _ = escape_down();
                self.ring.show(anchor, labels, actions);
                self.ring.failed_now.then(|| RING_FAILED_NOTE.to_owned())
            }
            Action::DragBegin => {
                self.drag_open = true;
                self.press_tried = true;
                // Before the button goes down: what is under the cursor now is what is grabbed.
                self.drag_on_caption = cursor().is_some_and(|(x, y)| (self.caption_at)(x, y));
                // A left button still owned from an earlier refused release must not pass for a
                // fresh down: let go first, then press for real. Either step refused = failure.
                let ok = ex.release(LEFT) && ex.press(LEFT);
                Some(if ok {
                    "ドラッグ開始".to_owned()
                } else {
                    SEND_FAILED_NOTE.to_owned()
                })
            }
            Action::DragMove { dx_mm, dy_mm } => {
                let mut note = None;
                if self.drag_open && !self.press_tried && !ex.held().contains(&LEFT) {
                    // The drag resumed in a new touch session without a button (the earlier
                    // press was refused): try once per session.
                    self.press_tried = true;
                    if !ex.press(LEFT) {
                        note = Some(SEND_FAILED_NOTE.to_owned());
                    }
                }
                if ex.held().contains(&LEFT) {
                    // A drag that took hold of a title bar moves a window: use the window speed.
                    let gain = if self.drag_on_caption {
                        th.window_px_per_mm
                    } else {
                        th.drag_px_per_mm
                    };
                    let (dx, dy) = self.drag_px.take(dx_mm * gain, dy_mm * gain);
                    ex.move_mouse(dx, dy);
                }
                note
            }
            Action::WindowMoveBegin => {
                self.win_px.clear();
                let Some(cursor) = cursor() else {
                    return Some(NO_CURSOR_NOTE.to_owned());
                };
                // The worker reports whether a window was found and moved.
                self.win_active = self.win(WinCmd::Begin { cursor });
                (!self.win_active).then(|| WORKER_NOTE.to_owned())
            }
            Action::WindowMove { dx_mm, dy_mm } => {
                if self.win_active {
                    let (dx, dy) = self
                        .win_px
                        .take(dx_mm * th.window_px_per_mm, dy_mm * th.window_px_per_mm);
                    if dx != 0 || dy != 0 {
                        self.win(WinCmd::Move { dx, dy });
                    }
                }
                None
            }
            Action::CornerResizeBegin { corner } => {
                self.resize_px.clear();
                let Some(cursor) = cursor() else {
                    return Some(NO_CURSOR_NOTE.to_owned());
                };
                self.resize_active = self.win(WinCmd::BeginResize {
                    corner: opposite_corner(*corner),
                    cursor,
                });
                self.last_corner = None;
                (!self.resize_active).then(|| WORKER_NOTE.to_owned())
            }
            Action::WindowResizeBegin => {
                self.resize_px.clear();
                let Some(cursor) = cursor() else {
                    return Some(NO_CURSOR_NOTE.to_owned());
                };
                self.resize_active = self.win(WinCmd::BeginGrid { cursor });
                (!self.resize_active).then(|| WORKER_NOTE.to_owned())
            }
            Action::WindowResizeMove { dx_mm, dy_mm } => {
                if self.resize_active {
                    let (dx, dy) = self
                        .resize_px
                        .take(dx_mm * th.window_px_per_mm, dy_mm * th.window_px_per_mm);
                    if dx != 0 || dy != 0 {
                        self.win(WinCmd::GridMove { dx, dy });
                    }
                }
                None
            }
            Action::CornerResizeMove { dx_mm, dy_mm } => {
                if self.resize_active {
                    let (dx, dy) = self
                        .resize_px
                        .take(dx_mm * th.window_px_per_mm, dy_mm * th.window_px_per_mm);
                    if dx != 0 || dy != 0 {
                        self.win(WinCmd::Resize { dx, dy });
                    }
                }
                None
            }
            Action::AppSwitchBegin => self.scrub_begin(ex),
            Action::AppSwitchStep { forward } => {
                if !self.scrub_active {
                    return None;
                }
                let ok = if *forward {
                    ex.tap(&[SCRUB_TAB])
                } else {
                    ex.tap(&[SCRUB_SHIFT, SCRUB_TAB])
                };
                if ok {
                    None
                } else {
                    // A refused key (Shift or Tab left down) would turn the next steps around:
                    // back out of the scrub and let go of everything.
                    self.release_everything(ex);
                    Some(SEND_FAILED_NOTE.to_owned())
                }
            }
            Action::WindowZoomBegin => {
                let Some(cursor) = cursor() else {
                    return Some(NO_CURSOR_NOTE.to_owned());
                };
                self.zoom_active = self.win(WinCmd::BeginZoom { cursor });
                (!self.zoom_active).then(|| WORKER_NOTE.to_owned())
            }
            Action::WindowZoom { ratio } => {
                if self.zoom_active {
                    self.win(WinCmd::Zoom { ratio: *ratio });
                }
                None
            }
            Action::Scroll { h, v } => {
                // Sent as it comes; a refused wheel event is only worth a note, not a retry.
                (!ex.wheel(*h, *v)).then(|| SEND_FAILED_NOTE.to_owned())
            }
            Action::DragFlick(a) => {
                // The drag ended just before (`DragEnd` runs first), so the window's move loop is
                // over. Only a drag that took hold of a title bar flicks; a text selection does not.
                if !std::mem::take(&mut self.drag_on_caption) {
                    return None;
                }
                // A refused release keeps the button down: do not fire keys over a held button.
                if self.release_pending || ex.held().contains(&LEFT) {
                    return None;
                }
                self.unavailable_shown = false;
                self.run_plan(a, ex, "タイトルバーで払う")
            }
            Action::Trigger(a) => self.trigger(a, ex),
        }
    }

    fn trigger<S: InputSink>(
        &mut self,
        action: &config::Action,
        ex: &mut Executor<S>,
    ) -> Option<String> {
        // A gesture or ring action is its own session: it may tell again that a monitor cannot
        // be controlled (no `Suppress::Off` comes for it).
        self.unavailable_shown = false;
        let source = self.source_label();
        self.run_plan(action, ex, &source)
    }

    /// Name of what started the action about to run ("3本指で右スワイプ", the ring slot, ...).
    fn source_label(&mut self) -> String {
        match (
            self.source_override.take(),
            self.last_corner.take(),
            self.last_press.take(),
            self.last_multi.take(),
        ) {
            (Some(s), _, _, _) => s,
            (None, Some((c, slot)), _, _) => corner_swipe_label(c, slot),
            (None, None, Some((n, deep)), _) => press_label(n, deep),
            (None, None, None, Some((n, kind))) => multi_label(n, kind),
            (None, None, None, None) => "ジェスチャー".to_owned(),
        }
    }

    /// Start of the app switch scrub: Alt goes down and is held (the executor owns it, so every
    /// release path lets go), then one Tab opens the switcher.
    fn scrub_begin<S: InputSink>(&mut self, ex: &mut Executor<S>) -> Option<String> {
        let source = self.source_label();
        if !ex.press(SCRUB_ALT) {
            ex.release(SCRUB_ALT);
            return Some(SEND_FAILED_NOTE.to_owned());
        }
        self.scrub_active = true;
        let label = config::Action::AppSwitchScrub.label();
        Some(if ex.tap(&[SCRUB_TAB]) {
            format!("{source} → {label}")
        } else {
            self.release_everything(ex);
            SEND_FAILED_NOTE.to_owned()
        })
    }

    /// One detent of an edge slider.
    fn edge_step<S: InputSink>(
        &mut self,
        side: EdgeSide,
        outs: &[config::EdgeOut],
        ex: &mut Executor<S>,
    ) -> Option<String> {
        const CTRL: Key = Key::Vk(0x11);
        let mut note = None;
        self.in_edge = true;
        for out in outs {
            match out {
                config::EdgeOut::Act(a) => note = self.run_plan(a, ex, side.label()),
                config::EdgeOut::Wheel(n) => {
                    if !ex.wheel_notches(*n, side.horizontal()) {
                        note = Some(SEND_FAILED_NOTE.to_owned());
                    }
                }
                config::EdgeOut::CtrlWheel(n) => {
                    let ok = ex.press(CTRL) && ex.wheel_notches(*n, side.horizontal());
                    ex.release(CTRL);
                    if !ok {
                        note = Some(SEND_FAILED_NOTE.to_owned());
                    }
                }
            }
        }
        self.in_edge = false;
        note
    }

    fn run_plan<S: InputSink>(
        &mut self,
        action: &config::Action,
        ex: &mut Executor<S>,
        source: &str,
    ) -> Option<String> {
        let label = action.label();
        match plan_for(action) {
            Plan::Keys(keys) => Some(if ex.tap(&keys) {
                format!("{source} → {label}")
            } else {
                SEND_FAILED_NOTE.to_owned()
            }),
            Plan::Click(b) => Some(if ex.tap(&[Key::Mouse(b)]) {
                format!("{source} → {label}")
            } else {
                SEND_FAILED_NOTE.to_owned()
            }),
            Plan::Volume(n) => Some(match self.sys.volume_step(n) {
                Ok(st) => {
                    let note = format!("{source} → {label}（{}%）", percent(st.after));
                    let info = LevelInfo {
                        kind: LevelKind::Volume,
                        percent: Some(percent(st.after) as u8),
                        muted: st.muted,
                    };
                    let changed = level_changed(Some(st.before), Some(st.after));
                    self.level_done(info, changed, note)
                }
                Err(e) => e,
            }),
            Plan::ToggleMute => Some(match self.sys.toggle_mute() {
                Ok((muted, value)) => {
                    let note = format!(
                        "{source} → {}",
                        if muted {
                            "ミュート"
                        } else {
                            "ミュート解除"
                        }
                    );
                    let info = LevelInfo {
                        kind: LevelKind::Volume,
                        percent: value.map(|v| percent(v) as u8),
                        muted,
                    };
                    // A mute toggle is not a slider step: no tick.
                    self.level_done(info, false, note)
                }
                Err(e) => e,
            }),
            Plan::Brightness(n) => Some(match self.sys.brightness_step(n) {
                Ok((v, changed)) => {
                    let note = match v {
                        Some(v) => format!("{source} → {label}（{}%）", percent(v)),
                        None => format!("{source} → {label}"),
                    };
                    let info = LevelInfo {
                        kind: LevelKind::Brightness,
                        percent: v.map(|v| percent(v) as u8),
                        muted: false,
                    };
                    self.level_done(info, changed, note)
                }
                Err(e) => e,
            }),
            Plan::Launch { path, args } => Some(match launch_app(&path, &args) {
                Ok(()) => format!("{source} → {label}"),
                Err(e) => e,
            }),
            Plan::Arrange(p) => Some(if self.win(WinCmd::Arrange(p)) {
                // The worker reports it if the window cannot be moved.
                format!("{source} → {label}")
            } else {
                WORKER_NOTE.to_owned()
            }),
            Plan::Nothing => Some(format!("{source} → {label}（まだ動作しません）")),
            Plan::Invalid(msg) => Some(msg),
        }
    }
}

/// Start `path` (an exe or a .lnk) through the shell. The existence check and the shell call
/// run on their own short thread so a slow disk or app never stalls the engine; a failure there
/// cannot reach the diagnostics note any more, so it is logged with the path.
fn launch_app(path: &str, args: &str) -> Result<(), String> {
    #[cfg(all(windows, not(test)))]
    {
        let (path, args) = (path.to_owned(), args.to_owned());
        std::thread::Builder::new()
            .name("yubi-launch".into())
            .spawn(move || {
                if !std::path::Path::new(&path).exists() {
                    eprintln!("yubi: app not found: {path}");
                } else if !crate::os::appicon::shell_open(&path, &args) {
                    eprintln!("yubi: the shell could not start {path}");
                }
            })
            .map(|_| ())
            .map_err(|e| format!("アプリを起動できません: {e}"))
    }
    #[cfg(not(all(windows, not(test))))]
    {
        let _ = (path, args);
        Ok(())
    }
}

/// A fraction 0..=1 as a whole percentage.
fn percent(v: f32) -> u32 {
    (v * 100.0).round().clamp(0.0, 100.0) as u32
}

/// The fraction a step of `n` percentage points adds.
pub fn step_fraction(n: i8) -> f32 {
    f32::from(n) / 100.0
}

const VOLUME_FAILED_NOTE: &str = "音量を変えられませんでした";
const BRIGHTNESS_FAILED_NOTE: &str = "明るさを変えられませんでした";
const BRIGHTNESS_UNAVAILABLE_NOTE: &str =
    "このモニターは明るさを変えられません（DDC/CI に対応していないか、オフです）";

thread_local! {
    /// The audio endpoint is a COM object that must stay on the thread that opened it (the engine
    /// thread), so it lives here instead of in the `Send` dispatcher.
    static VOLUME: std::cell::RefCell<Option<crate::os::volume::Volume>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` on the engine thread's audio endpoint, opening it on first use.
fn with_volume<T>(
    f: impl FnOnce(&crate::os::volume::Volume) -> windows::core::Result<T>,
) -> Result<T, String> {
    VOLUME.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot =
                Some(crate::os::volume::Volume::open().map_err(|_| VOLUME_FAILED_NOTE.to_owned())?);
        }
        let vol = slot.as_ref().ok_or_else(|| VOLUME_FAILED_NOTE.to_owned())?;
        f(vol).map_err(|_| {
            // A device change invalidates the endpoint: reopen it on the next use.
            *slot = None;
            VOLUME_FAILED_NOTE.to_owned()
        })
    })
}

/// The OS side of the volume and brightness actions. Both are opened on first use.
#[derive(Default)]
struct SystemOps {
    brightness: Option<crate::os::brightness::BrightnessWorker>,
    /// The brightness the last step is expected to give (for the overlay text only; the worker
    /// adds the step to a fresh read) and its sequence number. It lives across gestures so two
    /// quick gestures both count, until the worker reports back (see [`pending_brightness`]).
    brightness_target: Option<(f32, u64)>,
    /// The newest step the worker has not been heard back about: its sequence number and whether
    /// the HUD had to show it without a level.
    brightness_awaiting: Option<(u64, bool)>,
    /// The monitor under the cursor at the last brightness step.
    brightness_monitor: Option<isize>,
}

/// What the worker found out about the newest brightness step.
#[derive(Debug, Clone, Copy, PartialEq)]
enum BrightnessReport {
    /// Nothing (yet, or nothing to say).
    Nothing,
    /// The monitor under the cursor cannot be controlled.
    Unavailable,
    /// The step went through and the level it left is now known.
    Level(f32),
}

/// Decide what to tell about the step `seq` (shown with an unknown level when `unknown`), given
/// the worker's status. `None` while the worker has not finished with it (ask again later).
fn brightness_report(
    seq: u64,
    unknown: bool,
    status: &crate::os::brightness::WorkerStatus,
) -> Option<BrightnessReport> {
    if status.applied_seq < seq {
        return None;
    }
    Some(match (&status.error, status.value) {
        (Some(_), _) => BrightnessReport::Unavailable,
        (None, Some(v)) if unknown => BrightnessReport::Level(v),
        _ => BrightnessReport::Nothing,
    })
}

/// A cached brightness older than this is not shown as a percentage (the monitor's own buttons
/// may have changed it since).
const BRIGHTNESS_FRESH: std::time::Duration = std::time::Duration::from_secs(2);

/// The step target that is still waiting for the worker, or `None` once the worker has finished
/// with it (applied or failed). A report for an older step keeps a newer target.
fn pending_brightness(
    target: Option<(f32, u64)>,
    status: &crate::os::brightness::WorkerStatus,
) -> Option<(f32, u64)> {
    let (_, seq) = target?;
    (status.applied_seq < seq).then_some(target).flatten()
}

/// The percentage base for the overlay text: the expected value of a pending step, else the
/// worker's last value while it is fresh. `None` when it would be a guess.
fn brightness_base(
    pending: Option<(f32, u64)>,
    value: Option<f32>,
    age: Option<std::time::Duration>,
) -> Option<f32> {
    pending
        .map(|t| t.0)
        .or_else(|| value.filter(|_| age.is_some_and(|a| a <= BRIGHTNESS_FRESH)))
}

impl SystemOps {
    /// The worker's answer for the newest brightness step, once, when it has one.
    fn poll_brightness(&mut self) -> BrightnessReport {
        let (Some((seq, unknown)), Some(worker)) = (self.brightness_awaiting, &self.brightness)
        else {
            return BrightnessReport::Nothing;
        };
        if !worker.is_running() {
            self.brightness_awaiting = None;
            return BrightnessReport::Nothing;
        }
        match brightness_report(seq, unknown, &worker.status()) {
            Some(r) => {
                self.brightness_awaiting = None;
                r
            }
            None => BrightnessReport::Nothing,
        }
    }

    fn volume_step(&mut self, n: i8) -> Result<VolumeStep, String> {
        // A brightness answer must not replace the volume HUD shown now.
        self.brightness_awaiting = None;
        with_volume(|v| {
            let before = v.get()?;
            let after = crate::os::volume::step_value(before, step_fraction(n));
            v.set(after)?;
            // A failed mute read only costs the HUD its muted look.
            let muted = v.is_muted().unwrap_or(false);
            Ok(VolumeStep {
                before,
                after,
                muted,
            })
        })
    }

    /// Toggle mute; returns the new mute state and the volume. The volume is `None` when it could
    /// not be read back: the toggle itself still worked.
    fn toggle_mute(&mut self) -> Result<(bool, Option<f32>), String> {
        self.brightness_awaiting = None;
        with_volume(|v| Ok((v.toggle_mute()?, v.get().ok())))
    }

    /// Never blocks: the worker thread talks to the display. It reads the brightness fresh and adds
    /// the step, so a change made with the monitor's own buttons is respected. The returned value
    /// is only the expected result (`None` when the cached value is too old to say); the flag says
    /// whether the step moved the level (false at the 0 % / 100 % end).
    fn brightness_step(&mut self, n: i8) -> Result<(Option<f32>, bool), String> {
        let worker = self
            .brightness
            .get_or_insert_with(crate::os::brightness::BrightnessWorker::spawn);
        if !worker.is_running() {
            // Nobody would answer: do not wait for it.
            self.brightness_awaiting = None;
            return Err(BRIGHTNESS_FAILED_NOTE.to_owned());
        }
        let mut status = worker.status();
        // A value or a pending target that belongs to another monitor is not a base.
        let here = crate::os::brightness::cursor_monitor();
        if here != self.brightness_monitor {
            self.brightness_target = None;
            self.brightness_monitor = here;
        }
        if status.monitor != here {
            status.value = None;
        }
        self.brightness_target = pending_brightness(self.brightness_target, &status);
        let delta = step_fraction(n);
        let base = brightness_base(
            self.brightness_target,
            status.value,
            status.updated.map(|t| t.elapsed()),
        );
        let seq = worker.step(delta);
        self.brightness_awaiting = Some((seq, base.is_none()));
        let expected = base.map(|b| crate::os::volume::step_value(b, delta));
        self.brightness_target = expected.map(|v| (v, seq));
        match (&status.error, status.value) {
            (Some(_), None) => Err(BRIGHTNESS_FAILED_NOTE.to_owned()),
            // Unknown base: nothing to compare, so no claim that the level moved (no tick).
            _ => Ok((expected, base.is_some() && level_changed(base, expected))),
        }
    }
}

/// One volume step: the value before and after, and whether the endpoint is muted.
struct VolumeStep {
    before: f32,
    after: f32,
    muted: bool,
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        // Closing the channel ends the worker; wait so a move in flight finishes cleanly.
        self.win_tx = None;
        if let Some(j) = self.win_join.take() {
            let _ = j.join();
        }
    }
}

fn snap_label(d: SnapDir) -> &'static str {
    match d {
        SnapDir::Left => "左にスナップ",
        SnapDir::Right => "右にスナップ",
        SnapDir::Up => "最大化",
        SnapDir::Down => "元に戻す・最小化",
    }
}

/// What the 3x3 cell picked, for the note shown while the drag starts.
fn grid_label(h: (i8, i8)) -> String {
    let what = match h {
        (-1, -1) => "左上の角",
        (1, -1) => "右上の角",
        (-1, 1) => "左下の角",
        (1, 1) => "右下の角",
        (-1, 0) => "左の辺",
        (1, 0) => "右の辺",
        (0, -1) => "上の辺",
        (0, 1) => "下の辺",
        _ => "中央 (移動)",
    };
    format!("ウィンドウのサイズ変更: {what}")
}

fn run_worker(rx: Receiver<WinCmd>, notify: impl Fn(String)) {
    use crate::os::window::{self, SnapDir as OsSnap, WindowMover};
    let mut mover: Option<WindowMover> = None;
    let mut resize_corner = window::Corner::BottomRight;
    let mut grid = (0i8, 0i8);
    for cmd in rx {
        match cmd {
            WinCmd::Begin { cursor } => {
                mover = None;
                match window::top_level_at(cursor) {
                    None => notify("動かせるウィンドウがありません".to_owned()),
                    Some(h) if window::is_elevated_window(h) => notify(ELEVATED_NOTE.to_owned()),
                    Some(h) => {
                        mover = WindowMover::begin(h, cursor);
                        notify(if mover.is_some() {
                            "ウィンドウ移動".to_owned()
                        } else {
                            "ウィンドウを動かせませんでした".to_owned()
                        });
                    }
                }
            }
            WinCmd::Move { dx, dy } => {
                if let Some(m) = mover.as_mut() {
                    m.move_by(dx, dy);
                }
            }
            WinCmd::End { snap } => {
                // No mover: the begin already told why, so there is nothing to add.
                if let Some(m) = mover.take() {
                    let hwnd = m.hwnd();
                    m.end();
                    notify(match snap {
                        None => "ウィンドウ移動 終了".to_owned(),
                        Some(d) => {
                            let os = match d {
                                SnapDir::Left => OsSnap::Left,
                                SnapDir::Right => OsSnap::Right,
                                SnapDir::Up => OsSnap::Up,
                                SnapDir::Down => OsSnap::Down,
                            };
                            if window::snap(hwnd, os) {
                                format!("ウィンドウ移動 → {}", snap_label(d))
                            } else {
                                "スナップできませんでした".to_owned()
                            }
                        }
                    });
                }
            }
            WinCmd::BeginResize { corner, cursor } => {
                mover = None;
                match window::foreground() {
                    None => notify("サイズを変えられるウィンドウがありません".to_owned()),
                    Some(h) if window::is_elevated_window(h) => notify(ELEVATED_NOTE.to_owned()),
                    Some(h) => {
                        mover = WindowMover::begin(h, cursor);
                        resize_corner = corner;
                        notify(if mover.is_some() {
                            "ウィンドウのサイズ変更".to_owned()
                        } else {
                            "ウィンドウのサイズを変えられませんでした".to_owned()
                        });
                    }
                }
            }
            WinCmd::Resize { dx, dy } => {
                if let Some(m) = mover.as_mut() {
                    m.resize_from(resize_corner, dx, dy);
                }
            }
            WinCmd::BeginGrid { cursor } => {
                mover = None;
                match window::top_level_at(cursor) {
                    None => notify("サイズを変えられるウィンドウがありません".to_owned()),
                    Some(h) if window::is_elevated_window(h) => notify(ELEVATED_NOTE.to_owned()),
                    Some(h) => {
                        // The cell is read before a maximised window is restored under the cursor.
                        let before = window::visible_rect(h);
                        mover = WindowMover::begin(h, cursor);
                        if let Some(m) = mover.as_ref() {
                            grid = window::grid_handle(before.unwrap_or_else(|| m.rect()), cursor);
                            notify(grid_label(grid));
                        } else {
                            notify("ウィンドウのサイズを変えられませんでした".to_owned());
                        }
                    }
                }
            }
            WinCmd::GridMove { dx, dy } => {
                if let Some(m) = mover.as_mut() {
                    m.resize_handle(grid, dx, dy);
                }
            }
            WinCmd::EndResize => {
                if let Some(m) = mover.take() {
                    m.end();
                    notify("ウィンドウのサイズ変更 終了".to_owned());
                }
            }
            WinCmd::BeginZoom { cursor } => {
                mover = None;
                match window::top_level_at(cursor) {
                    None => notify("拡大・縮小できるウィンドウがありません".to_owned()),
                    Some(h) if window::is_elevated_window(h) => notify(ELEVATED_NOTE.to_owned()),
                    Some(h) => {
                        mover = WindowMover::begin_zoom(h, cursor);
                        notify(if mover.is_some() {
                            "ウィンドウの拡大・縮小".to_owned()
                        } else {
                            "ウィンドウの大きさを変えられませんでした".to_owned()
                        });
                    }
                }
            }
            WinCmd::Zoom { ratio } => {
                if let Some(m) = mover.as_mut() {
                    m.zoom_to(ratio);
                }
            }
            WinCmd::EndZoom => {
                if let Some(m) = mover.take() {
                    m.end();
                    notify("ウィンドウの拡大・縮小 終了".to_owned());
                }
            }
            WinCmd::Arrange(preset) => match window::foreground() {
                None => notify("並べられるウィンドウがありません".to_owned()),
                Some(h) if window::is_elevated_window(h) => notify(ELEVATED_NOTE.to_owned()),
                Some(h) => {
                    if let Err(e) = window::apply_preset(h, preset) {
                        notify(e);
                    }
                }
            },
            WinCmd::Cancel => {
                if let Some(m) = mover.take() {
                    m.end();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BrightnessReport, brightness_report, pending_brightness};
    use crate::os::brightness::{BrightnessError, WorkerStatus};

    #[test]
    fn brightness_report_follows_the_worker() {
        let st = |applied_seq, value, failed: bool| WorkerStatus {
            applied_seq,
            value,
            error: failed.then_some(BrightnessError::Unsupported),
            ..WorkerStatus::default()
        };
        // Not answered yet: ask again later.
        assert_eq!(brightness_report(3, true, &st(2, None, false)), None);
        assert_eq!(brightness_report(3, true, &st(2, None, true)), None);
        // A monitor that cannot be controlled, or a failed step: the card.
        assert_eq!(
            brightness_report(3, true, &st(3, None, true)),
            Some(BrightnessReport::Unavailable)
        );
        assert_eq!(
            brightness_report(3, false, &st(4, None, true)),
            Some(BrightnessReport::Unavailable)
        );
        // The level of a step shown without one is filled in; a known one needs nothing more.
        assert_eq!(
            brightness_report(3, true, &st(3, Some(1.0), false)),
            Some(BrightnessReport::Level(1.0))
        );
        assert_eq!(
            brightness_report(3, false, &st(3, Some(1.0), false)),
            Some(BrightnessReport::Nothing)
        );
    }

    #[test]
    fn brightness_target_survives_until_the_worker_reports() {
        let st = |applied_seq| WorkerStatus {
            applied_seq,
            ..WorkerStatus::default()
        };
        let t = Some((0.6, 4));
        // Nothing finished since the step: a second quick gesture builds on the target.
        assert_eq!(pending_brightness(t, &st(3)), t);
        // A report (success or failure) for an older step must not drop a newer pending target.
        let failed = WorkerStatus {
            error: Some(crate::os::brightness::BrightnessError::Unsupported),
            ..st(3)
        };
        assert_eq!(pending_brightness(t, &failed), t);
        // The worker finished with it (or a newer one): the target is done.
        assert_eq!(pending_brightness(t, &st(4)), None);
        assert_eq!(pending_brightness(t, &st(5)), None);
        assert_eq!(pending_brightness(None, &st(0)), None);
    }

    #[test]
    fn brightness_base_ignores_a_stale_cache() {
        use std::time::Duration;
        let secs = |s| Some(Duration::from_secs(s));
        // A pending step wins.
        assert_eq!(
            brightness_base(Some((0.7, 2)), Some(0.5), secs(60)),
            Some(0.7)
        );
        // Fresh worker value is used, an old or unknown-age one is not.
        assert_eq!(brightness_base(None, Some(0.5), secs(1)), Some(0.5));
        assert_eq!(brightness_base(None, Some(0.5), secs(3)), None);
        assert_eq!(brightness_base(None, Some(0.5), None), None);
        assert_eq!(brightness_base(None, None, secs(0)), None);
    }

    use super::*;
    use crate::config::Profile;
    use crate::engine::executor::SynthEvent;
    use crate::engine::resolve::app_blocked;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::mpsc::Receiver;

    #[derive(Default)]
    struct Fake {
        log: Vec<SynthEvent>,
        refuse_up: Rc<Cell<bool>>,
        refuse_down: Rc<Cell<bool>>,
    }

    impl InputSink for &mut Fake {
        fn send(&mut self, ev: SynthEvent) -> bool {
            let refused = match ev {
                SynthEvent::Up(_) => self.refuse_up.get(),
                SynthEvent::Down(_) => self.refuse_down.get(),
                SynthEvent::Move(..) | SynthEvent::Wheel(..) => false,
            };
            if !refused {
                self.log.push(ev);
            }
            !refused
        }
    }

    const START: Action = Action::Debug(GestureEvent::TouchStart { fingers: 3 });

    fn profile(exe: &str, enabled: bool) -> Profile {
        Profile {
            exe_name: exe.to_owned(),
            enabled,
            ..Profile::default()
        }
    }

    fn setup() -> (Dispatcher, Receiver<WinCmd>) {
        let (tx, rx) = channel();
        (Dispatcher::with_channel(tx), rx)
    }

    const CURSOR: fn() -> Option<(i32, i32)> = || Some((100, 200));

    #[test]
    fn per_app_filter_is_case_insensitive_and_only_for_disabled_profiles() {
        let list = [profile("Game.EXE", false), profile("code.exe", true)];
        assert!(app_blocked(&list, Some("game.exe")));
        assert!(!app_blocked(&list, Some("code.exe")));
        assert!(!app_blocked(&list, Some("other.exe")));
        assert!(!app_blocked(&list, None));
        assert!(!app_blocked(&[], Some("game.exe")));
    }

    #[test]
    fn sub_pixels_accumulate_and_nothing_is_lost() {
        let mut s = SubPixel::default();
        let mut total = (0, 0);
        // Binary fractions are exact in f32, so the totals are exact too.
        for _ in 0..8 {
            let (x, y) = s.take(0.25, -0.375);
            total.0 += x;
            total.1 += y;
        }
        assert_eq!(total, (2, -3));
        let mut s = SubPixel::default();
        assert_eq!(s.take(0.75, 0.0), (0, 0));
        assert_eq!(s.take(0.75, 0.0), (1, 0));
        s.clear();
        assert_eq!(s.take(0.25, 0.0), (0, 0));
    }

    #[test]
    fn plans_for_fixed_and_custom_actions() {
        use config::Action as A;
        assert_eq!(
            plan_for(&A::TaskView),
            Plan::Keys(vec![Key::Vk(0x5B), Key::Vk(0x09)])
        );
        assert_eq!(
            plan_for(&A::ShowDesktop),
            Plan::Keys(vec![Key::Vk(0x5B), Key::Vk(0x44)])
        );
        assert_eq!(
            plan_for(&A::DesktopPrev),
            Plan::Keys(vec![Key::Vk(0x11), Key::Vk(0x5B), Key::Vk(0x25)])
        );
        assert_eq!(
            plan_for(&A::DesktopNext),
            Plan::Keys(vec![Key::Vk(0x11), Key::Vk(0x5B), Key::Vk(0x27)])
        );
        assert_eq!(
            plan_for(&A::AppSwitch),
            Plan::Keys(vec![Key::Vk(0x12), Key::Vk(0x09)])
        );
        assert_eq!(
            plan_for(&A::TabSwitch),
            Plan::Keys(vec![Key::Vk(0x11), Key::Vk(0x09)])
        );
        assert_eq!(
            plan_for(&A::Find),
            Plan::Keys(vec![Key::Vk(0x11), Key::Vk(0x46)])
        );
        assert_eq!(plan_for(&A::MiddleClick), Plan::Click(MouseButton::Middle));
        assert_eq!(plan_for(&A::RightClick), Plan::Click(MouseButton::Right));
        assert_eq!(
            plan_for(&A::Keys("Ctrl+Shift+T".into())),
            Plan::Keys(vec![Key::Vk(0x11), Key::Vk(0x10), Key::Vk(0x54)])
        );
        assert!(matches!(
            plan_for(&A::Keys("Ctrl+".into())),
            Plan::Invalid(_)
        ));
        assert_eq!(plan_for(&A::VolumeStep(5)), Plan::Volume(5));
        assert_eq!(plan_for(&A::BrightnessStep(-5)), Plan::Brightness(-5));
        assert_eq!(plan_for(&A::ToggleMute), Plan::ToggleMute);
        assert_eq!(plan_for(&A::None), Plan::Nothing);
        assert_eq!(
            plan_for(&A::WindowArrange(config::WindowPreset::RightHalf)),
            Plan::Arrange(config::WindowPreset::RightHalf)
        );
        assert!(matches!(plan_for(&A::WindowZoom), Plan::Invalid(_)));
    }

    #[test]
    fn pinch_zoom_runs_begin_zoom_end_and_ends_even_in_practice() {
        let (mut d, rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&START, &mut ex, &th, CURSOR);
        assert!(
            d.handle(&Action::WindowZoomBegin, &mut ex, &th, CURSOR)
                .is_none()
        );
        d.handle(&Action::WindowZoom { ratio: 1.25 }, &mut ex, &th, CURSOR);
        d.handle(&Action::WindowZoomEnd, &mut ex, &th, CURSOR);
        assert_eq!(rx.try_recv(), Ok(WinCmd::BeginZoom { cursor: (100, 200) }));
        assert_eq!(rx.try_recv(), Ok(WinCmd::Zoom { ratio: 1.25 }));
        assert_eq!(rx.try_recv(), Ok(WinCmd::EndZoom));
        assert!(rx.try_recv().is_err());
        // An end without a begin sends nothing.
        d.handle(&Action::WindowZoomEnd, &mut ex, &th, CURSOR);
        assert!(rx.try_recv().is_err());
        // Practice mode: nothing begins and nothing is scaled; an arrange trigger is dropped too.
        d.set_dry_run(true);
        d.handle(&Action::WindowZoomBegin, &mut ex, &th, CURSOR);
        d.handle(&Action::WindowZoom { ratio: 2.0 }, &mut ex, &th, CURSOR);
        let note = d.handle(
            &Action::Trigger(config::Action::WindowArrange(
                config::WindowPreset::LeftHalf,
            )),
            &mut ex,
            &th,
            CURSOR,
        );
        assert!(note.is_none());
        d.handle(&Action::WindowZoomEnd, &mut ex, &th, CURSOR);
        assert!(rx.try_recv().is_err());
        // Practice switched on mid-pinch: scaling stops, but the end still reaches the worker.
        d.set_dry_run(false);
        d.handle(&Action::WindowZoomBegin, &mut ex, &th, CURSOR);
        d.set_dry_run(true);
        d.handle(&Action::WindowZoom { ratio: 1.1 }, &mut ex, &th, CURSOR);
        d.handle(&Action::WindowZoomEnd, &mut ex, &th, CURSOR);
        assert_eq!(rx.try_recv(), Ok(WinCmd::BeginZoom { cursor: (100, 200) }));
        assert_eq!(rx.try_recv(), Ok(WinCmd::EndZoom));
    }

    #[test]
    fn arrange_goes_to_the_worker_and_cancel_drops_a_zoom() {
        let (mut d, rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        let p = config::WindowPreset::CenterTwoThirds;
        d.handle(&START, &mut ex, &th, CURSOR);
        let note = d.handle(
            &Action::Trigger(config::Action::WindowArrange(p)),
            &mut ex,
            &th,
            CURSOR,
        );
        assert!(note.is_some_and(|n| n.contains("ウィンドウを中央に 2/3")));
        assert_eq!(rx.try_recv(), Ok(WinCmd::Arrange(p)));
        d.handle(&Action::WindowZoomBegin, &mut ex, &th, CURSOR);
        d.cancel();
        assert_eq!(rx.try_recv(), Ok(WinCmd::BeginZoom { cursor: (100, 200) }));
        assert_eq!(rx.try_recv(), Ok(WinCmd::Cancel));
    }

    #[test]
    fn drag_presses_moves_in_whole_pixels_and_releases() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        assert!(d.handle(&Action::DragBegin, &mut ex, &th, CURSOR).is_some());
        let mv = Action::DragMove {
            dx_mm: 0.05,
            dy_mm: 0.0,
        };
        // 0.05 mm * 12 px/mm = 0.6 px: nothing at first, then one pixel.
        d.handle(&mv, &mut ex, &th, CURSOR);
        d.handle(&mv, &mut ex, &th, CURSOR);
        assert!(d.handle(&Action::DragEnd, &mut ex, &th, CURSOR).is_some());
        assert!(ex.held().is_empty());
        drop(ex);
        assert_eq!(
            f.log,
            vec![
                SynthEvent::Down(LEFT),
                SynthEvent::Move(1, 0),
                SynthEvent::Up(LEFT)
            ]
        );
    }

    #[test]
    fn blocked_app_drops_everything_but_ends() {
        let (mut d, rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.set_blocked(true);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        d.handle(&Action::WindowMoveBegin, &mut ex, &th, CURSOR);
        d.handle(
            &Action::Trigger(config::Action::ShowDesktop),
            &mut ex,
            &th,
            CURSOR,
        );
        assert!(ex.held().is_empty());
        assert!(rx.try_recv().is_err());
        // A drag that began before the app changed is still closed.
        d.set_blocked(false);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        d.set_blocked(true);
        d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        assert!(ex.held().is_empty());
        drop(ex);
        assert_eq!(
            f.log,
            vec![SynthEvent::Down(LEFT), SynthEvent::Up(LEFT)],
            "no input was sent for the blocked session"
        );
    }

    #[test]
    fn corner_resize_runs_begin_move_end_against_the_opposite_corner() {
        use crate::os::window::Corner;
        let (mut d, rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(
            &Action::CornerResizeBegin { corner: 0 },
            &mut ex,
            &th,
            CURSOR,
        );
        d.handle(
            &Action::CornerResizeMove {
                dx_mm: 2.0,
                dy_mm: -1.0,
            },
            &mut ex,
            &th,
            CURSOR,
        );
        d.handle(&Action::CornerResizeEnd, &mut ex, &th, CURSOR);
        // A second End is a no-op.
        d.handle(&Action::CornerResizeEnd, &mut ex, &th, CURSOR);
        let cmds: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                cmds.first(),
                Some(WinCmd::BeginResize {
                    corner: Corner::BottomRight,
                    cursor: (100, 200)
                })
            ),
            "{cmds:?}"
        );
        assert!(matches!(cmds.get(1), Some(WinCmd::Resize { dx, dy }) if *dx > 0 && *dy < 0));
        assert!(matches!(cmds.get(2), Some(WinCmd::EndResize)));
        assert_eq!(cmds.len(), 3);
    }

    #[test]
    fn corner_resize_is_not_started_when_blocked_or_dry_but_always_ends() {
        let th = Thresholds::default();
        for dry in [false, true] {
            let (mut d, rx) = setup();
            let mut f = Fake::default();
            let mut ex = Executor::new(&mut f);
            d.handle(
                &Action::CornerResizeBegin { corner: 3 },
                &mut ex,
                &th,
                CURSOR,
            );
            assert!(matches!(rx.try_recv(), Ok(WinCmd::BeginResize { .. })));
            // The app changes (or practice starts) mid-gesture: the resize is still closed.
            if dry {
                d.set_dry_run(true);
            } else {
                d.set_blocked(true);
                // The block is latched at the start of the next touch session.
                d.handle(&START, &mut ex, &th, CURSOR);
            }
            d.handle(
                &Action::CornerResizeMove {
                    dx_mm: 3.0,
                    dy_mm: 3.0,
                },
                &mut ex,
                &th,
                CURSOR,
            );
            d.handle(&Action::CornerResizeEnd, &mut ex, &th, CURSOR);
            assert!(matches!(rx.try_recv(), Ok(WinCmd::EndResize)), "dry={dry}");
            assert!(rx.try_recv().is_err());
            // A new one does not start.
            d.handle(
                &Action::CornerResizeBegin { corner: 3 },
                &mut ex,
                &th,
                CURSOR,
            );
            assert!(rx.try_recv().is_err(), "dry={dry}");
        }
    }

    #[test]
    fn practice_mode_runs_nothing_new_but_still_closes_what_began() {
        let (mut d, rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        let mv = Action::DragMove {
            dx_mm: 1.0,
            dy_mm: 0.0,
        };
        d.set_dry_run(true);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        d.handle(&mv, &mut ex, &th, CURSOR);
        d.handle(&Action::WindowMoveBegin, &mut ex, &th, CURSOR);
        d.handle(&Action::WindowResizeBegin, &mut ex, &th, CURSOR);
        d.handle(
            &Action::WindowResizeMove {
                dx_mm: 1.0,
                dy_mm: 0.0,
            },
            &mut ex,
            &th,
            CURSOR,
        );
        let note = d.handle(
            &Action::Trigger(config::Action::ShowDesktop),
            &mut ex,
            &th,
            CURSOR,
        );
        assert!(note.is_none());
        assert!(ex.held().is_empty());
        assert!(rx.try_recv().is_err());
        // Practice mode switched on in the middle of a drag: the end still lets go.
        d.set_dry_run(false);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        d.set_dry_run(true);
        d.handle(&mv, &mut ex, &th, CURSOR);
        d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        assert!(ex.held().is_empty());
        drop(ex);
        assert_eq!(f.log, vec![SynthEvent::Down(LEFT), SynthEvent::Up(LEFT)]);
        // Off again: triggers run.
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.set_dry_run(false);
        assert!(
            d.handle(
                &Action::Trigger(config::Action::ShowDesktop),
                &mut ex,
                &th,
                CURSOR
            )
            .is_some()
        );
    }

    #[test]
    fn scroll_is_sent_as_a_wheel_and_dropped_for_a_blocked_app() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&START, &mut ex, &th, CURSOR);
        assert!(
            d.handle(&Action::Scroll { h: 3, v: -120 }, &mut ex, &th, CURSOR)
                .is_none()
        );
        d.set_blocked(true);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::Scroll { h: 0, v: 50 }, &mut ex, &th, CURSOR);
        drop(ex);
        assert_eq!(f.log, vec![SynthEvent::Wheel(3, -120)]);
    }

    #[test]
    fn practice_mode_never_scrolls() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.set_dry_run(true);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::Scroll { h: 0, v: 120 }, &mut ex, &th, CURSOR);
        d.set_dry_run(false);
        d.handle(&Action::Scroll { h: 0, v: -120 }, &mut ex, &th, CURSOR);
        drop(ex);
        assert_eq!(f.log, vec![SynthEvent::Wheel(0, -120)]);
    }

    #[test]
    fn the_unavailable_card_is_told_once_per_slide() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        assert!(d.report_level(BrightnessReport::Nothing).is_none());
        let note = d.report_level(BrightnessReport::Unavailable).unwrap();
        assert!(note.contains("明るさを変えられません"));
        // The later steps of the same slide say nothing more, even a late level report.
        assert!(d.report_level(BrightnessReport::Unavailable).is_none());
        assert!(d.report_level(BrightnessReport::Level(0.5)).is_none());
        // The slide ends (the freeze lets go): the next slide may tell again.
        d.handle(&Action::Suppress(Suppress::Off), &mut ex, &th, CURSOR);
        assert!(d.report_level(BrightnessReport::Unavailable).is_some());
    }

    #[test]
    fn a_practice_edge_step_touches_no_monitor() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.set_dry_run(true);
        let step = Action::EdgeStep {
            side: EdgeSide::Right,
            kind: config::EdgeKind::Brightness,
            forward: true,
            outs: vec![config::EdgeOut::Act(config::Action::BrightnessStep(5))],
        };
        assert!(d.handle(&step, &mut ex, &th, CURSOR).is_none());
        assert!(d.take_level_step().is_none());
        assert!(d.sys.brightness.is_none());
    }

    #[test]
    fn drag_move_without_a_held_button_is_ignored() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        let mv = Action::DragMove {
            dx_mm: 5.0,
            dy_mm: 5.0,
        };
        d.handle(&mv, &mut ex, &th, CURSOR);
        drop(ex);
        assert!(f.log.is_empty());
    }

    /// A drag that began where `caption` says, ended and flicked; returns the flick's note and
    /// whether any key was sent for it.
    fn flick_after_drag(
        caption: fn(i32, i32) -> bool,
        blocked: bool,
        dry_run: bool,
    ) -> (Option<String>, bool) {
        let (mut d, _rx) = setup();
        d.caption_at = caption;
        d.set_blocked(blocked);
        d.set_dry_run(dry_run);
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        let note = d.handle(
            &Action::DragFlick(config::Action::TaskView),
            &mut ex,
            &th,
            CURSOR,
        );
        let held = ex.held().len();
        drop(ex);
        assert_eq!(held, 0);
        let keys = f
            .log
            .iter()
            .any(|e| matches!(e, SynthEvent::Down(Key::Vk(0x09))));
        (note, keys)
    }

    #[test]
    fn a_flick_runs_only_when_the_drag_began_on_a_title_bar() {
        let (note, keys) = flick_after_drag(|_, _| true, false, false);
        assert_eq!(note.as_deref(), Some("タイトルバーで払う → タスクビュー"));
        assert!(keys);
        let (note, keys) = flick_after_drag(|_, _| false, false, false);
        assert_eq!(note, None);
        assert!(!keys);
    }

    #[test]
    fn the_caption_is_asked_at_the_cursor_before_the_button_goes_down() {
        let (mut d, _rx) = setup();
        // Only the cursor point (100, 200) is a title bar.
        d.caption_at = |x, y| (x, y) == (100, 200);
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        assert!(d.drag_on_caption);
        d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, || Some((1, 1)));
        assert!(!d.drag_on_caption);
        d.handle(&Action::DragBegin, &mut ex, &th, || None);
        assert!(!d.drag_on_caption);
    }

    #[test]
    fn a_flick_is_blocked_by_pause_and_practice() {
        assert_eq!(flick_after_drag(|_, _| true, true, false), (None, false));
        assert_eq!(flick_after_drag(|_, _| true, false, true), (None, false));
    }

    #[test]
    fn a_title_bar_drag_moves_at_the_window_speed() {
        let moved = |caption: fn(i32, i32) -> bool| {
            let (mut d, _rx) = setup();
            d.caption_at = caption;
            let th = Thresholds {
                drag_px_per_mm: 10.0,
                window_px_per_mm: 50.0,
                ..Thresholds::default()
            };
            let mut f = Fake::default();
            let mut ex = Executor::new(&mut f);
            d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
            let mv = Action::DragMove {
                dx_mm: 1.0,
                dy_mm: 0.0,
            };
            d.handle(&mv, &mut ex, &th, CURSOR);
            drop(ex);
            f.log
                .iter()
                .find_map(|e| match e {
                    SynthEvent::Move(dx, _) => Some(*dx),
                    _ => None,
                })
                .unwrap_or(0)
        };
        assert_eq!(moved(|_, _| true), 50);
        assert_eq!(moved(|_, _| false), 10);
    }

    #[test]
    fn a_flick_is_used_up_by_one_run() {
        let (mut d, _rx) = setup();
        d.caption_at = |_, _| true;
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        let flick = Action::DragFlick(config::Action::ShowDesktop);
        assert!(d.handle(&flick, &mut ex, &th, CURSOR).is_some());
        assert!(d.handle(&flick, &mut ex, &th, CURSOR).is_none());
    }

    #[test]
    fn trigger_sends_keys_and_clicks_and_names_the_gesture() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(
            &Action::Debug(GestureEvent::Multi {
                fingers: 5,
                kind: MultiKind::PinchIn,
            }),
            &mut ex,
            &th,
            CURSOR,
        );
        let note = d
            .handle(
                &Action::Trigger(config::Action::TaskView),
                &mut ex,
                &th,
                CURSOR,
            )
            .unwrap();
        assert_eq!(note, "5本指ピンチイン → タスクビュー");
        d.handle(
            &Action::Trigger(config::Action::RightClick),
            &mut ex,
            &th,
            CURSOR,
        );
        let bad = d
            .handle(
                &Action::Trigger(config::Action::Keys("Ctrl+".into())),
                &mut ex,
                &th,
                CURSOR,
            )
            .unwrap();
        assert!(bad.contains("送れません"), "{bad}");
        assert!(ex.held().is_empty());
        drop(ex);
        let rmb = Key::Mouse(MouseButton::Right);
        assert_eq!(
            f.log,
            vec![
                SynthEvent::Down(Key::Vk(0x5B)),
                SynthEvent::Down(Key::Vk(0x09)),
                SynthEvent::Up(Key::Vk(0x09)),
                SynthEvent::Up(Key::Vk(0x5B)),
                SynthEvent::Down(rmb),
                SynthEvent::Up(rmb),
            ]
        );
    }

    #[test]
    fn app_change_mid_session_does_not_touch_the_running_drag() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        let mv = Action::DragMove {
            dx_mm: 1.0,
            dy_mm: 0.0,
        };
        d.set_blocked(false);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        // The foreground poll flips to a blocked app in the middle of the session.
        d.set_blocked(true);
        d.handle(&mv, &mut ex, &th, CURSOR);
        d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        // The next session picks the new decision up.
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        drop(ex);
        assert_eq!(
            f.log,
            vec![
                SynthEvent::Down(LEFT),
                SynthEvent::Move(12, 0),
                SynthEvent::Up(LEFT)
            ]
        );
    }

    #[test]
    fn refused_release_is_reported_and_retried() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let refuse = f.refuse_up.clone();
        let mut ex = Executor::new(&mut f);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        refuse.set(true);
        let note = d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        assert_eq!(note.as_deref(), Some(RELEASE_FAILED_NOTE));
        assert_eq!(ex.held(), &[LEFT], "still owned, so it can be retried");
        // Retrying while the system still refuses keeps it owned and says nothing new.
        assert!(d.retry_release(&mut ex).is_none());
        assert_eq!(ex.held(), &[LEFT]);
        refuse.set(false);
        let note = d.retry_release(&mut ex);
        assert_eq!(note.as_deref(), Some(RELEASED_LATER_NOTE));
        assert!(ex.held().is_empty());
        assert!(d.retry_release(&mut ex).is_none());
    }

    #[test]
    fn retry_leaves_a_button_alone_while_a_drag_holds_it() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        assert!(d.retry_release(&mut ex).is_none());
        assert_eq!(ex.held(), &[LEFT]);
    }

    #[test]
    fn release_everything_while_dragging_lets_go_or_keeps_trying() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let refuse = f.refuse_up.clone();
        let mut ex = Executor::new(&mut f);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        assert!(d.release_everything(&mut ex).is_none());
        assert!(ex.held().is_empty());

        // Refused: the button is not orphaned, the retry picks it up and the note says why.
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        refuse.set(true);
        let note = d.release_everything(&mut ex);
        assert_eq!(note.as_deref(), Some(RELEASE_FAILED_NOTE));
        assert_eq!(ex.held(), &[LEFT]);
        refuse.set(false);
        assert_eq!(
            d.retry_release(&mut ex).as_deref(),
            Some(RELEASED_LATER_NOTE)
        );
        assert!(ex.held().is_empty());
    }

    #[test]
    fn drag_begin_over_a_stuck_button_does_not_claim_a_start() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let refuse = f.refuse_up.clone();
        let mut ex = Executor::new(&mut f);
        d.handle(&START, &mut ex, &th, CURSOR);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        refuse.set(true);
        d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        // The next drag begins while the old down is still owned and the up is still refused.
        d.handle(&START, &mut ex, &th, CURSOR);
        let note = d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        assert_eq!(note.as_deref(), Some(SEND_FAILED_NOTE));
        // Once the system accepts input again the drag gets a real down.
        refuse.set(false);
        d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        let note = d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        assert_eq!(note.as_deref(), Some("ドラッグ開始"));
        drop(ex);
        let downs = f
            .log
            .iter()
            .filter(|e| matches!(e, SynthEvent::Down(_)))
            .count();
        let ups = f
            .log
            .iter()
            .filter(|e| matches!(e, SynthEvent::Up(_)))
            .count();
        assert_eq!((downs, ups), (2, 2), "{:?}", f.log);
    }

    #[test]
    fn a_resumed_drag_without_a_button_presses_once_per_session() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let refuse = f.refuse_down.clone();
        let mut ex = Executor::new(&mut f);
        let mv = Action::DragMove {
            dx_mm: 1.0,
            dy_mm: 0.0,
        };
        refuse.set(true);
        d.handle(&START, &mut ex, &th, CURSOR);
        let note = d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        assert_eq!(note.as_deref(), Some(SEND_FAILED_NOTE));
        // Same session: no second try, and no movement without the button.
        assert!(d.handle(&mv, &mut ex, &th, CURSOR).is_none());
        assert!(ex.held().is_empty());
        // Fingers lift and return in time (no DragEnd, no DragBegin): the new session tries once.
        refuse.set(false);
        d.handle(&START, &mut ex, &th, CURSOR);
        assert!(d.handle(&mv, &mut ex, &th, CURSOR).is_none());
        assert_eq!(ex.held(), &[LEFT]);
        d.handle(&Action::DragEnd, &mut ex, &th, CURSOR);
        drop(ex);
        assert_eq!(
            f.log,
            vec![
                SynthEvent::Down(LEFT),
                SynthEvent::Move(12, 0),
                SynthEvent::Up(LEFT)
            ]
        );
    }

    #[test]
    fn a_resume_press_that_is_refused_is_reported() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let refuse = f.refuse_down.clone();
        let mut ex = Executor::new(&mut f);
        d.handle(&START, &mut ex, &th, CURSOR);
        refuse.set(true);
        d.handle(&Action::DragBegin, &mut ex, &th, CURSOR);
        d.handle(&START, &mut ex, &th, CURSOR);
        let mv = Action::DragMove {
            dx_mm: 1.0,
            dy_mm: 0.0,
        };
        let note = d.handle(&mv, &mut ex, &th, CURSOR);
        assert_eq!(note.as_deref(), Some(SEND_FAILED_NOTE));
        // Only one attempt per session.
        assert!(d.handle(&mv, &mut ex, &th, CURSOR).is_none());
    }

    #[test]
    fn ring_without_cursor_is_cancelled_not_opened_at_the_corner() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        let open = Action::RingOpen {
            labels: vec!["a".into()],
            actions: vec![config::Action::None],
        };
        let note = d.handle(&open, &mut ex, &th, || None);
        assert_eq!(note.as_deref(), Some(NO_CURSOR_NOTE));
        assert!(!d.ring.open);
        assert!(d.take_overlay_failed());
        // With a position it opens normally.
        d.handle(&open, &mut ex, &th, || Some((5, 6)));
        assert!(d.ring.open);
        assert!(!d.take_overlay_failed());
    }

    #[test]
    fn window_move_without_cursor_or_worker_says_so() {
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);

        let (mut d, rx) = setup();
        let note = d.handle(&Action::WindowMoveBegin, &mut ex, &th, || None);
        assert_eq!(note.as_deref(), Some(NO_CURSOR_NOTE));
        assert!(rx.try_recv().is_err(), "nothing was sent to the worker");
        // The move never began, so its end is silent.
        assert!(
            d.handle(&Action::WindowMoveEnd { snap: None }, &mut ex, &th, CURSOR)
                .is_none()
        );

        // A dead worker (receiver gone).
        let (tx, rx) = channel();
        drop(rx);
        let mut d = Dispatcher::with_channel(tx);
        let note = d.handle(&Action::WindowMoveBegin, &mut ex, &th, CURSOR);
        assert_eq!(note.as_deref(), Some(WORKER_NOTE));

        // No worker at all.
        let mut d = Dispatcher::new(None);
        let note = d.handle(&Action::WindowMoveBegin, &mut ex, &th, CURSOR);
        assert_eq!(note.as_deref(), Some(WORKER_NOTE));
    }

    #[test]
    fn window_move_goes_to_the_worker_with_pixels_and_snap() {
        let (mut d, rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&Action::WindowMoveBegin, &mut ex, &th, CURSOR);
        d.handle(
            &Action::WindowMove {
                dx_mm: 1.0,
                dy_mm: -0.5,
            },
            &mut ex,
            &th,
            CURSOR,
        );
        // The worker reports the real outcome; the dispatcher writes nothing itself.
        assert!(
            d.handle(
                &Action::WindowMoveEnd {
                    snap: Some(SnapDir::Left),
                },
                &mut ex,
                &th,
                CURSOR,
            )
            .is_none()
        );
        // A stray end after the move is over does nothing.
        assert!(
            d.handle(&Action::WindowMoveEnd { snap: None }, &mut ex, &th, CURSOR)
                .is_none()
        );
        let cmds: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            cmds,
            vec![
                WinCmd::Begin { cursor: (100, 200) },
                WinCmd::Move { dx: 12, dy: -6 },
                WinCmd::End {
                    snap: Some(SnapDir::Left)
                },
            ]
        );
    }

    #[test]
    fn cancel_drops_a_window_move_without_snapping() {
        let (mut d, rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&Action::WindowMoveBegin, &mut ex, &th, CURSOR);
        d.cancel();
        let cmds: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            cmds,
            vec![WinCmd::Begin { cursor: (100, 200) }, WinCmd::Cancel]
        );
    }

    const ALT: Key = Key::Vk(0x12);
    const TAB: Key = Key::Vk(0x09);
    const SHIFT: Key = Key::Vk(0x10);
    const ESC: Key = Key::Vk(0x1B);

    fn down(k: Key) -> SynthEvent {
        SynthEvent::Down(k)
    }
    fn up(k: Key) -> SynthEvent {
        SynthEvent::Up(k)
    }

    #[test]
    fn scrub_holds_alt_tabs_both_ways_and_lets_go_at_the_end() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&START, &mut ex, &th, CURSOR);
        assert!(
            d.handle(&Action::AppSwitchBegin, &mut ex, &th, CURSOR)
                .is_some()
        );
        assert!(d.scrub_active());
        assert_eq!(ex.held(), &[ALT]);
        d.handle(
            &Action::AppSwitchStep { forward: true },
            &mut ex,
            &th,
            CURSOR,
        );
        d.handle(
            &Action::AppSwitchStep { forward: false },
            &mut ex,
            &th,
            CURSOR,
        );
        // The idle retry must not let go of the Alt a gesture is holding.
        assert_eq!(d.retry_release(&mut ex), None);
        assert_eq!(ex.held(), &[ALT]);
        d.handle(
            &Action::AppSwitchEnd { cancel: false },
            &mut ex,
            &th,
            CURSOR,
        );
        assert!(!d.scrub_active() && ex.held().is_empty());
        drop(ex);
        assert_eq!(
            f.log,
            vec![
                down(ALT),
                down(TAB),
                up(TAB),
                down(TAB),
                up(TAB),
                down(SHIFT),
                down(TAB),
                up(TAB),
                up(SHIFT),
                up(ALT),
            ]
        );
    }

    #[test]
    fn a_cancelled_scrub_sends_esc_before_alt_goes_up() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&Action::AppSwitchBegin, &mut ex, &th, CURSOR);
        d.handle(&Action::AppSwitchEnd { cancel: true }, &mut ex, &th, CURSOR);
        assert!(ex.held().is_empty());
        drop(ex);
        assert_eq!(
            f.log,
            vec![down(ALT), down(TAB), up(TAB), down(ESC), up(ESC), up(ALT)]
        );
    }

    #[test]
    fn release_everything_backs_a_scrub_out_with_esc_and_lets_go_of_alt() {
        // Pause, config change, disable, device removed and shutdown all end here.
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&Action::AppSwitchBegin, &mut ex, &th, CURSOR);
        assert_eq!(d.release_everything(&mut ex), None);
        assert!(!d.scrub_active() && ex.held().is_empty());
        // The end that may still arrive later does nothing.
        d.handle(
            &Action::AppSwitchEnd { cancel: false },
            &mut ex,
            &th,
            CURSOR,
        );
        drop(ex);
        assert_eq!(f.log.last(), Some(&up(ALT)));
        assert!(f.log.contains(&down(ESC)));
        assert_eq!(f.log.iter().filter(|e| **e == up(ALT)).count(), 1);
    }

    #[test]
    fn dropping_the_executor_lets_go_of_a_held_alt() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(&Action::AppSwitchBegin, &mut ex, &th, CURSOR);
        // A panic unwinds through here.
        drop(ex);
        assert_eq!(f.log.last(), Some(&up(ALT)));
    }

    #[test]
    fn a_refused_alt_up_is_retried_after_the_scrub_ended() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let refuse = f.refuse_up.clone();
        let mut ex = Executor::new(&mut f);
        d.handle(&Action::AppSwitchBegin, &mut ex, &th, CURSOR);
        refuse.set(true);
        assert!(
            d.handle(
                &Action::AppSwitchEnd { cancel: false },
                &mut ex,
                &th,
                CURSOR
            )
            .is_some()
        );
        assert_eq!(ex.held(), &[ALT]);
        refuse.set(false);
        assert!(d.retry_release(&mut ex).is_some());
        assert!(ex.held().is_empty());
    }

    #[test]
    fn a_refused_key_during_a_step_ends_the_scrub_instead_of_turning_it_around() {
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let refuse = f.refuse_up.clone();
        let mut ex = Executor::new(&mut f);
        d.handle(&Action::AppSwitchBegin, &mut ex, &th, CURSOR);
        refuse.set(true);
        assert!(
            d.handle(
                &Action::AppSwitchStep { forward: false },
                &mut ex,
                &th,
                CURSOR
            )
            .is_some()
        );
        assert!(!d.scrub_active());
        refuse.set(false);
        d.retry_release(&mut ex);
        assert!(ex.held().is_empty());
        // Later steps of the same touch send nothing.
        let before = ex.held().len();
        d.handle(
            &Action::AppSwitchStep { forward: true },
            &mut ex,
            &th,
            CURSOR,
        );
        assert_eq!(ex.held().len(), before);
        drop(ex);
        // The begin's Tab and the refused step's Tab, nothing after.
        assert_eq!(f.log.iter().filter(|e| **e == down(TAB)).count(), 2);
    }

    #[test]
    fn practice_and_blocked_apps_send_no_key_for_a_scrub() {
        let th = Thresholds::default();
        for blocked in [false, true] {
            let (mut d, _rx) = setup();
            let mut f = Fake::default();
            let mut ex = Executor::new(&mut f);
            if blocked {
                d.set_blocked(true);
            } else {
                d.set_dry_run(true);
            }
            d.handle(&START, &mut ex, &th, CURSOR);
            d.handle(&Action::AppSwitchBegin, &mut ex, &th, CURSOR);
            d.handle(
                &Action::AppSwitchStep { forward: true },
                &mut ex,
                &th,
                CURSOR,
            );
            d.handle(
                &Action::AppSwitchEnd { cancel: false },
                &mut ex,
                &th,
                CURSOR,
            );
            assert!(!d.scrub_active());
            drop(ex);
            assert!(f.log.is_empty(), "blocked={blocked}: {:?}", f.log);
        }
    }

    #[test]
    fn the_scrub_action_outside_a_swipe_is_one_alt_tab() {
        assert_eq!(
            plan_for(&config::Action::AppSwitchScrub),
            plan_for(&config::Action::AppSwitch)
        );
        let (mut d, _rx) = setup();
        let th = Thresholds::default();
        let mut f = Fake::default();
        let mut ex = Executor::new(&mut f);
        d.handle(
            &Action::Trigger(config::Action::AppSwitchScrub),
            &mut ex,
            &th,
            CURSOR,
        );
        assert!(ex.held().is_empty());
        drop(ex);
        assert_eq!(f.log, vec![down(ALT), down(TAB), up(TAB), up(ALT)]);
    }
}
