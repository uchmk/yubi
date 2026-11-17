//! Shared contracts of the recognizer: gesture ids, session ids, effect kinds, the actions the
//! recognizer emits, the thresholds it reads and the per-gesture settings latched at touchdown.
//!
//! Everything here is plain data. Recognition stays pure: frames, time and config in, actions out.

use std::fmt;

use super::gestures::press::PressCfg;
use super::resolve::{self, resolve, resolve_edge};
use crate::config::{self, Config, Sensitivity};

pub type Point = (f32, f32);

/// Stable token of a gesture ("tap.3", "swipe.4.left", "pinch.5.in"). These are the keys of the
/// per-app overrides in the schema v2 config, so the spelling must not change. Ids of gestures the
/// recognizer does not detect yet (press, hold, corner) are reserved here already.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GestureId(String);

impl GestureId {
    pub fn tap(fingers: usize) -> Self {
        Self(format!("tap.{fingers}"))
    }

    pub fn swipe(fingers: usize, dir: SwipeDir) -> Self {
        Self(format!("swipe.{fingers}.{}", dir.token()))
    }

    pub fn pinch(fingers: usize, inward: bool) -> Self {
        Self(format!(
            "pinch.{fingers}.{}",
            if inward { "in" } else { "out" }
        ))
    }

    /// Light or deep press with `fingers` fingers (reserved).
    pub fn press(fingers: usize, deep: bool) -> Self {
        Self(format!(
            "press.{fingers}.{}",
            if deep { "deep" } else { "light" }
        ))
    }

    /// The long hold with `fingers` fingers (1 to 5).
    pub fn long_hold(fingers: usize) -> Self {
        Self(format!("hold.long.{fingers}"))
    }

    /// A swipe starting in a pad corner (0 = top-left, 1 = top-right, 2 = bottom-left,
    /// 3 = bottom-right) along `dir`.
    /// Any slot of a pad corner: a swipe direction, or the tap / press / hold slot.
    pub fn corner_slot(index: usize, slot: config::CornerSlot) -> Self {
        Self(format!(
            "corner.{}.{}",
            CORNER_TOKENS[index % 4],
            slot.token()
        ))
    }

    /// The flick at the end of a title-bar drag (`left`, `right`, `up`, `down`).
    pub fn drag_flick(dir: &str) -> Self {
        Self(format!("drag.flick.{dir}"))
    }

    /// A finger-hold or thumb-hold combo (reserved).
    pub fn hold_combo(
        thumb_hold: bool,
        hold: config::Finger,
        trigger: config::HoldTrigger,
    ) -> Self {
        Self(format!(
            "hold.{}.{}.{}",
            if thumb_hold { "thumb" } else { "finger" },
            hold.token(),
            trigger.token()
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Every id the config can bind, in a fixed order.
    pub fn all() -> Vec<Self> {
        let mut out = Vec::new();
        for n in 3..=5 {
            out.push(Self::tap(n));
        }
        for n in 3..=5 {
            for d in SwipeDir::ALL {
                out.push(Self::swipe(n, d));
            }
            out.push(Self::pinch(n, true));
            out.push(Self::pinch(n, false));
        }
        for d in SwipeDir::ALL {
            out.push(Self::drag_flick(d.token()));
        }
        for n in 1..=5 {
            out.push(Self::press(n, false));
            out.push(Self::press(n, true));
        }
        for n in 1..=5 {
            out.push(Self::long_hold(n));
        }
        for i in 0..4 {
            for slot in config::CornerSlot::ALL {
                out.push(Self::corner_slot(i, slot));
            }
        }
        for thumb in [false, true] {
            for f in config::Finger::ALL {
                for t in config::HoldTrigger::ALL {
                    out.push(Self::hold_combo(thumb, f, t));
                }
            }
        }
        out
    }

    /// The id spelled `token`, if there is one.
    pub fn parse(token: &str) -> Option<Self> {
        Self::all().into_iter().find(|id| id.as_str() == token)
    }
}

/// Spelling of the pad corners in gesture ids.
const CORNER_TOKENS: [&str; 4] = ["tl", "tr", "bl", "br"];

impl fmt::Display for GestureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One touch session: all fingers down to all fingers up (or a cancel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct SessionId(pub u64);

/// What an effect does to its gesture. Recognizers will report continuous gestures as
/// `Begin`/`Update`/`End`/`Cancel` and one-shots as `Trigger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "contract for the gestures that come next; the actions below already map onto it"
)]
pub enum EffectKind {
    Begin,
    Update,
    End,
    Cancel,
    Trigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeDir {
    Left,
    Right,
    Up,
    Down,
}

impl SwipeDir {
    pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Up, Self::Down];

    fn token(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

/// The kind of a multi-finger (3/4/5) gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiKind {
    Tap,
    PinchIn,
    PinchOut,
    SwipeLeft,
    SwipeRight,
    SwipeUp,
    SwipeDown,
}

impl MultiKind {
    pub fn gesture_id(self, fingers: usize) -> GestureId {
        match self {
            Self::Tap => GestureId::tap(fingers),
            Self::PinchIn => GestureId::pinch(fingers, true),
            Self::PinchOut => GestureId::pinch(fingers, false),
            Self::SwipeLeft => GestureId::swipe(fingers, SwipeDir::Left),
            Self::SwipeRight => GestureId::swipe(fingers, SwipeDir::Right),
            Self::SwipeUp => GestureId::swipe(fingers, SwipeDir::Up),
            Self::SwipeDown => GestureId::swipe(fingers, SwipeDir::Down),
        }
    }

    /// "5本指ピンチイン", "4本指で左にスワイプ", ... for notes and the diagnostics page.
    pub fn label(self, fingers: usize) -> String {
        let what = match self {
            Self::Tap => "タップ",
            Self::PinchIn => "ピンチイン",
            Self::PinchOut => "ピンチアウト",
            Self::SwipeLeft => "で左にスワイプ",
            Self::SwipeRight => "で右にスワイプ",
            Self::SwipeUp => "で上にスワイプ",
            Self::SwipeDown => "で下にスワイプ",
        };
        format!("{fingers}本指{what}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GestureEvent {
    /// The debounced finger count changed (0 = all lifted).
    FingerCount(usize),
    TouchStart {
        fingers: usize,
    },
    TouchEnd {
        /// Most fingers that were down at once.
        fingers: usize,
        duration_us: u64,
    },
    Tap {
        fingers: usize,
    },
    LongHold {
        fingers: usize,
    },
    /// A 3/4/5-finger gesture was recognised (sent right before its `Trigger`).
    Multi {
        fingers: usize,
        kind: MultiKind,
    },
    /// A light or deep press was recognised (sent right before its `Trigger`).
    Press {
        fingers: usize,
        deep: bool,
    },
    /// A corner trigger was recognised (sent right before its `Trigger` or resize).
    Corner {
        corner: usize,
        slot: config::CornerSlot,
    },
}

/// Where a flick throws a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapDir {
    Left,
    Right,
    Up,
    Down,
}

/// A pad edge an edge slider can sit on (the bottom edge stays free for the click zone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeSide {
    Left,
    Right,
    Top,
}

impl EdgeSide {
    /// Key in `Profile::edge_overrides`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Left => "edge.left",
            Self::Right => "edge.right",
            Self::Top => "edge.top",
        }
    }

    /// Note text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "左端",
            Self::Right => "右端",
            Self::Top => "上端",
        }
    }

    /// The top edge slides sideways; the side edges slide up and down.
    pub fn horizontal(self) -> bool {
        self == Self::Top
    }
}

