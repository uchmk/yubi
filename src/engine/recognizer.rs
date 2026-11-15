//! Gesture recognizer: a pure state machine from frames to actions.
//!
//! It never touches the OS and never reads a clock; time comes from `Frame::t_us` and `tick`.
//! It tracks fingers (debounced count, taps, long hold: debug events only) and runs the real
//! gestures in parallel: drag, window move (tap, then swipe) and five-finger pinch / swipe.
//! The first gesture to commit in a touch session wins; the others stay quiet until all fingers
//! are lifted.

use std::collections::{BTreeMap, VecDeque};

use super::Frame;
use crate::config::{self, Config};

/// Every tunable of the recognizer, in millimetres, microseconds and pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// A changed finger count must hold this long before it is believed.
    pub debounce_us: u64,
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
    /// Five fingers: spread at or below this fraction of the start is a pinch in.
    pub five_pinch_in: f32,
    /// Five fingers: spread at or above this multiple of the start is a pinch out.
    pub five_pinch_out: f32,
    /// Five fingers: vertical centroid travel that makes a swipe.
    pub five_swipe_mm: f32,
    /// Five fingers: a swipe keeps the spread inside this band (fractions of the start).
    pub five_swipe_band: (f32, f32),
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            debounce_us: 40_000,
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
            five_pinch_in: 0.75,
            five_pinch_out: 1.3,
            five_swipe_mm: 15.0,
            five_swipe_band: (0.85, 1.15),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiveKind {
    PinchIn,
    PinchOut,
    SwipeUp,
    SwipeDown,
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
    /// A five-finger gesture was recognised (sent right before its `Trigger`, if any).
    Five(FiveKind),
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
    /// One-shot (five-finger pinch / swipe); never `config::Action::None`.
    Trigger(config::Action),
}

/// What the UI shows about the recognizer.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Snapshot {
    pub finger_count: usize,
    /// Centroid of the active contacts in mm.
    pub centroid: Option<(f32, f32)>,
}

