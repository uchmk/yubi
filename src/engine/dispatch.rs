//! Dispatch: turns the recognizer's actions into effects.
//!
//! Mouse buttons and keys go through the executor (which owns everything held down). Windows are
//! moved by a dedicated worker thread fed by a channel, so a slow or hung target window can never
//! stall the engine thread. The per-app filter is a pure function over the profile list.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use super::executor::{Executor, InputSink, Key, MouseButton};
use super::recognizer::{Action, GestureEvent, SnapDir, Thresholds};
use super::types::MultiKind;
use crate::config;
use crate::os::keys::KeyCombo;

pub const ELEVATED_NOTE: &str = "管理者のウィンドウは動かせません";
const SEND_FAILED_NOTE: &str = "入力を送れませんでした（管理者のウィンドウが前面かもしれません）";
pub const RELEASE_FAILED_NOTE: &str = "ボタンを離せませんでした（再試行します）";
const RELEASED_LATER_NOTE: &str = "ボタンを離しました";
pub const WORKER_NOTE: &str = "ウィンドウ操作を開始できませんでした";
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

pub fn plan_for(action: &config::Action) -> Plan {
    use config::Action as A;
    match action {
        A::TaskView => keys_plan("Win+Tab"),
        A::ShowDesktop => keys_plan("Win+D"),
        A::DesktopPrev => keys_plan("Ctrl+Win+Left"),
        A::DesktopNext => keys_plan("Ctrl+Win+Right"),
        A::AppSwitch => keys_plan("Alt+Tab"),
        A::TabSwitch => keys_plan("Ctrl+Tab"),
        A::Find => keys_plan("Ctrl+F"),
        A::MiddleClick => Plan::Click(MouseButton::Middle),
        A::RightClick => Plan::Click(MouseButton::Right),
        A::Keys(s) => keys_plan(s),
        A::VolumeStep(n) => Plan::Volume(*n),
        A::BrightnessStep(n) => Plan::Brightness(*n),
        A::ToggleMute => Plan::ToggleMute,
        // The ring menu comes with M3c.
        A::RingMenu | A::None => Plan::Nothing,
    }
}

/// Commands for the window-ops worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Drop the move without snapping (device gone, config changed, ...).
    Cancel,
}

pub struct Dispatcher {
    /// Per-app filter result, latched at the start of each touch session.
    blocked: bool,
    /// Latest per-app result from the foreground poll; takes effect at the next touch start.
    live_blocked: bool,
    /// The recognizer has a drag open (begun, not yet ended).
    drag_open: bool,
    /// The left button was already (re)tried this touch session.
    press_tried: bool,
    /// A release was refused and not yet reported as fixed.
    release_pending: bool,
    drag_px: SubPixel,
    win_px: SubPixel,
    win_tx: Option<Sender<WinCmd>>,
    win_join: Option<JoinHandle<()>>,
    /// A window move was accepted and not yet ended.
    win_active: bool,
    last_multi: Option<(usize, MultiKind)>,
    /// Volume and brightness, opened on first use (engine thread only).
    sys: SystemOps,
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
            drag_open: false,
            press_tried: false,
            release_pending: false,
            drag_px: SubPixel::default(),
            win_px: SubPixel::default(),
            win_tx,
            win_join: None,
            win_active: false,
            last_multi: None,
            sys: SystemOps::default(),
        }
    }

    /// Record the foreground app's filter result. It takes effect at the next touch start, so
    /// the app is decided once per touch session.
    pub fn set_blocked(&mut self, blocked: bool) {
        self.live_blocked = blocked;
    }

    /// False when the worker is missing or has died.
    fn win(&self, cmd: WinCmd) -> bool {
        self.win_tx.as_ref().is_some_and(|tx| tx.send(cmd).is_ok())
    }

    /// Try again to let go of buttons the executor still owns that no gesture holds (an up was
    /// refused earlier). Returns a note when the retry finally worked.
    pub fn retry_release<S: InputSink>(&mut self, ex: &mut Executor<S>) -> Option<String> {
        if self.drag_open || ex.held().is_empty() {
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
        let stuck = ex.release_all();
        self.cancel();
        (stuck > 0).then(|| {
            self.release_pending = true;
            RELEASE_FAILED_NOTE.to_owned()
        })
    }

    /// Forget a half-done gesture (the executor lets go of buttons and keys on its own).
    pub fn cancel(&mut self) {
        if self.win_active {
            self.win(WinCmd::Cancel);
        }
        self.win_active = false;
        self.drag_open = false;
        self.drag_px.clear();
        self.win_px.clear();
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
            Action::Debug(_) => None,
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
            _ if self.blocked => None,
            Action::DragBegin => {
                self.drag_open = true;
                self.press_tried = true;
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
                    let (dx, dy) = self
                        .drag_px
                        .take(dx_mm * th.drag_px_per_mm, dy_mm * th.drag_px_per_mm);
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
            Action::Trigger(a) => self.trigger(a, ex),
        }
    }

    fn trigger<S: InputSink>(
        &mut self,
        action: &config::Action,
        ex: &mut Executor<S>,
    ) -> Option<String> {
        let source = match self.last_multi.take() {
            Some((n, kind)) => multi_label(n, kind),
            None => "ジェスチャー".to_owned(),
        };
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
                Ok(v) => format!("{source} → {label}（{}%）", percent(v)),
                Err(e) => e,
            }),
            Plan::ToggleMute => Some(match self.sys.toggle_mute() {
                Ok(muted) => format!(
                    "{source} → {}",
                    if muted {
                        "ミュート"
                    } else {
                        "ミュート解除"
                    }
                ),
                Err(e) => e,
            }),
            Plan::Brightness(n) => Some(match self.sys.brightness_step(n) {
                Ok(v) => format!("{source} → {label}（{}%）", percent(v)),
                Err(e) => e,
            }),
            Plan::Nothing => Some(format!("{source} → {label}（まだ動作しません）")),
            Plan::Invalid(msg) => Some(msg),
        }
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
const BRIGHTNESS_UNKNOWN_NOTE: &str = "明るさを読み取っています。もう一度お試しください";
const BRIGHTNESS_FAILED_NOTE: &str = "明るさを変えられませんでした";

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
    /// The brightness the last step asked for (the worker applies it later) and the worker's
    /// report generation when it was asked. It lives across gestures, so two quick gestures both
    /// count, until the worker reports back (see [`pending_brightness`]).
    brightness_target: Option<(f32, u64)>,
}