/// What the mouse hook should swallow while a gesture owns the pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Suppress {
    #[default]
    Off,
    /// The cursor stays where it is.
    Cursor,
    /// The cursor stays and clicks are held back.
    CursorAndClicks,
    /// Only clicks are held back (the cursor moves on).
    Clicks,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Reported for the diagnostics page and tests; never acted on.
    Debug(GestureEvent),
    DragBegin,
    /// Centroid travel since the previous move (millimetres, Y grows downward).
    DragMove {
        dx_mm: f32,
        dy_mm: f32,
    },
    DragEnd,
    /// The drag ended with a flick: run this action if the drag began on a title bar. Always
    /// follows a `DragEnd`; never `config::Action::None`.
    DragFlick(config::Action),
    WindowMoveBegin,
    WindowMove {
        dx_mm: f32,
        dy_mm: f32,
    },
    WindowMoveEnd {
        snap: Option<SnapDir>,
    },
    /// Resize the window under the cursor from the 3x3 cell the cursor is in (corner cell: that
    /// corner, edge cell: that edge, centre: a move) until `WindowResizeEnd`.
    WindowResizeBegin,
    WindowResizeMove {
        dx_mm: f32,
        dy_mm: f32,
    },
    WindowResizeEnd,
    /// A pinch bound to 「ウィンドウを拡大・縮小」 was recognised: the window under the cursor is
    /// picked now and scaled by every `WindowZoom` until `WindowZoomEnd`.
    WindowZoomBegin,
    /// Spread of the fingers now over the spread when the zoom began (1.0 = unchanged).
    WindowZoom {
        ratio: f32,
    },
    WindowZoomEnd,
    /// A swipe bound to 「アプリ切り替え (なぞって選ぶ)」 was recognised: Alt goes down and stays
    /// down, and Tab is sent once. Every `AppSwitchStep` until `AppSwitchEnd` moves the selection.
    AppSwitchBegin,
    /// The fingers travelled one more step: Tab (`forward`, the swipe's own direction) or
    /// Shift+Tab.
    AppSwitchStep {
        forward: bool,
    },
    /// The scrub is over: Alt goes up (the selected window opens). `cancel` sends Esc first, so
    /// nothing is switched.
    AppSwitchEnd {
        cancel: bool,
    },
    /// Two-finger scroll output in `WHEEL_DELTA` units (120 = one notch), at most one per input
    /// frame. Positive `v` scrolls up and positive `h` scrolls right (the Win32 wheel convention).
    Scroll {
        h: i32,
        v: i32,
    },
    /// One-shot (multi-finger tap / pinch / swipe / press); never `config::Action::None`.
    Trigger(config::Action),
    /// Freeze the cursor (and hold clicks) from now on; `Off` always runs, even in a blocked app.
    Suppress(Suppress),
    /// One detent of an edge slider: the outputs of one step in the direction of travel.
    EdgeStep {
        side: EdgeSide,
        /// What the slider is set to (the practice view names it).
        kind: config::EdgeKind,
        /// The direction of travel: up along a side edge or right along the top edge.
        forward: bool,
        outs: Vec<config::EdgeOut>,
    },
    /// A corner swipe resizes the foreground window from the corner opposite to pad corner
    /// `corner` (0 = top-left ... 3 = bottom-right) until `CornerResizeEnd`.
    CornerResizeBegin {
        corner: usize,
    },
    /// Finger travel since the previous frame (millimetres, Y grows downward).
    CornerResizeMove {
        dx_mm: f32,
        dy_mm: f32,
    },
    CornerResizeEnd,
    /// Show the ring menu at the cursor.
    RingOpen {
        labels: Vec<String>,
        /// The action of each slot (same order), for the glyph / app icon.
        actions: Vec<config::Action>,
    },
    /// Highlight a slot (`None` = the centre).
    RingSelect(Option<usize>),
    /// Hide the ring; `committed` flashes that slot first. The slot's action follows as `Trigger`.
    RingClose {
        committed: Option<usize>,
    },
    /// A haptic tick for a recognised press. `provisional`: a light press that still waits for a
    /// possible deep press, so its action has not run yet (the weaker tick).
    PressTick {
        deep: bool,
        provisional: bool,
    },
}

