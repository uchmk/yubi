//! Gesture recognizer: the thin coordinator over tracking, arbitration and the gesture machines.
//!
//! It never touches the OS and never reads a clock; time comes from `Frame::t_us` and `tick`.
//! It tracks fingers (debounced count, taps, long hold: its plain actions run from here) and runs the real
//! gestures in parallel: drag, window move (tap, then swipe) and the 3/4/5-finger tap, pinch and
//! swipe. Gestures only propose to commit; the arbiter picks one winner per touch session, and the
//! others stay quiet until all fingers are lifted.
//!
//! A touch that goes stale (no frame for `stale_us` once its cadence is known) is cancelled: what
//! was in flight is closed and the fingers still down are ignored until everything is lifted.

use super::Frame;
use super::arbitration::{Arbiter, Owner, Proposal};
use super::gestures::corner::CornerState;
use super::gestures::drag::{DragPhase, DragState};
use super::gestures::edge::EdgeState;
use super::gestures::multi::{MultiHit, MultiState};
use super::gestures::press::{PressHit, PressState};
use super::gestures::ring::RingState;
use super::gestures::scroll::ScrollState;
use super::gestures::window::WindowState;
use super::tracking::{Environment, Tracker};
use super::types::{Ctx, GestureCfg, Point, SessionId, centroid};
use crate::config::Config;

pub use super::types::{Action, GestureEvent, SnapDir, Snapshot, Thresholds};

#[derive(Debug)]
struct Session {
    #[allow(dead_code, reason = "tags the effects of the gestures that come next")]
    id: SessionId,
    start_us: u64,
    started: bool,
    peak: usize,
    max_travel: f32,
    hold_us: u64,
    hold_since: Option<u64>,
    hold_fired: bool,
    /// The long-hold action already ran in this touch session; it never runs twice (a count
    /// change such as 2 -> 3 -> 2 re-arms the timer, not the action).
    hold_acted: bool,
    /// This touch began soon after a 3-finger tap, so it may become a window move.
    win_candidate: bool,
    /// The touch began while a drag was being kept after release.
    drag_was_held: bool,
}