#[derive(Debug)]
struct Track {
    start: (f32, f32),
    travel: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Winner {
    Drag,
    Window,
    Five,
}

#[derive(Debug)]
struct Session {
    start_us: u64,
    started: bool,
    peak: usize,
    max_travel: f32,
    hold_us: u64,
    hold_since: Option<u64>,
    hold_fired: bool,
    /// The gesture that committed first; the others are cancelled until all fingers lift.
    winner: Option<Winner>,
    /// This touch began soon after a 3-finger tap, so it may become a window move.
    win_candidate: bool,
    /// The touch began while a drag was being kept after release.
    drag_was_held: bool,
    five_fired: bool,
}

/// Gesture settings, read when a touch session starts (and kept while a drag outlives it).
#[derive(Debug, Clone, Default)]
struct GestureCfg {
    drag: bool,
    drag_n: usize,
    lift_us: u64,
    keep_drag: bool,
    window: bool,
    flick: bool,
    five: Option<config::Five>,
}

impl GestureCfg {
    fn from_config(cfg: &Config) -> Self {
        Self {
            drag: cfg.drag.enabled,
            drag_n: usize::from(cfg.drag.fingers.clamp(3, 5)),
            lift_us: u64::from(cfg.drag.lift_timeout_ms) * 1000,
            keep_drag: cfg.drag.keep_drag_on_release,
            window: cfg.window.enabled && cfg.window.move_with_three_finger,
            flick: cfg.window.flick_snap,
            five: cfg.five.enabled.then(|| cfg.five.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DragPhase {
    #[default]
    Idle,
    Active,
    /// Fingers are below N since this time; the drag ends when `lift_timeout` runs out.
    Lifted(u64),
    /// `keep_drag_on_release`: the drag stays until the next tap.
    Held,
}

#[derive(Debug, Default)]
struct DragState {
    phase: DragPhase,
    /// Where the centroid was when N fingers were first all down (for the start threshold).
    origin: Option<(f32, f32)>,
    /// Centroid at the previous move; `None` re-bases on the next frame.
    last: Option<(f32, f32)>,
}

#[derive(Debug, Default)]
struct WindowState {
    /// End of the latest 3-finger tap.
    tap_at: Option<u64>,
    active: bool,
    origin: Option<(f32, f32)>,
    last: Option<(f32, f32)>,
    /// Recent (time, centroid) samples with exactly three fingers, for the flick speed.
    samples: VecDeque<(u64, (f32, f32))>,
}

#[derive(Debug, Default)]
struct FiveState {
    /// Centroid and spread when five fingers were first all down.
    start: Option<((f32, f32), f32)>,
}

#[derive(Debug, Default)]
pub struct Recognizer {
    th: Thresholds,
    g: GestureCfg,
    tracks: BTreeMap<u32, Track>,
    raw: usize,
    /// Count that differs from `stable`, with the time it was first seen.
    pending: Option<(usize, u64)>,
    stable: usize,
    session: Option<Session>,
    centroid: Option<(f32, f32)>,
    /// Positions of the active contacts in the latest frame.
    cur: Vec<(f32, f32)>,
    drag: DragState,
    window: WindowState,
    five: FiveState,
}

impl Recognizer {
    pub fn new(th: Thresholds) -> Self {
        Self {
            th,
            ..Self::default()
        }
    }

    pub fn thresholds(&self) -> &Thresholds {
        &self.th
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            finger_count: self.stable,
            centroid: self.centroid,
        }
    }

    /// True while fingers are on the pad (a gesture may be in flight).
    pub fn active(&self) -> bool {
        self.session.is_some()
    }

    /// Forget everything without emitting events (device gone, config changed, disabled).
    pub fn reset(&mut self) {
        let th = self.th;
        *self = Self::new(th);
    }

    pub fn step(&mut self, frame: &Frame, cfg: &Config) -> Vec<Action> {
        let mut out = Vec::new();
        let t = frame.t_us;
        let active: Vec<_> = frame
            .contacts
            .iter()
            .filter(|c| c.tip && c.confidence)
            .collect();

        let ids: Vec<u32> = active.iter().map(|c| c.id).collect();
        self.tracks.retain(|id, _| ids.contains(id));
        if !active.is_empty() && self.session.is_none() {
            // A drag that outlives its touch keeps the settings it started with.
            if self.drag.phase == DragPhase::Idle {
                self.g = GestureCfg::from_config(cfg);
            }
            let win_candidate = self.g.window
                && self
                    .window
                    .tap_at
                    .is_some_and(|at| t.saturating_sub(at) <= self.th.window_tap_gap_us);
            self.window.tap_at = None;
            self.session = Some(Session {
                start_us: t,
                started: false,
                peak: 0,
                max_travel: 0.0,
                // The config is read once per gesture.
                hold_us: u64::from(cfg.long_hold.hold_ms) * 1000,
                hold_since: None,
                hold_fired: false,
                winner: None,
                win_candidate,
                drag_was_held: self.drag.phase == DragPhase::Held,
                five_fired: false,
            });
        }
        for c in &active {
            let pos = (c.x_mm, c.y_mm);
            let tr = self.tracks.entry(c.id).or_insert(Track {
                start: pos,
                travel: 0.0,
            });
            tr.travel = tr.travel.max(dist(tr.start, pos));
            if let Some(s) = self.session.as_mut() {
                s.max_travel = s.max_travel.max(tr.travel);
            }
        }
        self.cur = active.iter().map(|c| (c.x_mm, c.y_mm)).collect();
        self.centroid = centroid(&self.cur);

        let raw = active.len();
        if raw == 0 {
            self.pending = None;
            self.raw = 0;
            self.end_session(t, &mut out);
            return out;
        }
        if raw != self.raw {
            self.pending = (raw != self.stable).then_some((raw, t));
        }
        self.raw = raw;
        self.advance(t, true, &mut out);
        out
    }

    /// Fire timers that are due at `now_us`.
    pub fn tick(&mut self, now_us: u64) -> Vec<Action> {
        let mut out = Vec::new();
        self.advance(now_us, false, &mut out);
        out
    }

    /// Time of the next timer, if any (for the engine's wait).
    pub fn next_deadline(&self) -> Option<u64> {
        let lift = match self.drag.phase {
            DragPhase::Lifted(since) => Some(since + self.g.lift_us),
            _ => None,
        };
        [
            self.pending.map(|(_, since)| since + self.th.debounce_us),
            self.hold_deadline(),
            lift,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn hold_deadline(&self) -> Option<u64> {
        let s = self.session.as_ref()?;
        if s.hold_fired || self.stable == 0 || self.pending.is_some() {
            return None;
        }
        s.hold_since.map(|h| h + s.hold_us)
    }

    fn advance(&mut self, now: u64, from_frame: bool, out: &mut Vec<Action>) {
        if let Some((n, since)) = self.pending {
            let confirmed = since + self.th.debounce_us;
            if now >= confirmed {
                self.pending = None;
                self.stable = n;
                out.push(Action::Debug(GestureEvent::FingerCount(n)));
                if let Some(s) = self.session.as_mut() {
                    s.peak = s.peak.max(n);
                    // A new count restarts the hold timer; travel stays cumulative.
                    s.hold_since = Some(confirmed);
                    s.hold_fired = false;
                    if !s.started {
                        s.started = true;
                        out.push(Action::Debug(GestureEvent::TouchStart { fingers: n }));
                    }
                }
            }
        }
        if let Some(due) = self.hold_deadline()
            && now >= due
            && let Some(s) = self.session.as_mut()
        {
            if s.max_travel < self.th.hold_travel_mm {
                out.push(Action::Debug(GestureEvent::LongHold {
                    fingers: self.stable,
                }));
            }
            s.hold_fired = true;
        }
        self.gestures(now, from_frame, out);
    }

    fn end_session(&mut self, t: u64, out: &mut Vec<Action>) {
        let Some(s) = self.session.take() else {
            return;
        };
        self.tracks.clear();
        let duration = t.saturating_sub(s.start_us);
        let mut is_tap = false;
        if s.peak > 0 {
            if !s.hold_fired
                && s.max_travel < self.th.tap_travel_mm
                && duration < self.th.tap_max_us
            {
                is_tap = true;
                out.push(Action::Debug(GestureEvent::Tap { fingers: s.peak }));
            }
            out.push(Action::Debug(GestureEvent::TouchEnd {
                fingers: s.peak,
                duration_us: duration,
            }));
        }
        self.finish_gestures(&s, is_tap, t, out);
        if self.stable != 0 {
            self.stable = 0;
            out.push(Action::Debug(GestureEvent::FingerCount(0)));
        }
    }

    /// All fingers are up: close what was in flight and arm what follows a tap.
    fn finish_gestures(&mut self, s: &Session, is_tap: bool, t: u64, out: &mut Vec<Action>) {
        if self.window.active {
            let snap = if self.g.flick {
                self.flick_dir(t)
            } else {
                None
            };
            out.push(Action::WindowMoveEnd { snap });
            self.window = WindowState::default();
        }
        self.window.origin = None;
        self.window.last = None;
        self.window.samples.clear();
        if is_tap && s.peak == 3 && self.g.window {
            self.window.tap_at = Some(t);
        }
        match self.drag.phase {
            DragPhase::Active | DragPhase::Held if is_tap && s.drag_was_held => {
                out.push(Action::DragEnd);
                self.drag = DragState::default();
            }
            DragPhase::Active => {
                self.drag.phase = if self.g.keep_drag {
                    DragPhase::Held
                } else {
                    DragPhase::Lifted(t)
                };
            }
            _ => {}
        }
        self.drag.origin = None;
        self.drag.last = None;
        self.five.start = None;
    }

    /// Run every gesture state machine against the latest positions and debounced count.
    fn gestures(&mut self, now: u64, from_frame: bool, out: &mut Vec<Action>) {
        if let DragPhase::Lifted(since) = self.drag.phase
            && now >= since + self.g.lift_us
        {
            out.push(Action::DragEnd);
            self.drag = DragState::default();
        }
        let Some(mut s) = self.session.take() else {
            return;
        };
        let raw = self.cur.len();
        let stable = self.stable;
        let c = centroid(&self.cur);
        if self.g.drag {
            self.drag_step(&mut s, raw, stable, c, now, out);
        }
        if self.g.window && s.win_candidate {
            self.window_step(&mut s, raw, stable, c, now, from_frame, out);
        }
        if self.g.five.is_some() {
            self.five_step(&mut s, raw, stable, c, out);
        }
        self.session = Some(s);
    }

    fn drag_step(
        &mut self,
        s: &mut Session,
        raw: usize,
        stable: usize,
        c: Option<(f32, f32)>,
        now: u64,
        out: &mut Vec<Action>,
    ) {
        let n = self.g.drag_n;
        let full = raw == n && stable == n;
        let free = matches!(s.winner, None | Some(Winner::Drag));
        match self.drag.phase {
            DragPhase::Idle => {
                // A tap-then-swipe window move wins over a 3-finger drag, but only for the tap
                // window (300 ms from the second touch); after that the touch is a plain drag.
                let in_tap_window =
                    s.win_candidate && now.saturating_sub(s.start_us) <= self.th.window_tap_gap_us;
                let blocked = !free || (in_tap_window && n == 3);
                match c {
                    Some(c) if full && !blocked => {
                        let origin = *self.drag.origin.get_or_insert(c);
                        if dist(origin, c) > self.th.drag_start_mm {
                            out.push(Action::DragBegin);
                            self.drag.phase = DragPhase::Active;
                            self.drag.last = Some(c);
                            s.winner = Some(Winner::Drag);
                        }
                    }
                    _ if !full => self.drag.origin = None,
                    _ => {}
                }
            }
            DragPhase::Active => match c {
                Some(c) if full => {
                    if let Some(l) = self.drag.last {
                        let (dx, dy) = (c.0 - l.0, c.1 - l.1);
                        if dx != 0.0 || dy != 0.0 {
                            out.push(Action::DragMove {
                                dx_mm: dx,
                                dy_mm: dy,
                            });
                        }
                    }
                    self.drag.last = Some(c);
                }
                _ => {
                    // The count changed: re-base so the cursor does not jump.
                    self.drag.last = None;
                    if stable < n {
                        self.drag.phase = if self.g.keep_drag {
                            DragPhase::Held
                        } else {
                            DragPhase::Lifted(now)
                        };
                    }
                }
            },
            DragPhase::Lifted(_) | DragPhase::Held => {
                if let Some(c) = c
                    && full
                    && free
                {
                    self.drag.phase = DragPhase::Active;
                    self.drag.last = Some(c);
                    s.winner = Some(Winner::Drag);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments, reason = "one state machine step")]
    fn window_step(
        &mut self,
        s: &mut Session,
        raw: usize,
        stable: usize,
        c: Option<(f32, f32)>,
        now: u64,
        from_frame: bool,
        out: &mut Vec<Action>,
    ) {
        let full = raw == 3 && stable == 3;
        let free = matches!(s.winner, None | Some(Winner::Window));
        let Some(c) = c.filter(|_| full) else {
            // Count changed: re-base on the next full frame.
            self.window.last = None;
            if !self.window.active {
                self.window.origin = None;
            }
            return;
        };
        if self.window.active {
            if let Some(l) = self.window.last {
                let (dx, dy) = (c.0 - l.0, c.1 - l.1);
                if dx != 0.0 || dy != 0.0 {
                    out.push(Action::WindowMove {
                        dx_mm: dx,
                        dy_mm: dy,
                    });
                }
            }
            self.window.last = Some(c);
        } else if free {
            let origin = *self.window.origin.get_or_insert(c);
            if dist(origin, c) > self.th.window_start_mm {
                out.push(Action::WindowMoveBegin);
                self.window.active = true;
                self.window.last = Some(c);
                s.winner = Some(Winner::Window);
            }
        }
        if self.window.active && from_frame {
            self.window.samples.push_back((now, c));
            while self
                .window
                .samples
                .front()
                .is_some_and(|(t, _)| t + 200_000 < now)
            {
                self.window.samples.pop_front();
            }
        }
    }

    fn five_step(
        &mut self,
        s: &mut Session,
        raw: usize,
        stable: usize,
        c: Option<(f32, f32)>,
        out: &mut Vec<Action>,
    ) {
        let Some(cfg) = self.g.five.as_ref() else {
            return;
        };
        let free = matches!(s.winner, None | Some(Winner::Five));
        let Some(c) = c.filter(|_| raw == 5 && stable == 5) else {
            self.five.start = None;
            return;
        };
        if s.five_fired || !free {
            return;
        }
        let spread = spread_of(&self.cur, c);
        let (start_c, start_spread) = *self.five.start.get_or_insert((c, spread));
        if start_spread < 1.0 {
            return;
        }
        let ratio = spread / start_spread;
        let (lo, hi) = self.th.five_swipe_band;
        let kind = if ratio <= self.th.five_pinch_in {
            FiveKind::PinchIn
        } else if ratio >= self.th.five_pinch_out {
            FiveKind::PinchOut
        } else if (lo..=hi).contains(&ratio) && (c.1 - start_c.1).abs() > self.th.five_swipe_mm {
            // Y grows downward on the pad.
            if c.1 < start_c.1 {
                FiveKind::SwipeUp
            } else {
                FiveKind::SwipeDown
            }
        } else {
            return;
        };
        let action = match kind {
            FiveKind::PinchIn => &cfg.pinch_in,
            FiveKind::PinchOut => &cfg.pinch_out,
            FiveKind::SwipeUp => &cfg.swipe_up,
            FiveKind::SwipeDown => &cfg.swipe_down,
        }
        .clone();
        s.five_fired = true;
        s.winner = Some(Winner::Five);
        out.push(Action::Debug(GestureEvent::Five(kind)));
        if action != config::Action::None {
            out.push(Action::Trigger(action));
        }
    }

    /// Direction of the flick that ended a window move, from the speed over the last
    /// `flick_window_us` before the lift.
    fn flick_dir(&self, lift_us: u64) -> Option<SnapDir> {
        let &(t_last, p_last) = self.window.samples.back()?;
        if lift_us.saturating_sub(t_last) > self.th.flick_stale_us {
            return None;
        }
        let &(t0, p0) = self
            .window
            .samples
            .iter()
            .find(|(t, _)| t + self.th.flick_window_us >= t_last)?;
        let span = t_last - t0;
        if span < self.th.flick_min_span_us {
            return None;
        }
        let secs = span as f32 / 1e6;
        let (vx, vy) = ((p_last.0 - p0.0) / secs, (p_last.1 - p0.1) / secs);
        let (ax, ay) = (vx.abs(), vy.abs());
        if ax.hypot(ay) < self.th.flick_speed_mm_s {
            return None;
        }
        let ratio = self.th.flick_axis_ratio;
        if ax >= ratio * ay {
            Some(if vx > 0.0 {
                SnapDir::Right
            } else {
                SnapDir::Left
            })
        } else if ay >= ratio * ax {
            Some(if vy > 0.0 { SnapDir::Down } else { SnapDir::Up })
        } else {
            None
        }
    }
}

fn centroid(points: &[(f32, f32)]) -> Option<(f32, f32)> {
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
fn spread_of(points: &[(f32, f32)], c: (f32, f32)) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    points.iter().map(|p| dist(*p, c)).sum::<f32>() / points.len() as f32
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::replay;
    use crate::input::Contact;

    fn c(id: u32, x: f32, y: f32) -> Contact {
        Contact {
            id,
            x: 0.0,
            y: 0.0,
            x_mm: x,
            y_mm: y,
            tip: true,
            confidence: true,
            pressure: None,
        }
    }

    fn frame(t_ms: u64, contacts: Vec<Contact>) -> Frame {
        Frame {
            t_us: t_ms * 1000,
            button: false,
            contacts,
        }
    }

    fn events(actions: &[Action]) -> Vec<GestureEvent> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Debug(e) => Some(*e),
                _ => None,
            })
            .collect::<Vec<_>>()
    }

    #[test]
    fn single_finger_tap() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        let mut all = Vec::new();
        all.extend(r.step(&frame(0, vec![c(1, 50.0, 50.0)]), &cfg));
        all.extend(r.step(&frame(60, vec![c(1, 50.5, 50.0)]), &cfg));
        all.extend(r.step(&frame(100, vec![]), &cfg));
        let e = events(&all);
        assert!(e.contains(&GestureEvent::Tap { fingers: 1 }), "{e:?}");
        assert_eq!(e.last(), Some(&GestureEvent::FingerCount(0)));
    }

    #[test]
    fn touch_that_moves_is_not_a_tap() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        let mut all = Vec::new();
        all.extend(r.step(&frame(0, vec![c(1, 50.0, 50.0)]), &cfg));
        all.extend(r.step(&frame(60, vec![c(1, 55.0, 50.0)]), &cfg));
        all.extend(r.step(&frame(100, vec![]), &cfg));
        assert!(
            !events(&all)
                .iter()
                .any(|e| matches!(e, GestureEvent::Tap { .. }))
        );
    }

    #[test]
    fn slow_touch_is_not_a_tap() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        let mut all = Vec::new();
        all.extend(r.step(&frame(0, vec![c(1, 50.0, 50.0)]), &cfg));
        all.extend(r.step(&frame(250, vec![]), &cfg));
        assert!(
            !events(&all)
                .iter()
                .any(|e| matches!(e, GestureEvent::Tap { .. }))
        );
    }

    #[test]
    fn finger_count_waits_for_debounce() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        // One finger lands, the second follows 10 ms later: only "2" is reported.
        let mut all = r.step(&frame(0, vec![c(1, 10.0, 10.0)]), &cfg);
        all.extend(r.step(&frame(10, vec![c(1, 10.0, 10.0), c(2, 30.0, 10.0)]), &cfg));
        assert!(all.is_empty());
        assert_eq!(r.next_deadline(), Some(50_000));
        all.extend(r.step(&frame(55, vec![c(1, 10.0, 10.0), c(2, 30.0, 10.0)]), &cfg));
        let e = events(&all);
        assert_eq!(
            e,
            vec![
                GestureEvent::FingerCount(2),
                GestureEvent::TouchStart { fingers: 2 }
            ]
        );
        assert_eq!(r.snapshot().finger_count, 2);
        assert_eq!(r.snapshot().centroid, Some((20.0, 10.0)));
    }