/// What the UI shows about the recognizer.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Snapshot {
    pub finger_count: usize,
    /// Centroid of the active contacts in mm.
    pub centroid: Option<Point>,
}

/// Every tunable of the recognizer, in millimetres, microseconds and pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// A changed finger count must hold this long before it is believed.
    pub debounce_us: u64,
    /// No frame for this long while fingers are down cancels the touch (once the report cadence
    /// has been seen: three frames in the session).
    pub stale_us: u64,
    /// A tap moves less than this (largest distance of any contact from where it landed).
    pub tap_travel_mm: f32,
    /// A tap is over within this time (first touch to last lift).
    pub tap_max_us: u64,
    /// A long hold moves less than this while it waits.
    pub hold_travel_mm: f32,
    /// A long hold (and a corner hold) fires after this long at rest.
    pub hold_us: u64,
    /// The centroid must travel this far before a drag begins.
    pub drag_start_mm: f32,
    /// Cursor pixels per millimetre of centroid travel while dragging.
    pub drag_px_per_mm: f32,
    /// After a 3-finger tap, a new touch within this time may become a window move.
    pub window_tap_gap_us: u64,
    /// The centroid must travel this far before a window move begins.
    pub window_start_mm: f32,
    /// Window pixels per millimetre of centroid travel while moving a window.
    pub window_px_per_mm: f32,
    /// Flick speed is measured over this long before the lift.
    pub flick_window_us: u64,
    /// A flick is at least this fast.
    pub flick_speed_mm_s: f32,
    /// The dominant axis of a flick is at least this many times the other.
    pub flick_axis_ratio: f32,
    /// A flick needs its last full-count sample to be this recent at the lift.
    pub flick_stale_us: u64,
    /// ...and samples spanning at least this much time to measure a speed from.
    pub flick_min_span_us: u64,
    /// Multi-finger: spread at or below this fraction of the start is a pinch in.
    pub multi_pinch_in: f32,
    /// Multi-finger: spread at or above this multiple of the start is a pinch out.
    pub multi_pinch_out: f32,
    /// Multi-finger: a pinch also changes the spread by at least this much (mm).
    pub multi_pinch_min_mm: f32,
    /// Multi-finger: centroid travel that makes a swipe.
    pub multi_swipe_mm: f32,
    /// Multi-finger: the dominant axis of a swipe is at least this many times the other.
    pub multi_axis_ratio: f32,
    /// Multi-finger: a swipe keeps the spread inside this band (fractions of the start).
    pub multi_swipe_band: (f32, f32),
    /// Edge slider: width of the band along the pad edge in which a slide may start (mm).
    pub edge_strip_mm: f32,
    /// Edge slider: travel per step (mm); a custom slider has its own.
    pub edge_detent_mm: f32,
    /// Travel of the fingers per window step of the scrubbing app switch (mm).
    pub app_switch_step_mm: f32,
    /// Press: the calibrated pressure must stay past a threshold this long.
    pub press_persist_us: u64,
    /// Press: after it fires, pressure must stay below light-off this long before it can fire again.
    pub press_rearm_us: u64,
    /// Press: centroid movement beyond this before it fires cancels it (mm).
    pub press_move_mm: f32,
    /// Scroll: the centroid of the two fingers must travel this far before scrolling begins (it
    /// is raised above the tap travel so a tap never scrolls).
    pub scroll_start_mm: f32,
    /// Scroll: the centroid travel is at least this many times the change of finger spread
    /// (otherwise the fingers are pinching, which stays Windows').
    pub scroll_pan_ratio: f32,
    /// Scroll: a spread change this large before the scroll begins rules the touch out as a pan.
    pub scroll_pinch_mm: f32,
    /// Scroll: wheel units per millimetre of centroid travel at speed 1.0 (before acceleration).
    pub scroll_units_per_mm: f32,
    /// Scroll axis lock: the dominant axis needs this many times the other one.
    pub scroll_axis_ratio: f32,
    /// Scroll inertia: the lift speed is measured over this long before the lift.
    pub inertia_window_us: u64,
    /// Scroll inertia: the last pan sample must be this recent at the lift.
    pub inertia_stale_us: u64,
    /// Scroll inertia: below this speed (wheel units per second) the glide ends (or never starts).
    pub inertia_min_units_s: f32,
    /// Scroll inertia: the fastest glide start (wheel units per second).
    pub inertia_max_units_s: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            debounce_us: 40_000,
            stale_us: 150_000,
            tap_travel_mm: 2.0,
            tap_max_us: 200_000,
            hold_travel_mm: 3.0,
            hold_us: 500_000,
            drag_start_mm: 1.5,
            drag_px_per_mm: 12.0,
            window_tap_gap_us: 300_000,
            window_start_mm: 8.0,
            window_px_per_mm: 12.0,
            flick_window_us: 60_000,
            flick_speed_mm_s: 400.0,
            flick_axis_ratio: 2.0,
            flick_stale_us: 100_000,
            flick_min_span_us: 15_000,
            multi_pinch_in: 0.75,
            multi_pinch_out: 1.3,
            multi_pinch_min_mm: 4.0,
            multi_swipe_mm: 15.0,
            multi_axis_ratio: 1.5,
            multi_swipe_band: (0.85, 1.15),
            edge_strip_mm: 6.0,
            edge_detent_mm: 2.0,
            app_switch_step_mm: 10.0,
            press_persist_us: 22_000,
            press_rearm_us: 50_000,
            press_move_mm: 3.0,
            scroll_start_mm: 3.0,
            scroll_pan_ratio: 1.5,
            scroll_pinch_mm: 4.0,
            scroll_units_per_mm: 20.0,
            scroll_axis_ratio: 2.0,
            inertia_window_us: 80_000,
            inertia_stale_us: 120_000,
            inertia_min_units_s: 90.0,
            inertia_max_units_s: 14_000.0,
        }
    }
}