#[derive(Debug, Default)]
pub struct Recognizer {
    th: Thresholds,
    g: GestureCfg,
    tracker: Tracker,
    arb: Arbiter,
    session: Option<Session>,
    next_session: u64,
    /// Device / config / app state as the engine last reported it.
    env: Environment,
    /// The same, latched at the first raw touchdown of the current session.
    latched: Option<Environment>,
    centroid: Option<Point>,
    /// Positions of the active contacts in the latest frame.
    cur: Vec<Point>,
    /// The set of contact IDs changed in the frame being processed.
    rebase: bool,
    drag: DragState,
    window: WindowState,
    multi: MultiState,
    edge: EdgeState,
    corner: CornerState,
    ring: RingState,
    press: PressState,
    /// Raw pressure of every active contact in the latest frame (median of the last three).
    pressures: Vec<f32>,
    scroll: ScrollState,
    /// The app (exe) the pan that started the running glide was made in.
    glide_exe: Option<String>,
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
            finger_count: self.tracker.stable,
            centroid: self.centroid,
        }
    }

    /// True while fingers are on the pad (a gesture may be in flight).
    pub fn active(&self) -> bool {
        self.session.is_some()
    }

    /// Whether the touch was cancelled and the recognizer waits for every finger to lift.
    #[allow(dead_code, reason = "read by tests and the diagnostics that come next")]
    pub fn awaiting_all_up(&self) -> bool {
        self.tracker.awaiting_all_up()
    }

    /// The pad size in mm, once known; latched with the next touch.
    pub fn set_pad_dims(&mut self, dims_mm: Option<(f32, f32)>) {
        self.env.dims_mm = dims_mm;
    }

    /// The foreground app (and whether its profile switches yubi off), as the engine last saw
    /// it. A touch session latches it at its first raw touchdown and keeps those bindings until
    /// every finger lifts. A change during the session is NOT a cancel: a drag presses the left
    /// button, which activates the window under the cursor, so the foreground often changes right
    /// after a drag or window move begins.
    pub fn set_foreground(&mut self, exe: Option<&str>, blocked: bool) {
        self.env.exe = exe.map(|e| e.trim().to_owned());
        self.env.app_blocked = blocked;
        // Only a glide after the lift follows the app: it must not leak into another window.
        if self.scroll.glide_deadline().is_some() && (blocked || self.env.exe != self.glide_exe) {
            self.stop_glide();
        }
    }

    /// Stop a scroll glide in flight (config change, pause, another app).
    pub fn stop_glide(&mut self) {
        self.scroll.stop_glide();
        self.glide_exe = None;
    }

    /// The engine bumps this whenever the config changes; latched with the next touch.
    pub fn set_config_generation(&mut self, generation: u64) {
        self.env.config_gen = generation;
    }

    /// True while the ring menu is on screen.
    pub fn ring_open(&self) -> bool {
        self.ring.is_open()
    }

    /// Close the ring without running anything (Esc, or the overlay failed). The fingers may
    /// still be down; they do nothing more until all are lifted.
    pub fn cancel_ring(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        self.ring.cancel(&mut out);
        out
    }

    /// The cursor freeze was lost (watchdog): end a committed edge slide or an open ring cleanly.
    /// The fingers do nothing more until all are lifted. Empty when neither is in flight.
    pub fn abort_freeze(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.edge.is_committed() || self.corner.is_committed() || self.ring.is_open() {
            self.cancel_touch(&mut out);
        }
        out
    }

    /// What the current touch session latched at its first raw touchdown.
    #[allow(dead_code, reason = "read by tests and the gestures that come next")]
    pub fn latched(&self) -> Option<&Environment> {
        self.latched.as_ref()
    }

    /// Forget everything without emitting events (device gone, config changed, disabled).
    pub fn reset(&mut self) {
        let (th, env) = (self.th, self.env.clone());
        *self = Self::new(th);
        self.env = env;
    }

    pub fn step(&mut self, frame: &Frame, cfg: &Config) -> Vec<Action> {
        let mut out = Vec::new();
        let t = frame.t_us;
        // A frame after a long silence: the touch went stale before this frame arrived.
        if self
            .tracker
            .stale_deadline(self.th.stale_us)
            .is_some_and(|d| t >= d)
        {
            self.cancel_touch(&mut out);
        }
        let active: Vec<_> = frame
            .contacts
            .iter()
            .filter(|c| c.tip && c.confidence)
            .collect();
        let obs = self.tracker.observe(t, &active);
        if !active.is_empty() {
            // A touch stops a glide.
            self.scroll.stop_glide();
        }
        self.cur = active.iter().map(|c| (c.x_mm, c.y_mm)).collect();
        self.centroid = centroid(&self.cur);
        self.pressures = self
            .tracker
            .tracks()
            .values()
            .filter_map(|t| t.pressure)
            .collect();
        if self.tracker.awaiting_all_up() {
            if obs.raw == 0 {
                self.tracker.all_up();
            }
            return out;
        }
        if !active.is_empty() && self.session.is_none() {
            self.start_session(t, cfg);
        }
        if let Some(s) = self.session.as_mut() {
            s.max_travel = s.max_travel.max(self.tracker.max_displacement());
        }
        if obs.raw == 0 {
            self.end_session(t, &mut out);
            return out;
        }
        self.rebase = obs.ids_changed;
        self.advance(t, true, &mut out);
        self.rebase = false;
        out
    }

    /// Fire timers that are due at `now_us`.
    pub fn tick(&mut self, now_us: u64) -> Vec<Action> {
        let mut out = Vec::new();
        self.scroll.glide_tick(now_us, &mut out);
        if self
            .tracker
            .stale_deadline(self.th.stale_us)
            .is_some_and(|d| now_us >= d)
        {
            self.cancel_touch(&mut out);
            return out;
        }
        self.advance(now_us, false, &mut out);
        out
    }

    /// Time of the next timer, if any (for the engine's wait).
    pub fn next_deadline(&self) -> Option<u64> {
        [
            self.tracker.debounce_deadline(self.th.debounce_us),
            self.hold_deadline(),
            self.drag.lift_deadline(&self.g),
            self.ring.deadline(&self.g),
            self.session
                .as_ref()
                .and_then(|_| self.press.deadline(&self.th)),
            self.scroll.glide_deadline(),
            self.session
                .as_ref()
                .and_then(|_| self.tracker.stale_deadline(self.th.stale_us)),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn hold_deadline(&self) -> Option<u64> {
        let s = self.session.as_ref()?;
        if s.hold_fired || self.tracker.stable == 0 || self.tracker.is_pending() {
            return None;
        }
        s.hold_since.map(|h| h + s.hold_us)
    }

    /// First raw touchdown: latch the settings and the environment for the whole session.
    fn start_session(&mut self, t: u64, cfg: &Config) {
        // A drag that outlives its touch keeps the settings it started with.
        if self.drag.phase == DragPhase::Idle {
            self.g = GestureCfg::from_config(cfg, self.env.exe.as_deref());
            self.th = Thresholds::from_sensitivity(&cfg.sensitivity);
            self.g.set_press(cfg, self.env.exe.as_deref());
        }
        let win_candidate = self.g.window
            && self
                .window
                .tap_at
                .is_some_and(|at| t.saturating_sub(at) <= self.th.window_tap_gap_us);
        self.window.tap_at = None;
        // A blocked app switches the edge sliders and the ring off like every other gesture.
        if self.env.app_blocked {
            self.g.edge = None;
            self.g.corner = None;
            self.g.ring = None;
        }
        self.edge = EdgeState::default();
        self.corner = CornerState::default();
        self.ring.reset();
        self.next_session += 1;
        self.latched = Some(self.env.clone());
        self.arb.clear();
        self.session = Some(Session {
            id: SessionId(self.next_session),
            start_us: t,
            started: false,
            peak: 0,
            max_travel: 0.0,
            // The config is read once per gesture.
            hold_us: u64::from(cfg.sensitivity.hold_ms) * 1000,
            hold_since: None,
            hold_fired: false,
            hold_acted: false,
            win_candidate,
            drag_was_held: self.drag.phase == DragPhase::Held,
        });
    }

    /// Close what is in flight, drop the session and ignore the fingers until all are up.
    fn cancel_touch(&mut self, out: &mut Vec<Action>) {
        self.window.cancel(out);
        self.drag.cancel(out);
        self.multi.finish();
        self.edge.release(out);
        self.corner.release(out);
        self.ring.cancel(out);
        self.press.reset();
        self.scroll.cancel();
        self.arb.clear();
        self.session = None;
        self.latched = None;
        if self.tracker.stable != 0 {
            out.push(Action::Debug(GestureEvent::FingerCount(0)));
        }
        self.tracker.cancel();
    }

    fn advance(&mut self, now: u64, from_frame: bool, out: &mut Vec<Action>) {
        if let Some((n, confirmed)) = self.tracker.confirm(now, self.th.debounce_us) {
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
        if let Some(due) = self.hold_deadline()
            && now >= due
            && let Some(s) = self.session.as_mut()
        {
            let fired = s.max_travel < self.th.hold_travel_mm;
            s.hold_fired = true;
            if fired {
                let fingers = self.tracker.stable;
                out.push(Action::Debug(GestureEvent::LongHold { fingers }));
                // The ring-menu choice is not in this table: the ring state machine opens it.
                // Only a count with a real action claims the session (an unbound one must not
                // stop the edge slider, drag or scroll), once per touch session, and never for
                // 6 or more contacts. Once it fired, no other gesture runs until all fingers lift
                // (for example a 4-finger swipe after a bound 4-finger long hold stays quiet).
                let action = (1..=5)
                    .contains(&fingers)
                    .then(|| self.g.long_hold[fingers - 1].clone())
                    .filter(|a| *a != crate::config::Action::None);
                if let Some(action) = action
                    && !s.hold_acted
                    && self.arb.resolve(&[Proposal {
                        who: Owner::Hold,
                        at_us: now,
                    }]) == Some(Owner::Hold)
                {
                    s.hold_acted = true;
                    out.push(Action::Trigger(action));
                }
            }
        }
        self.gestures(now, from_frame, out);
    }

    fn end_session(&mut self, t: u64, out: &mut Vec<Action>) {
        let Some(s) = self.session.take() else {
            return;
        };
        let touch_exe = self.latched.take().and_then(|e| e.exe);
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
        self.glide_exe = self.scroll.glide_deadline().and(touch_exe);
        if self.tracker.stable != 0 {
            self.tracker.stable = 0;
            out.push(Action::Debug(GestureEvent::FingerCount(0)));
        }
    }

    /// All fingers are up: close what was in flight and arm what follows a tap.
    fn finish_gestures(&mut self, s: &Session, is_tap: bool, t: u64, out: &mut Vec<Action>) {
        // A provisional light press fires when the fingers lift.
        if let Some(hit) = self
            .press
            .finish_hit(&self.g, self.arb.is_free_for(Owner::Pressure), t)
        {
            self.press.commit(hit, self.g.ticks, out);
        }
        // A touch that pressed is not a tap.
        let is_tap = is_tap && !self.press.touched();
        self.press.reset();
        self.window
            .finish(&self.g, &self.th, is_tap, s.peak, t, out);
        if is_tap
            && !s.drag_was_held
            && let Some(hit) = self
                .multi
                .tap(&self.g, s.peak, self.arb.is_free_for(Owner::Multi))
        {
            self.multi.commit(hit, out);
        }
        self.drag.finish(&self.g, is_tap, s.drag_was_held, t, out);
        self.edge.release(out);
        self.corner.release(out);
        self.ring.finish(&self.g, t, out);
        self.multi.finish();
        self.scroll.finish(self.g.scroll.as_ref(), &self.th, t);
        self.arb.clear();
    }

    /// Run every gesture state machine against the latest positions and debounced count.
    fn gestures(&mut self, now: u64, from_frame: bool, out: &mut Vec<Action>) {
        self.drag.expire(now, &self.g, out);
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let ctx = Ctx {
            th: &self.th,
            g: &self.g,
            now,
            from_frame,
            raw: self.cur.len(),
            stable: self.tracker.stable,
            cur: &self.cur,
            centroid: centroid(&self.cur),
            rebase: self.rebase,
            session_start_us: s.start_us,
            win_candidate: s.win_candidate,
            dims: self.latched.as_ref().and_then(|e| e.dims_mm),
            max_travel: s.max_travel,
            pressures: &self.pressures,
        };
        let mut proposals: Vec<Proposal> = Vec::new();
        let mut hit: Option<MultiHit> = None;
        let mut press_hit: Option<PressHit> = None;
        if ctx.g.drag {
            self.drag.drive(&ctx, out);
            let free = self.arb.is_free_for(Owner::Drag);
            proposals.extend(self.drag.step(&ctx, free));
        }
        if ctx.g.window && ctx.win_candidate {
            let free = self.arb.is_free_for(Owner::Window);
            proposals.extend(self.window.step(&ctx, free, out));
        }
        if ctx.g.any_multi() {
            let free = self.arb.is_free_for(Owner::Multi);
            if let Some(h) = self.multi.step(&ctx, free) {
                proposals.push(h.proposal);
                hit = Some(h);
            }
        }
        if ctx.g.edge.is_some() {
            self.edge.drive(&ctx, out);
            let free = self.arb.is_free_for(Owner::Edge);
            proposals.extend(self.edge.step(&ctx, free, &self.corner));
        }
        if ctx.g.corner.is_some() {
            self.corner.drive(&ctx, out);
            let free = self.arb.is_free_for(Owner::Corner);
            proposals.extend(self.corner.step(&ctx, free));
        }
        if ctx.g.ring.is_some() {
            self.ring.drive(&ctx, out);
            let free = self.arb.is_free_for(Owner::Ring);
            proposals.extend(self.ring.step(&ctx, free));
        }
        if ctx.g.any_press() {
            let free = self.arb.is_free_for(Owner::Pressure);
            if let Some(h) = self.press.step(&ctx, free, out) {
                proposals.push(h.proposal);
                press_hit = Some(h);
            }
        }
        if ctx.g.scroll.is_some() {
            let free = self.arb.is_free_for(Owner::Scroll);
            proposals.extend(self.scroll.step(&ctx, free));
        }
        let winner = self.arb.resolve(&proposals);
        if winner != Some(Owner::Pressure) && press_hit.take().is_some() {
            self.press.lost();
        }
        match winner {
            Some(Owner::Edge) => self.edge.commit(&ctx, out),
            Some(Owner::Corner) => self.corner.commit(&ctx, out),
            Some(Owner::Ring) => self.ring.commit(&ctx, out),
            Some(Owner::Drag) => self.drag.commit(&ctx, out),
            Some(Owner::Window) => self.window.commit(&ctx, out),
            Some(Owner::Multi) => {
                if let Some(h) = hit {
                    self.multi.commit(h, out);
                }
            }
            Some(Owner::Pressure) => {
                if let Some(h) = press_hit {
                    self.press.commit(h, ctx.g.ticks, out);
                }
            }
            Some(Owner::Scroll) => {
                if let Some(cfg) = ctx.g.scroll.as_ref() {
                    self.scroll.commit(&ctx, cfg, out);
                }
            }
            // The long hold already ran its action when it fired.
            Some(Owner::Hold) | None => {}
        }
    }
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
        cfg.sensitivity.hold_ms = 500;
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
        cfg.sensitivity.hold_ms = 300;
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
        cfg.sensitivity.hold_ms = 500;
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

    #[test]
    fn stale_touch_is_cancelled_and_waits_for_all_up() {
        let mut cfg = Config::default();
        cfg.drag.enabled = true;
        let mut r = Recognizer::default();
        for ms in (0..=140).step_by(11) {
            let x = 10.0 + ms as f32 * 0.1;
            let three = vec![c(1, x, 10.0), c(2, x + 10.0, 10.0), c(3, x + 20.0, 10.0)];
            r.step(&frame(ms, three), &cfg);
        }
        assert!(r.active());
        let due = r.next_deadline().unwrap();
        assert!(due <= 140_000 + r.thresholds().stale_us);
        let out = r.tick(140_000 + r.thresholds().stale_us);
        assert!(out.contains(&Action::DragEnd));
        assert!(r.awaiting_all_up() && !r.active());
        // Fingers still down are ignored, however they move.
        let moved = vec![c(1, 40.0, 10.0), c(2, 50.0, 10.0), c(3, 60.0, 10.0)];
        assert!(r.step(&frame(400, moved), &cfg).is_empty());
        assert!(!r.active());
        // Everything lifts: the next touch starts normally.
        r.step(&frame(420, vec![]), &cfg);
        assert!(!r.awaiting_all_up());
        r.step(&frame(440, vec![c(1, 10.0, 10.0)]), &cfg);
        assert!(r.active());
    }

    #[test]
    fn late_frame_cancels_before_it_is_read() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        for ms in [0, 11, 22] {
            r.step(&frame(ms, vec![c(1, 10.0, 10.0)]), &cfg);
        }
        r.step(&frame(500, vec![c(1, 10.0, 10.0)]), &cfg);
        assert!(r.awaiting_all_up() && !r.active());
    }

    #[test]
    fn environment_is_latched_at_first_touchdown() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        r.set_foreground(Some("a.exe"), false);
        r.set_config_generation(1);
        r.step(&frame(0, vec![c(1, 10.0, 10.0)]), &cfg);
        // A config change during the touch applies to the next session only.
        r.set_config_generation(2);
        r.step(&frame(11, vec![c(1, 10.0, 10.0)]), &cfg);
        let latched = r.latched().unwrap().clone();
        assert!(!latched.app_blocked);
        assert_eq!(latched.exe.as_deref(), Some("a.exe"));
        assert_eq!(latched.config_gen, 1);
        r.step(&frame(22, vec![]), &cfg);
        assert!(r.latched().is_none());
        // Between touches the foreground may change freely; the next touch latches it.
        r.set_foreground(Some("b.exe"), true);
        r.step(&frame(40, vec![c(1, 10.0, 10.0)]), &cfg);
        let latched = r.latched().unwrap().clone();
        assert!(latched.app_blocked);
        assert_eq!(latched.exe.as_deref(), Some("b.exe"));
        assert_eq!(latched.config_gen, 2);
    }

    /// A fast two-finger pan in `exe`, lifted; returns the recognizer with a glide in flight.
    fn gliding(exe: &str) -> Recognizer {
        let mut cfg = Config::default();
        cfg.scroll.enabled = true;
        cfg.scroll.inertia = true;
        let mut r = Recognizer::default();
        r.set_foreground(Some(exe), false);
        let mut t = 0u64;
        for k in 0..30 {
            let y = 40.0 + k as f32 * 1.5;
            r.step(&frame(t, vec![c(1, 40.0, y), c(2, 50.0, y)]), &cfg);
            t += 11;
        }
        r.step(&frame(t, vec![]), &cfg);
        assert!(r.scroll.glide_deadline().is_some(), "no glide to test");
        r
    }

    #[test]
    fn a_glide_stops_when_the_foreground_app_changes() {
        let mut r = gliding("a.exe");
        // The same app (polled again) keeps it.
        r.set_foreground(Some("a.exe"), false);
        assert!(r.next_deadline().is_some());
        r.set_foreground(Some("b.exe"), false);
        assert!(r.scroll.glide_deadline().is_none());
        // A blocked app stops it too.
        let mut r = gliding("a.exe");
        r.set_foreground(Some("a.exe"), true);
        assert!(r.scroll.glide_deadline().is_none());
        // The engine's config change path.
        let mut r = gliding("a.exe");
        r.stop_glide();
        assert!(r.next_deadline().is_none());
        // Pausing resets the recognizer: a glide must not keep scrolling.
        let mut r = gliding("a.exe");
        r.reset();
        assert!(r.scroll.glide_deadline().is_none());
    }

    #[test]
    fn foreground_change_during_a_drag_does_not_end_it() {
        let mut cfg = Config::default();
        cfg.drag.enabled = true;
        let mut r = Recognizer::default();
        r.set_foreground(Some("a.exe"), false);
        let mut t = 0;
        let mut out = Vec::new();
        let step = |r: &mut Recognizer, x: f32, t: u64| {
            r.step(
                &frame(
                    t,
                    vec![c(1, x, 10.0), c(2, x + 10.0, 10.0), c(3, x + 20.0, 10.0)],
                ),
                &cfg,
            )
        };
        // Three fingers move far enough to begin a drag.
        for k in 0..12 {
            out.extend(step(&mut r, 10.0 + k as f32 * 2.0, t));
            t += 11;
        }
        assert!(out.iter().any(|a| matches!(a, Action::DragBegin)));
        // The left button press activates another app (and even a blocked one): the drag goes on
        // with the bindings it latched at touchdown.
        r.set_foreground(Some("b.exe"), true);
        assert!(r.active() && !r.awaiting_all_up());
        assert_eq!(r.latched().unwrap().exe.as_deref(), Some("a.exe"));
        let after = step(&mut r, 40.0, t);
        assert!(after.iter().any(|a| matches!(a, Action::DragMove { .. })));
        assert!(!after.iter().any(|a| matches!(a, Action::DragEnd)));
    }

    #[test]
    fn session_latches_per_app_bindings_and_sensitivity() {
        let mut cfg = Config::default();
        cfg.four.swipe_left = crate::config::Action::Find;
        let mut p = crate::config::Profile {
            exe_name: "code.exe".into(),
            enabled: true,
            ..Default::default()
        };
        p.overrides.insert(
            "swipe.4.left".into(),
            crate::config::Action::Keys("Ctrl+PageUp".into()),
        );
        cfg.profiles.push(p);
        cfg.sensitivity.swipe_mm = 30.0;
        cfg.sensitivity.hold_ms = 900;
        let mut r = Recognizer::default();
        r.set_foreground(Some("code.exe"), false);
        r.step(&frame(0, vec![c(1, 10.0, 10.0)]), &cfg);
        assert_eq!(
            r.g.multi_for(4).unwrap().swipe_left,
            crate::config::Action::Keys("Ctrl+PageUp".into())
        );
        assert_eq!(r.thresholds().multi_swipe_mm, 30.0);
        assert_eq!(r.session.as_ref().unwrap().hold_us, 900_000);
        let mut r = Recognizer::default();
        r.set_foreground(Some("other.exe"), false);
        r.step(&frame(0, vec![c(1, 10.0, 10.0)]), &cfg);
        assert_eq!(
            r.g.multi_for(4).unwrap().swipe_left,
            crate::config::Action::Find
        );
    }

    #[test]
    fn swapping_a_finger_keeps_the_count_but_rebases() {
        let mut cfg = Config::default();
        cfg.four.enabled = false;
        cfg.five.enabled = true;
        cfg.five.swipe_up = crate::config::Action::TaskView;
        let mut r = Recognizer::default();
        let mk = |ids: [u32; 5], dy: f32| -> Vec<Contact> {
            ids.iter()
                .enumerate()
                .map(|(k, &id)| c(id, 20.0 + 10.0 * k as f32, 40.0 + dy))
                .collect()
        };
        let mut t = 0;
        for _ in 0..8 {
            r.step(&frame(t, mk([1, 2, 3, 4, 5], 0.0)), &cfg);
            t += 11;
        }
        // One finger swaps IDs and its place jumps; no swipe may come from that jump alone.
        let out = r.step(&frame(t, mk([1, 2, 3, 4, 6], -30.0)), &cfg);
        assert!(!out.iter().any(|a| matches!(a, Action::Trigger(_))));
    }

    fn fingers_at(n: u32, dx: f32) -> Vec<Contact> {
        (0..n)
            .map(|k| c(k + 1, 20.0 + 15.0 * k as f32 + dx, 40.0))
            .collect()
    }

    fn triggers(out: &[Action]) -> Vec<crate::config::Action> {
        out.iter()
            .filter_map(|a| match a {
                Action::Trigger(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// `n` fingers rest for `rest_ms`, then swipe left 60 mm and lift.
    fn rest_swipe_lift(r: &mut Recognizer, cfg: &Config, n: u32, rest_ms: u64) -> Vec<Action> {
        let mut out = Vec::new();
        let mut t = 0;
        while t <= rest_ms {
            out.extend(r.step(&frame(t, fingers_at(n, 0.0)), cfg));
            t += 11;
        }
        for i in 1..=12 {
            out.extend(r.step(&frame(t, fingers_at(n, -5.0 * i as f32)), cfg));
            t += 11;
        }
        out.extend(r.step(&frame(t, vec![]), cfg));
        out
    }

    #[test]
    fn a_bound_long_hold_takes_the_session_so_a_swipe_after_it_stays_quiet() {
        use crate::config::Action as A;
        let mut cfg = Config::default();
        cfg.sensitivity.hold_ms = 300;
        cfg.four.enabled = true;
        cfg.four.swipe_left = A::TaskView;
        // Control: with no long-hold action the swipe fires.
        let out = rest_swipe_lift(&mut Recognizer::default(), &cfg, 4, 400);
        assert_eq!(triggers(&out), vec![A::TaskView], "{out:?}");
        // Bound: the long hold runs once and the swipe in the same session does not.
        *cfg.long_hold.actions.get_mut(4) = A::Find;
        let out = rest_swipe_lift(&mut Recognizer::default(), &cfg, 4, 400);
        assert_eq!(triggers(&out), vec![A::Find], "{out:?}");
    }

    #[test]
    fn the_long_hold_action_runs_once_per_touch_session() {
        use crate::config::Action as A;
        let mut cfg = Config::default();
        cfg.sensitivity.hold_ms = 300;
        *cfg.long_hold.actions.get_mut(2) = A::Find;
        let mut r = Recognizer::default();
        let mut out = Vec::new();
        // 2 fingers rest, a third joins and leaves: the timer re-arms for 2 fingers.
        for (from, to, n) in [(0u64, 400u64, 2u32), (400, 800, 3), (800, 1300, 2)] {
            let mut t = from;
            while t < to {
                out.extend(r.step(&frame(t, fingers_at(n, 0.0)), &cfg));
                t += 11;
            }
        }
        let holds = events(&out)
            .iter()
            .filter(|e| matches!(e, GestureEvent::LongHold { fingers: 2 }))
            .count();
        assert_eq!(holds, 2, "the hold timer re-armed: {out:?}");
        assert_eq!(triggers(&out), vec![A::Find], "{out:?}");
    }

    #[test]
    fn six_contacts_do_not_run_the_five_finger_long_hold() {
        use crate::config::Action as A;
        let mut cfg = Config::default();
        cfg.sensitivity.hold_ms = 300;
        *cfg.long_hold.actions.get_mut(5) = A::Find;
        let mut r = Recognizer::default();
        let mut out = Vec::new();
        for t in (0..600).step_by(11) {
            out.extend(r.step(&frame(t, fingers_at(6, 0.0)), &cfg));
        }
        assert!(triggers(&out).is_empty(), "{out:?}");
    }
}