    #[test]
    fn long_hold_fires_once_from_tick() {
        let mut cfg = Config::default();
        cfg.long_hold.hold_ms = 500;
        let mut r = Recognizer::default();
        let two = vec![c(1, 10.0, 10.0), c(2, 30.0, 10.0)];
        r.step(&frame(0, two.clone()), &cfg);
        assert!(r.tick(100_000).len() == 2); // count + touch start
        assert_eq!(r.next_deadline(), Some(540_000));
        assert!(r.tick(539_999).is_empty());
        assert_eq!(
            events(&r.tick(540_000)),
            vec![GestureEvent::LongHold { fingers: 2 }]
        );
        assert!(r.tick(900_000).is_empty());
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn moving_cancels_long_hold() {
        let mut cfg = Config::default();
        cfg.long_hold.hold_ms = 300;
        let mut r = Recognizer::default();
        r.step(&frame(0, vec![c(1, 10.0, 10.0)]), &cfg);
        r.tick(100_000);
        r.step(&frame(150, vec![c(1, 16.0, 10.0)]), &cfg);
        assert!(r.tick(400_000).is_empty());
    }

    #[test]
    fn untracked_contacts_are_ignored() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        let mut hover = c(1, 10.0, 10.0);
        hover.tip = false;
        let mut weak = c(2, 20.0, 10.0);
        weak.confidence = false;
        assert!(r.step(&frame(0, vec![hover, weak]), &cfg).is_empty());
        assert!(!r.active());
    }