impl Thresholds {
    /// The default thresholds with every user-tunable value taken from `[sensitivity]`.
    /// Debounce, stale and the flick sampling constants are not user settings.
    pub fn from_sensitivity(s: &Sensitivity) -> Self {
        let ms = |v: u16| u64::from(v) * 1000;
        Self {
            tap_travel_mm: s.tap_travel_mm,
            tap_max_us: ms(s.tap_ms),
            hold_travel_mm: s.hold_travel_mm,
            hold_us: u64::from(s.hold_ms) * 1000,
            drag_start_mm: s.drag_start_mm,
            drag_px_per_mm: s.drag_gain,
            window_tap_gap_us: ms(s.window_gap_ms),
            window_start_mm: s.window_start_mm,
            window_px_per_mm: s.window_gain,
            flick_speed_mm_s: s.flick_mm_s,
            multi_pinch_in: s.pinch_in,
            multi_pinch_out: s.pinch_out,
            multi_swipe_mm: s.swipe_mm,
            edge_strip_mm: s.edge_strip_mm,
            edge_detent_mm: s.edge_detent_mm,
            app_switch_step_mm: s.app_switch_step_mm,
            ..Self::default()
        }
    }
}

/// The bindings of one multi-finger count.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiCfg {
    pub tap: config::Action,
    pub swipe_left: config::Action,
    pub swipe_right: config::Action,
    pub swipe_up: config::Action,
    pub swipe_down: config::Action,
    pub pinch_in: config::Action,
    pub pinch_out: config::Action,
    /// False when drag uses the same finger count: drag wins and the swipes stay off.
    pub swipes: bool,
}

impl MultiCfg {
    pub fn action(&self, kind: MultiKind) -> &config::Action {
        match kind {
            MultiKind::Tap => &self.tap,
            MultiKind::PinchIn => &self.pinch_in,
            MultiKind::PinchOut => &self.pinch_out,
            MultiKind::SwipeLeft => &self.swipe_left,
            MultiKind::SwipeRight => &self.swipe_right,
            MultiKind::SwipeUp => &self.swipe_up,
            MultiKind::SwipeDown => &self.swipe_down,
        }
    }

    /// The binding of `kind` if it can fire (not `None`, and swipes allowed).
    pub fn bound(&self, kind: MultiKind) -> Option<&config::Action> {
        let swipe = matches!(
            kind,
            MultiKind::SwipeLeft
                | MultiKind::SwipeRight
                | MultiKind::SwipeUp
                | MultiKind::SwipeDown
        );
        let a = self.action(kind);
        (*a != config::Action::None && (self.swipes || !swipe)).then_some(a)
    }
}

/// Multi-finger counts the recognizer knows (3, 4 and 5 fingers).
#[allow(dead_code, reason = "the settings pages iterate the counts next")]
pub const MULTI_COUNTS: [usize; 3] = [3, 4, 5];

/// The targets of the three edge sliders, already resolved for the foreground app.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeCfg {
    pub left: config::EdgeTarget,
    pub right: config::EdgeTarget,
    pub top: config::EdgeTarget,
}

impl EdgeCfg {
    pub fn target(&self, side: EdgeSide) -> &config::EdgeTarget {
        match side {
            EdgeSide::Left => &self.left,
            EdgeSide::Right => &self.right,
            EdgeSide::Top => &self.top,
        }
    }

    fn any(&self) -> bool {
        [&self.left, &self.right, &self.top]
            .iter()
            .any(|t| **t != config::EdgeTarget::None)
    }
}

/// The corner triggers as latched at touchdown: the zone size and the 6 slots of each of the 4
/// corners (`None` = off): three swipe directions, then tap, press and hold. A trigger whose
/// own switch is off has all its slots `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct CornerGestureCfg {
    pub size_mm: f32,
    pub slots: [[Option<config::Action>; 6]; 4],
    /// Raw pressure at which a one-finger press in a zone counts as a click (the 1-finger light
    /// threshold of the custom press).
    pub press_threshold: f32,
}

impl CornerGestureCfg {
    pub fn from_config(cfg: &Config, exe: Option<&str>) -> Option<Self> {
        if !cfg.corner.enabled {
            return None;
        }
        let mut slots: [[Option<config::Action>; 6]; 4] = Default::default();
        for (i, row) in slots.iter_mut().enumerate() {
            for (k, slot) in config::CornerSlot::ALL.into_iter().enumerate() {
                if !cfg.corner.trigger_enabled(slot.trigger()) {
                    continue;
                }
                let a = resolve(cfg, exe, &GestureId::corner_slot(i, slot));
                row[k] = (a != config::Action::None).then_some(a);
            }
        }
        slots.iter().flatten().any(Option::is_some).then_some(Self {
            size_mm: cfg.corner.size_mm,
            slots,
            press_threshold: cfg.press.thresholds(1).0,
        })
    }

