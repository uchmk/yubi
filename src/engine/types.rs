//! Shared contracts of the recognizer: gesture ids, session ids, effect kinds, the actions the
//! recognizer emits, the thresholds it reads and the per-gesture settings latched at touchdown.
//!
//! Everything here is plain data. Recognition stays pure: frames, time and config in, actions out.

use std::fmt;

use super::resolve::resolve;
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

    /// The long hold with the configured finger count (reserved).
    pub fn long_hold() -> Self {
        Self("hold.long".to_owned())
    }

    /// A press or swipe starting in a pad corner (0 = top-left, 1 = top-right, 2 = bottom-left,
    /// 3 = bottom-right; reserved).
    pub fn corner(index: usize, swipe: bool) -> Self {
        Self(format!(
            "corner.{}.{}",
            CORNER_TOKENS[index % 4],
            if swipe { "swipe" } else { "tap" }
        ))
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
        for n in 1..=5 {
            out.push(Self::press(n, false));
            out.push(Self::press(n, true));
        }
        out.push(Self::long_hold());
        for i in 0..4 {
            out.push(Self::corner(i, false));
            out.push(Self::corner(i, true));
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
}

/// Where a flick throws a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapDir {
    Left,
    Right,
    Up,
    Down,
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
    WindowMoveBegin,
    WindowMove {
        dx_mm: f32,
        dy_mm: f32,
    },
    WindowMoveEnd {
        snap: Option<SnapDir>,
    },
    /// One-shot (multi-finger tap / pinch / swipe); never `config::Action::None`.
    Trigger(config::Action),
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
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            debounce_us: 40_000,
            stale_us: 150_000,
            tap_travel_mm: 2.0,
            tap_max_us: 200_000,
            hold_travel_mm: 3.0,
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
            drag_start_mm: s.drag_start_mm,
            drag_px_per_mm: s.drag_gain,
            window_tap_gap_us: ms(s.window_gap_ms),
            window_start_mm: s.window_start_mm,
            window_px_per_mm: s.window_gain,
            flick_speed_mm_s: s.flick_mm_s,
            multi_pinch_in: s.pinch_in,
            multi_pinch_out: s.pinch_out,
            multi_swipe_mm: s.swipe_mm,
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

/// Gesture settings, read when a touch session starts (and kept while a drag outlives it).
#[derive(Debug, Clone, Default)]
pub struct GestureCfg {
    pub drag: bool,
    pub drag_n: usize,
    pub lift_us: u64,
    pub keep_drag: bool,
    pub window: bool,
    pub flick: bool,
    /// Indexed by `count - 3`. Three fingers have no multi bindings yet.
    pub multi: [Option<MultiCfg>; 3],
}

impl GestureCfg {
    /// The settings for a touch session that began while `exe` was in the foreground. Every
    /// multi-finger binding goes through the resolver, so a per-app override applies.
    pub fn from_config(cfg: &Config, exe: Option<&str>) -> Self {
        let drag_n = usize::from(cfg.drag.fingers.clamp(3, 5));
        let swipes_free = |n: usize| !(cfg.drag.enabled && drag_n == n);
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
            drag: cfg.drag.enabled,
            drag_n,
            lift_us: u64::from(cfg.drag.lift_timeout_ms) * 1000,
            keep_drag: cfg.drag.keep_drag_on_release,
            window: cfg.window.enabled && cfg.window.move_with_three_finger,
            flick: cfg.window.flick_snap,
            multi: [
                None,
                cfg.four.enabled.then(|| multi(4)),
                cfg.five.enabled.then(|| multi(5)),
            ],
        }
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
    /// This touch began soon after a 3-finger tap, so it may become a window move.
    pub win_candidate: bool,
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
        cfg.drag.enabled = true;
        cfg.drag.fingers = 4;
        let g = GestureCfg::from_config(&cfg, None);
        let four = g.multi_for(4).unwrap();
        assert!(!four.swipes);
        assert!(four.bound(MultiKind::SwipeLeft).is_none());
        // Pinch and tap stay available (here: unbound by default).
        assert!(four.bound(MultiKind::PinchIn).is_none());
        cfg.drag.fingers = 3;
        let g = GestureCfg::from_config(&cfg, None);
        assert!(
            g.multi_for(4)
                .unwrap()
                .bound(MultiKind::SwipeLeft)
                .is_some()
        );
        assert!(g.multi_for(3).is_none() && g.multi_for(5).is_none());
    }
}