    #[test]
    fn replay_two_finger_tap() {
        let frames = replay::parse(include_str!("fixtures/two_finger_tap.jsonl")).unwrap();
        let out = replay::run(&frames, &Config::default());
        let e: Vec<_> = out
            .iter()
            .map(|(_, a)| events(std::slice::from_ref(a))[0])
            .collect();
        assert!(e.contains(&GestureEvent::FingerCount(2)), "{e:?}");
        assert!(e.contains(&GestureEvent::Tap { fingers: 2 }), "{e:?}");
        assert!(!e.iter().any(|x| matches!(x, GestureEvent::LongHold { .. })));
    }

    #[test]
    fn replay_three_finger_long_hold() {
        let frames = replay::parse(include_str!("fixtures/three_finger_hold.jsonl")).unwrap();
        let mut cfg = Config::default();
        cfg.long_hold.hold_ms = 500;
        let out = replay::run(&frames, &cfg);
        let hold: Vec<_> = out
            .iter()
            .filter(|(_, a)| matches!(a, Action::Debug(GestureEvent::LongHold { fingers: 3 })))
            .collect();
        assert_eq!(hold.len(), 1, "{out:?}");
        // Held from the 40 ms debounce, so it fires at 540 ms (on the timer, between frames).
        assert_eq!(hold[0].0, 540_000);
        assert!(
            !out.iter()
                .any(|(_, a)| matches!(a, Action::Debug(GestureEvent::Tap { .. })))
        );
    }
}