    fn index(slot: config::CornerSlot) -> usize {
        config::CornerSlot::ALL
            .iter()
            .position(|s| *s == slot)
            .unwrap_or(0)
    }

    pub fn slot(&self, corner: usize, slot: config::CornerSlot) -> Option<&config::Action> {
        self.slots.get(corner)?[Self::index(slot)].as_ref()
    }

    /// Swipe slot of `dir`.
    pub fn swipe(&self, corner: usize, dir: config::CornerDir) -> Option<&config::Action> {
        self.slot(corner, config::CornerSlot::Swipe(dir))
    }

    /// True when any slot of `corner` (any trigger) has an action.
    pub fn any(&self, corner: usize) -> bool {
        self.slots
            .get(corner)
            .is_some_and(|r| r.iter().any(Option::is_some))
    }
}

/// The action of each flick direction at the end of a drag, as resolved for the foreground app.
#[derive(Debug, Clone, PartialEq)]
pub struct DragFlickCfg {
    pub left: config::Action,
    pub right: config::Action,
    pub up: config::Action,
    pub down: config::Action,
}

impl DragFlickCfg {
    pub fn from_config(cfg: &Config, exe: Option<&str>) -> Option<Self> {
        if !cfg.drag.flick.enabled {
            return None;
        }
        let at = |d: &str| resolve(cfg, exe, &GestureId::drag_flick(d));
        let c = Self {
            left: at("left"),
            right: at("right"),
            up: at("up"),
            down: at("down"),
        };
        [&c.left, &c.right, &c.up, &c.down]
            .iter()
            .any(|a| **a != config::Action::None)
            .then_some(c)
    }

    /// What a flick toward `dir` runs (`None` for an unassigned direction).
    pub fn action(&self, dir: SnapDir) -> Option<&config::Action> {
        let a = match dir {
            SnapDir::Left => &self.left,
            SnapDir::Right => &self.right,
            SnapDir::Up => &self.up,
            SnapDir::Down => &self.down,
        };
        (*a != config::Action::None).then_some(a)
    }
}

/// The ring menu as latched at touchdown.
#[derive(Debug, Clone, PartialEq)]
pub struct RingCfg {
    /// The finger counts (3 to 5) whose long hold opens the ring: `[3, 4, 5]` flags.
    pub counts: [bool; 3],
    /// How long the fingers must rest before the ring opens.
    pub hold_us: u64,
    pub items: Vec<config::RingItem>,
}

impl RingCfg {
    /// Whether a long hold with `fingers` fingers opens the ring.
    pub fn opens_with(&self, fingers: usize) -> bool {
        (3..=5).contains(&fingers) && self.counts[fingers - 3]
    }

    /// The largest finger count that opens the ring.
    pub fn max_fingers(&self) -> usize {
        (3..=5).rev().find(|&n| self.opens_with(n)).unwrap_or(0)
    }
}

/// Most slots the ring shows (more would not fit around the circle).
pub const RING_MAX_ITEMS: usize = config::Ring::MAX_ITEMS;

/// Gesture settings, read when a touch session starts (and kept while a drag outlives it).
#[derive(Debug, Clone, Default)]
pub struct GestureCfg {
    /// The drag rows, indexed by `count - 3` (`None` = row off).
    pub drag_rows: [Option<config::DragRow>; 3],
    pub lift_us: u64,
    pub keep_drag: bool,
    pub flick: bool,
    /// The title-bar flick of a plain drag; `None` while it is off or no direction has an action.
    pub drag_flick: Option<DragFlickCfg>,
    /// Indexed by `count - 3`; `None` while that count's gestures are off.
    pub multi: [Option<MultiCfg>; 3],
    /// `None` when edge sliders are off or none has a target.
    pub edge: Option<EdgeCfg>,
    /// The plain action of a long hold per finger count (index = count - 1), as resolved for the
    /// foreground app. `None` for an unbound count, for every count while the long hold is off,
    /// and for a count bound to the ring menu: the ring state machine owns that one (see `ring`).
    pub long_hold: [config::Action; 5],
    /// `None` when corner swipes are off or no slot has an action.
    pub corner: Option<CornerGestureCfg>,
    /// `None` when the ring is off (with no runnable item the default items are used).
    pub ring: Option<RingCfg>,
    /// Press bindings and thresholds, indexed by `count - 1`; `None` = that count does nothing
    /// (switched off or both actions unset).
    pub press: [Option<PressCfg>; 5],
    /// Give a haptic tick when a press is recognised.
    pub ticks: bool,
    /// Two-finger scroll, when enabled.
    pub scroll: Option<ScrollCfg>,
}

/// The two-finger scroll settings of one touch session (per-app speed already applied).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollCfg {
    pub v_gain: f32,
    pub h_gain: f32,
    pub accel: f32,
    pub natural: bool,
    pub axis_lock: bool,
    pub inertia: bool,
    pub inertia_strength: f32,
}

impl ScrollCfg {
    pub fn from_config(cfg: &Config, exe: Option<&str>) -> Option<Self> {
        let s = &cfg.scroll;
        let app = resolve::scroll_speed(cfg, exe);
        s.enabled.then_some(Self {
            v_gain: s.vertical_speed * app,
            h_gain: s.horizontal_speed * app,
            accel: s.accel,
            natural: s.direction == config::ScrollDirection::Natural,
            axis_lock: s.axis_lock,
            inertia: s.inertia,
            inertia_strength: s.inertia_strength,
        })
    }
}