/// The step target that is still waiting for the worker, or `None` once the worker has reported:
/// a failed set, or the value we asked for. A report of an older value (a newer set is still
/// queued) keeps the target.
fn pending_brightness(
    target: Option<(f32, u64)>,
    status: &crate::os::brightness::WorkerStatus,
) -> Option<(f32, u64)> {
    let (v, generation) = target?;
    if status.generation == generation {
        return target;
    }
    let applied = status.value.is_some_and(|c| (c - v).abs() < 0.005);
    if status.error.is_none() && !applied {
        target
    } else {
        None
    }
}

impl SystemOps {
    fn volume_step(&mut self, n: i8) -> Result<f32, String> {
        with_volume(|v| v.step(step_fraction(n)))
    }

    fn toggle_mute(&mut self) -> Result<bool, String> {
        with_volume(|v| v.toggle_mute())
    }

    /// Never blocks: the worker thread talks to the display. The first call only asks for a read.
    fn brightness_step(&mut self, n: i8) -> Result<f32, String> {
        let worker = self
            .brightness
            .get_or_insert_with(crate::os::brightness::BrightnessWorker::spawn);
        let status = worker.status();
        self.brightness_target = pending_brightness(self.brightness_target, &status);
        let Some(current) = self.brightness_target.map(|t| t.0).or(status.value) else {
            worker.refresh();
            return Err(match status.error {
                Some(_) => BRIGHTNESS_FAILED_NOTE.to_owned(),
                None => BRIGHTNESS_UNKNOWN_NOTE.to_owned(),
            });
        };
        let v = crate::os::volume::step_value(current, step_fraction(n));
        worker.set(v);
        self.brightness_target = Some((v, status.generation));
        Ok(v)
    }
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

fn run_worker(rx: Receiver<WinCmd>, notify: impl Fn(String)) {
    use crate::os::window::{self, SnapDir as OsSnap, WindowMover};
    let mut mover: Option<WindowMover> = None;
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
    use super::pending_brightness;
    use crate::os::brightness::WorkerStatus;

    #[test]
    fn brightness_target_survives_until_the_worker_reports() {
        let st = |value, generation, failed: bool| WorkerStatus {
            value,
            error: failed.then_some(crate::os::brightness::BrightnessError::Unsupported),
            generation,
        };
        let t = Some((0.6, 4));
        // Nothing reported since the set: a second quick gesture steps from the target.
        assert_eq!(pending_brightness(t, &st(Some(0.5), 4, false)), t);
        // An earlier set landed while a newer one is queued: still pending.
        assert_eq!(pending_brightness(t, &st(Some(0.55), 5, false)), t);
        // The asked value was applied: the worker value is authoritative again.
        assert_eq!(pending_brightness(t, &st(Some(0.6), 5, false)), None);
        // A failed set drops the target.
        assert_eq!(pending_brightness(t, &st(Some(0.5), 5, true)), None);
        assert_eq!(pending_brightness(None, &st(None, 0, false)), None);
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
                SynthEvent::Move(..) => false,
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
}