/// Which counts (3, 4, 5) open the ring for `exe`: the effective long-hold binding per count
/// (the app's override, else the global one) decides, so an override of `ring_menu` opens the ring
/// in that app and any other override (or `none`) turns it off for that count there.
fn ring_counts_for(cfg: &Config, exe: Option<&str>) -> [bool; 3] {
    if !(cfg.long_hold.enabled && cfg.ring.enabled) {
        return [false; 3];
    }
    [3, 4, 5].map(|n| resolve(cfg, exe, &GestureId::long_hold(n)) == config::Action::RingMenu)
}

impl GestureCfg {
    /// How far a contact may travel before the hold candidate ends. The configured hold travel,
    /// but never more than the drag start distance when the drag's own finger count is also a
    /// long-hold count (ring or plain action): once a hand moves that much it is a drag, not a hold.
    pub fn hold_travel_limit(&self, th: &Thresholds) -> f32 {
        if self.touch_rows().any(|(n, _)| self.long_hold_count(n)) {
            th.hold_travel_mm.min(th.drag_start_mm)
        } else {
            th.hold_travel_mm
        }
    }

    /// The drag row for `n` fingers, if it is on.
    pub fn drag_row(&self, n: usize) -> Option<config::DragRow> {
        n.checked_sub(3)
            .and_then(|i| self.drag_rows.get(i))
            .copied()
            .flatten()
    }

    /// Whether any drag row is on.
    pub fn any_drag(&self) -> bool {
        self.drag_rows.iter().any(Option::is_some)
    }

    /// The rows that start by touching and moving (the others wait for a tap first).
    pub fn touch_rows(&self) -> impl Iterator<Item = (usize, config::DragRow)> + '_ {
        (3..=5).filter_map(|n| {
            self.drag_row(n)
                .filter(|r| r.start == config::DragStart::TouchMove)
                .map(|r| (n, r))
        })
    }

    /// The row for `n` fingers if it may start now: a touch-and-move row, or a tap-then-move row
    /// while this session is armed by a tap of `n` fingers (`armed`).
    pub fn startable_row(&self, n: usize, armed: Option<usize>) -> Option<config::DragRow> {
        self.drag_row(n).filter(|r| {
            r.start == config::DragStart::TouchMove
                || (r.start == config::DragStart::TapThenMove && armed == Some(n))
        })
    }

    /// A long hold with `n` fingers does something (opens the ring or runs an action).
    pub fn long_hold_count(&self, n: usize) -> bool {
        self.ring.as_ref().is_some_and(|r| r.opens_with(n))
            || (1..=5).contains(&n) && self.long_hold[n - 1] != config::Action::None
    }

    /// The settings for a touch session that began while `exe` was in the foreground. Every
    /// multi-finger binding goes through the resolver, so a per-app override applies.
    pub fn from_config(cfg: &Config, exe: Option<&str>) -> Self {
        let swipes_free = |n: usize| {
            !cfg.drag
                .row(n)
                .is_some_and(|r| r.enabled && r.start == config::DragStart::TouchMove)
        };
        let multi = |n: usize| MultiCfg {
            tap: resolve(cfg, exe, &MultiKind::Tap.gesture_id(n)),
            swipe_left: resolve(cfg, exe, &MultiKind::SwipeLeft.gesture_id(n)),
            swipe_right: resolve(cfg, exe, &MultiKind::SwipeRight.gesture_id(n)),
            swipe_up: resolve(cfg, exe, &MultiKind::SwipeUp.gesture_id(n)),
            swipe_down: resolve(cfg, exe, &MultiKind::SwipeDown.gesture_id(n)),
            pinch_in: resolve(cfg, exe, &MultiKind::PinchIn.gesture_id(n)),
            pinch_out: resolve(cfg, exe, &MultiKind::PinchOut.gesture_id(n)),
            swipes: swipes_free(n),
        };
        Self {
            drag_rows: [3, 4, 5].map(|n| cfg.drag.row(n).copied().filter(|r| r.enabled)),
            lift_us: u64::from(cfg.drag.lift_timeout_ms) * 1000,
            keep_drag: cfg.drag.keep_drag_on_release,
            flick: cfg.window.flick_snap,
            drag_flick: DragFlickCfg::from_config(cfg, exe),
            multi: [
                cfg.three.enabled.then(|| multi(3)),
                cfg.four.enabled.then(|| multi(4)),
                cfg.five.enabled.then(|| multi(5)),
            ],
            edge: cfg
                .edge
                .enabled
                .then(|| EdgeCfg {
                    left: resolve_edge(cfg, exe, EdgeSide::Left.key()),
                    right: resolve_edge(cfg, exe, EdgeSide::Right.key()),
                    top: resolve_edge(cfg, exe, EdgeSide::Top.key()),
                })
                .filter(EdgeCfg::any),
            corner: CornerGestureCfg::from_config(cfg, exe),
            // With no runnable item the built-in defaults are offered, so the ring always opens.
            ring: Some(ring_counts_for(cfg, exe))
                .filter(|c| c.iter().any(|&on| on))
                .map(|counts| RingCfg {
                    counts,
                    hold_us: u64::from(cfg.sensitivity.hold_ms) * 1000,
                    items: cfg
                        .ring
                        .effective_items()
                        .into_iter()
                        .take(RING_MAX_ITEMS)
                        .collect(),
                }),
            long_hold: std::array::from_fn(|i| {
                let a = resolve(cfg, exe, &GestureId::long_hold(i + 1));
                // The ring menu is the ring state machine's job, not a plain action.
                if cfg.long_hold.enabled && a != config::Action::RingMenu {
                    a
                } else {
                    config::Action::None
                }
            }),
            scroll: ScrollCfg::from_config(cfg, exe),
            ..Self::default()
        }
    }

    /// Fill in the press settings (raw pressure thresholds) for a touch session.
    pub fn set_press(&mut self, cfg: &Config, exe: Option<&str>) {
        self.ticks = cfg.press.haptic_ticks;
        for n in 1..=5 {
            self.press[n - 1] = cfg.press.count_active(n).then(|| {
                let (light_on, deep_on) = cfg.press.thresholds(n);
                PressCfg::new(
                    resolve(cfg, exe, &GestureId::press(n, false)),
                    resolve(cfg, exe, &GestureId::press(n, true)),
                    light_on,
                    deep_on,
                )
            });
            if self.press[n - 1]
                .as_ref()
                .is_some_and(|p| p.light == config::Action::None && p.deep == config::Action::None)
            {
                self.press[n - 1] = None;
            }
        }
    }

    pub fn press_for(&self, fingers: usize) -> Option<&PressCfg> {
        fingers
            .checked_sub(1)
            .and_then(|i| self.press.get(i))
            .and_then(Option::as_ref)
    }

    pub fn any_press(&self) -> bool {
        self.press.iter().any(Option::is_some)
    }

    pub fn multi_for(&self, fingers: usize) -> Option<&MultiCfg> {
        fingers
            .checked_sub(3)
            .and_then(|i| self.multi.get(i))
            .and_then(Option::as_ref)
    }

    pub fn any_multi(&self) -> bool {
        self.multi.iter().any(Option::is_some)
    }
}

/// What a gesture step sees of the current frame. Built by the coordinator once per step.
pub struct Ctx<'a> {
    pub th: &'a Thresholds,
    pub g: &'a GestureCfg,
    pub now: u64,
    pub from_frame: bool,
    /// Raw contact count of the latest frame.
    pub raw: usize,
    /// Debounced contact count.
    pub stable: usize,
    pub cur: &'a [Point],
    pub centroid: Option<Point>,
    /// The set of contact IDs changed in this frame: positions must be re-based.
    pub rebase: bool,
    pub session_start_us: u64,
    /// This touch began soon after a tap of this many fingers, so a tap-then-move row of that
    /// count may start.
    pub armed: Option<usize>,
    /// Pad width and height in mm (`None` until the device is known).
    pub dims: Option<(f32, f32)>,
    /// Furthest any contact of this session has been from where it landed.
    pub max_travel: f32,
    /// Raw (median-filtered) pressure of every contact that reports one.
    pub pressures: &'a [f32],
    /// The pad's physical button is down in the latest frame.
    pub button: bool,
    /// Highest debounced finger count seen in this session.
    pub peak: usize,
}

/// While fingers land, a drag of N fingers is still being assembled: a press or long hold of a
/// lower count may only fire once this long after the touchdown (see `Ctx::is_drag_transient`).
pub const LANDING_GRACE_US: u64 = 120_000;

impl Ctx<'_> {
    /// The `n`-finger reading is only a step of landing or lifting the drag's N fingers (the
    /// session is within the landing grace, or already reached N): a press or hold of the lower
    /// count `n` must not fire from it.
    pub fn is_drag_transient(&self, n: usize) -> bool {
        self.g.touch_rows().any(|(m, _)| {
            n < m
                && (self.peak >= m
                    || self.now.saturating_sub(self.session_start_us) < LANDING_GRACE_US)
        })
    }
}

impl Ctx<'_> {
    /// The touch already had as many fingers as a drag row above `n` (the drag's fingers are
    /// landing or lifting, not a gesture of `n` fingers).
    pub fn drag_reached_above(&self, n: usize) -> bool {
        self.g.touch_rows().any(|(m, _)| n < m && self.peak >= m)
    }
}

pub fn dist(a: Point, b: Point) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

pub fn centroid(points: &[Point]) -> Option<Point> {
    if points.is_empty() {
        return None;
    }
    let n = points.len() as f32;
    Some((
        points.iter().map(|p| p.0).sum::<f32>() / n,
        points.iter().map(|p| p.1).sum::<f32>() / n,
    ))
}

/// Mean distance of the points from `c`.
pub fn spread_of(points: &[Point], c: Point) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    points.iter().map(|p| dist(*p, c)).sum::<f32>() / points.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_overrides_apply_to_their_app_only() {
        let mut cfg = Config::default();
        cfg.corner.enabled = true;
        cfg.corner.corners[0].horizontal = config::Action::Find;
        let mut p = config::Profile {
            exe_name: "code.exe".into(),
            enabled: true,
            ..config::Profile::default()
        };
        p.overrides
            .insert("corner.tl.horizontal".into(), config::Action::TaskView);
        p.overrides
            .insert("corner.br.diagonal".into(), config::Action::ShowDesktop);
        cfg.profiles.push(p);
        let h = config::CornerDir::Horizontal;
        let other = CornerGestureCfg::from_config(&cfg, Some("other.exe")).unwrap();
        assert_eq!(other.swipe(0, h), Some(&config::Action::Find));
        assert_eq!(other.swipe(3, config::CornerDir::Diagonal), None);
        let code = CornerGestureCfg::from_config(&cfg, Some("code.exe")).unwrap();
        assert_eq!(code.swipe(0, h), Some(&config::Action::TaskView));
        assert_eq!(
            code.swipe(3, config::CornerDir::Diagonal),
            Some(&config::Action::ShowDesktop)
        );
    }

    #[test]
    fn ring_counts_come_from_the_long_hold_bindings() {
        use config::Action;
        let mut cfg = Config::default();
        cfg.ring.items = vec![config::RingItem {
            label: "a".into(),
            action: Action::Keys("F1".into()),
        }];
        // Default: the five-finger long hold opens the ring.
        let g = GestureCfg::from_config(&cfg, None);
        assert_eq!(g.ring.map(|r| r.counts), Some([false, false, true]));
        *cfg.long_hold.actions.get_mut(3) = Action::RingMenu;
        *cfg.long_hold.actions.get_mut(5) = Action::Find;
        let g = GestureCfg::from_config(&cfg, None);
        assert_eq!(g.ring.map(|r| r.counts), Some([true, false, false]));
        // The ring choice is not a plain long-hold action; the others are.
        assert_eq!(g.long_hold[2], Action::None);
        assert_eq!(g.long_hold[4], Action::Find);
        cfg.ring.enabled = false;
        assert!(GestureCfg::from_config(&cfg, None).ring.is_none());
        cfg.ring.enabled = true;
        cfg.long_hold.enabled = false;
        let g = GestureCfg::from_config(&cfg, None);
        assert!(g.ring.is_none());
        assert_eq!(g.long_hold[4], Action::None);
        // 1 and 2 fingers cannot open the ring.
        cfg.long_hold.enabled = true;
        *cfg.long_hold.actions.get_mut(2) = Action::RingMenu;
        cfg.normalize();
        assert_eq!(*cfg.long_hold.actions.get(2), Action::None);
    }

    #[test]
    fn ring_counts_follow_the_per_app_binding() {
        use config::Action;
        let mut cfg = Config::default(); // global: 5 fingers = ring menu
        let mut p = config::Profile {
            exe_name: "app.exe".into(),
            enabled: true,
            ..config::Profile::default()
        };
        // (a) an override of ring_menu opens the ring for that count in that app only.
        p.overrides.insert("hold.long.4".into(), Action::RingMenu);
        cfg.profiles = vec![p.clone()];
        let counts = |cfg: &Config, exe| GestureCfg::from_config(cfg, exe).ring.map(|r| r.counts);
        assert_eq!(counts(&cfg, Some("app.exe")), Some([false, true, true]));
        assert_eq!(counts(&cfg, Some("other.exe")), Some([false, false, true]));
        // (b) an explicit none turns the count off there; with no count left there is no ring.
        p.overrides.clear();
        p.overrides.insert("hold.long.5".into(), Action::None);
        cfg.profiles = vec![p.clone()];
        assert_eq!(counts(&cfg, Some("app.exe")), None);
        assert_eq!(counts(&cfg, None), Some([false, false, true]));
        // (c) another action turns the ring off for that count and runs as a plain long hold.
        p.overrides.insert("hold.long.5".into(), Action::Find);
        cfg.profiles = vec![p];
        let g = GestureCfg::from_config(&cfg, Some("app.exe"));
        assert!(g.ring.is_none());
        assert_eq!(g.long_hold[4], Action::Find);
    }

    #[test]
    fn gesture_id_tokens_are_stable() {
        assert_eq!(GestureId::tap(3).as_str(), "tap.3");
        assert_eq!(
            GestureId::swipe(4, SwipeDir::Left).to_string(),
            "swipe.4.left"
        );
        assert_eq!(GestureId::pinch(5, true).as_str(), "pinch.5.in");
        assert_eq!(GestureId::pinch(5, false).as_str(), "pinch.5.out");
        assert_eq!(MultiKind::SwipeUp.gesture_id(4).as_str(), "swipe.4.up");
        assert_eq!(
            MultiKind::SwipeRight.gesture_id(5).as_str(),
            "swipe.5.right"
        );
        assert_eq!(MultiKind::SwipeDown.gesture_id(3).as_str(), "swipe.3.down");
        assert_eq!(MultiKind::Tap.gesture_id(4).as_str(), "tap.4");
    }

    #[test]
    fn labels_name_the_finger_count() {
        assert_eq!(MultiKind::PinchIn.label(5), "5本指ピンチイン");
        assert_eq!(MultiKind::SwipeUp.label(5), "5本指で上にスワイプ");
        assert_eq!(MultiKind::SwipeLeft.label(4), "4本指で左にスワイプ");
    }

    #[test]
    fn drag_on_the_same_count_turns_swipes_off() {
        let mut cfg = Config::default();
        cfg.drag.only(4);
        let g = GestureCfg::from_config(&cfg, None);
        let four = g.multi_for(4).unwrap();
        assert!(!four.swipes);
        assert!(four.bound(MultiKind::SwipeLeft).is_none());
        // Pinch and tap stay available (here: unbound by default).
        assert!(four.bound(MultiKind::PinchIn).is_none());
        cfg.drag.only(3);
        let g = GestureCfg::from_config(&cfg, None);
        assert!(
            g.multi_for(4)
                .unwrap()
                .bound(MultiKind::SwipeLeft)
                .is_some()
        );
        assert!(g.multi_for(3).is_none() && g.multi_for(5).is_none());
        // The three-finger row follows the same rule once the three-finger gestures are on.
        cfg.three.enabled = true;
        cfg.three.swipe_left = config::Action::Find;
        cfg.drag.only(3);
        let g = GestureCfg::from_config(&cfg, None);
        assert!(
            g.multi_for(3)
                .unwrap()
                .bound(MultiKind::SwipeLeft)
                .is_none()
        );
        cfg.drag.only(4);
        let g = GestureCfg::from_config(&cfg, None);
        assert!(
            g.multi_for(3)
                .unwrap()
                .bound(MultiKind::SwipeLeft)
                .is_some()
        );
    }
}
