//! Two-finger scroll: yubi turns a two-finger pan into wheel input of its own, with separate
//! vertical / horizontal speeds, an acceleration curve, axis lock and inertia.
//!
//! State machine of one touch session:
//!
//! - Idle: waiting for exactly two stable, raw-confirmed contacts. Their centroid and spread
//!   are latched as the start.
//! - Ruled out: the spread changed clearly more than the centroid moved (a pinch), so Windows
//!   keeps the touch for the rest of the session.
//! - Proposing: the centroid travelled past the commit distance (above the tap travel, so a tap
//!   never scrolls) and the pan dominates the spread change. The arbiter decides.
//! - Active: this gesture owns the session. Every input frame turns the centroid delta into a
//!   wheel delta (direction, speed, acceleration, axis lock), accumulates the fraction and
//!   sends the whole units, at most once per frame.
//! - Gliding: after the last finger lifts a fast pan keeps going, decaying exponentially, driven
//!   by the engine's timer. The next touch (or a cancel) stops it.

use std::collections::VecDeque;

use super::super::arbitration::{Owner, Proposal};
use super::super::types::{Action, Ctx, Point, ScrollCfg, Thresholds, dist, spread_of};

/// Time between two glide steps.
const INERTIA_STEP_US: u64 = 8_000;
/// Distance the first scroll output is measured from: 1.5 mm is enough to see the dominant axis.
const AXIS_DECIDE_MM: f32 = 1.5;
/// A smoothed finger speed (mm/s) at which the acceleration is half of its maximum.
const ACCEL_HALF_MM_S: f32 = 100.0;
/// The most extra gain acceleration 1.0 adds.
const ACCEL_MAX_EXTRA: f32 = 3.0;
/// Weight of the newest sample in the smoothed finger speed.
const SPEED_ALPHA: f32 = 0.4;

/// Fractional wheel units carried between frames, so no motion is lost to rounding.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct WheelAccum {
    x: f32,
    y: f32,
}

impl WheelAccum {
    /// Add a fractional amount and take out the whole units.
    pub fn add(&mut self, dx: f32, dy: f32) -> (i32, i32) {
        self.x += dx;
        self.y += dy;
        let (ox, oy) = (self.x.trunc(), self.y.trunc());
        self.x -= ox;
        self.y -= oy;
        (ox as i32, oy as i32)
    }
}

/// Extra gain for a finger speed: 1.0 at rest, rising smoothly and monotonically towards
/// `1 + 3 * accel`. `accel` 0 is linear.
pub fn accel_gain(speed_mm_s: f32, accel: f32) -> f32 {
    let s = speed_mm_s.max(0.0);
    let accel = if accel.is_finite() { accel } else { 0.0 };
    1.0 + accel.clamp(0.0, 1.0) * ACCEL_MAX_EXTRA * s / (s + ACCEL_HALF_MM_S)
}

/// Decay rate (1/s) of the glide for a strength 0..=1: higher strength glides longer.
pub fn inertia_rate(strength: f32) -> f32 {
    7.0 - 5.5 * strength.clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lock {
    Free,
    Horizontal,
    Vertical,
}

/// A glide in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Inertia {
    vx: f32,
    vy: f32,
    rate: f32,
    min: f32,
    last_us: u64,
    next_us: u64,
    acc: WheelAccum,
}

#[derive(Debug, Default)]
pub struct ScrollState {
    /// Centroid and spread when the two fingers were first stable.
    start: Option<(Point, f32)>,
    /// A pinch was seen: no scroll for the rest of the session.
    ruled_out: bool,
    active: bool,
    /// Centroid and spread when the gesture took ownership.
    activation: Option<(Point, f32)>,
    /// The fingers spread or pinched after the scroll began: no more output this session.
    pinched: bool,
    /// Centroid of the previous frame while scrolling.
    last: Option<Point>,
    last_us: Option<u64>,
    speed: f32,
    lock: Option<Lock>,
    acc: WheelAccum,
    /// Recent wheel output (time, horizontal and vertical units) for the glide start speed.
    samples: VecDeque<(u64, f32, f32)>,
    glide: Option<Inertia>,
}

impl ScrollState {
    /// A finger touched down: a running glide stops.
    pub fn stop_glide(&mut self) {
        self.glide = None;
    }

    /// The glide's next step time, for the engine's wait.
    pub fn glide_deadline(&self) -> Option<u64> {
        self.glide.map(|g| g.next_us)
    }

    /// Detect the pan. Returns a proposal on the frame it is ready, and on every later frame
    /// while the gesture owns the session (so the arbiter keeps routing to it).
    pub fn step(&mut self, ctx: &Ctx, free: bool) -> Option<Proposal> {
        let th = ctx.th;
        let proposal = Proposal {
            who: Owner::Scroll,
            at_us: ctx.now,
        };
        let two = ctx.raw == 2 && ctx.stable == 2;
        let Some(c) = ctx.centroid.filter(|_| two) else {
            if !self.active {
                self.start = None;
            }
            self.last = None;
            return None;
        };
        if ctx.rebase {
            // The contact IDs changed: positions are not comparable with the previous frame.
            self.last = None;
            if !self.active {
                self.start = None;
                return None;
            }
        }
        if self.active {
            if !self.pinched
                && let Some((ac, asp)) = self.activation
            {
                let change = (spread_of(ctx.cur, c) - asp).abs();
                if change >= th.scroll_pinch_mm && dist(c, ac) < th.scroll_pan_ratio * change {
                    self.pinched = true;
                }
            }
            // Going quiet leaves the session to Windows (the arbiter keeps ownership at most).
            return (free && !self.pinched).then_some(proposal);
        }
        if self.ruled_out || !free {
            return None;
        }
        let spread = spread_of(ctx.cur, c);
        let (start_c, start_spread) = *self.start.get_or_insert((c, spread));
        let travel = dist(c, start_c);
        let change = (spread - start_spread).abs();
        let commit_mm = th.scroll_start_mm.max(th.tap_travel_mm + 0.5);
        if change >= th.scroll_pinch_mm && travel < th.scroll_pan_ratio * change {
            self.ruled_out = true;
            return None;
        }
        (travel >= commit_mm && travel >= th.scroll_pan_ratio * change).then_some(proposal)
    }

    /// The gesture owns the session: turn the centroid movement of this frame into wheel output.
    pub fn commit(&mut self, ctx: &Ctx, cfg: &ScrollCfg, out: &mut Vec<Action>) {
        let Some(c) = ctx.centroid else {
            return;
        };
        if self.pinched {
            return;
        }
        if !self.active {
            self.active = true;
            self.activation = Some((c, spread_of(ctx.cur, c)));
            let start = self.start.map_or(c, |(p, _)| p);
            self.lock = Some(if cfg.axis_lock {
                lock_for(c.0 - start.0, c.1 - start.1, ctx.th.scroll_axis_ratio)
            } else {
                Lock::Free
            });
            // The movement since the start is the first output, so nothing is lost.
            self.last = Some(start);
            self.last_us = None;
            self.speed = 0.0;
        }
        if !ctx.from_frame {
            return;
        }
        let Some(prev) = self.last.replace(c) else {
            self.last_us = Some(ctx.now);
            return;
        };
        let (dx, dy) = (c.0 - prev.0, c.1 - prev.1);
        let dt = self
            .last_us
            .map_or(11_000, |t| ctx.now.saturating_sub(t))
            .clamp(4_000, 50_000) as f32
            / 1e6;
        self.last_us = Some(ctx.now);
        let (ux, uy) = match self.lock {
            Some(Lock::Horizontal) => (dx, 0.0),
            Some(Lock::Vertical) => (0.0, dy),
            _ => (dx, dy),
        };
        let inst = (ux * ux + uy * uy).sqrt() / dt;
        self.speed = if self.speed == 0.0 {
            inst
        } else {
            self.speed + (inst - self.speed) * SPEED_ALPHA
        };
        let gain = accel_gain(self.speed, cfg.accel) * ctx.th.scroll_units_per_mm;
        // Natural: content follows the fingers (down moves the view up). Y grows downward.
        let sign = if cfg.natural { 1.0 } else { -1.0 };
        let (wx, wy) = (
            -ux * gain * cfg.h_gain * sign,
            uy * gain * cfg.v_gain * sign,
        );
        if wx != 0.0 || wy != 0.0 {
            self.samples.push_back((ctx.now, wx, wy));
        }
        let keep_from = ctx.now.saturating_sub(ctx.th.inertia_window_us * 2);
        while self.samples.front().is_some_and(|s| s.0 < keep_from) {
            self.samples.pop_front();
        }
        let (h, v) = self.acc.add(wx, wy);
        if h != 0 || v != 0 {
            out.push(Action::Scroll { h, v });
        }
    }

    /// All fingers are up: start the glide if the pan was fast enough, then forget the session.
    pub fn finish(&mut self, cfg: Option<&ScrollCfg>, th: &Thresholds, t: u64) {
        if self.active
            && !self.pinched
            && let Some(cfg) = cfg.filter(|c| c.inertia)
        {
            self.glide = self.start_glide(cfg, th, t);
        }
        self.cancel();
    }

    /// Drop the session state (a glide in flight is stopped by the next touch, not here).
    pub fn cancel(&mut self) {
        self.start = None;
        self.ruled_out = false;
        self.active = false;
        self.activation = None;
        self.pinched = false;
        self.last = None;
        self.last_us = None;
        self.speed = 0.0;
        self.lock = None;
        self.acc = WheelAccum::default();
        self.samples.clear();
    }

    fn start_glide(&self, cfg: &ScrollCfg, th: &Thresholds, t: u64) -> Option<Inertia> {
        let from = t.saturating_sub(th.inertia_window_us);
        let recent: Vec<_> = self.samples.iter().filter(|s| s.0 >= from).collect();
        let last = recent.last()?;
        if t.saturating_sub(last.0) > th.inertia_stale_us {
            return None;
        }
        // Divide by the time the samples cover (a pan that stopped before the lift only fills
        // part of the window), but never by less than one frame or more than the window.
        let first = recent.first()?;
        let span = last.0.saturating_sub(first.0);
        let step = if recent.len() > 1 {
            span / (recent.len() as u64 - 1)
        } else {
            11_000
        };
        let covered = (span + step).clamp(11_000, th.inertia_window_us.max(11_000));
        let window = covered as f32 / 1e6;
        let (mut vx, mut vy) = recent
            .iter()
            .fold((0.0, 0.0), |(x, y), s| (x + s.1 / window, y + s.2 / window));
        let speed = (vx * vx + vy * vy).sqrt();
        if speed < th.inertia_min_units_s {
            return None;
        }
        if speed > th.inertia_max_units_s {
            let k = th.inertia_max_units_s / speed;
            vx *= k;
            vy *= k;
        }
        Some(Inertia {
            vx,
            vy,
            rate: inertia_rate(cfg.inertia_strength),
            min: th.inertia_min_units_s,
            last_us: t,
            next_us: t + INERTIA_STEP_US,
            acc: WheelAccum::default(),
        })
    }

    /// Advance the glide to `now`.
    pub fn glide_tick(&mut self, now: u64, out: &mut Vec<Action>) {
        let Some(g) = self.glide.as_mut() else {
            return;
        };
        if now < g.next_us {
            return;
        }
        let dt = now.saturating_sub(g.last_us) as f32 / 1e6;
        let decay = (-g.rate * dt).exp();
        let k = (1.0 - decay) / g.rate;
        let (h, v) = g.acc.add(g.vx * k, g.vy * k);
        g.vx *= decay;
        g.vy *= decay;
        g.last_us = now;
        g.next_us = now + INERTIA_STEP_US;
        let done = (g.vx * g.vx + g.vy * g.vy).sqrt() < g.min;
        if h != 0 || v != 0 {
            out.push(Action::Scroll { h, v });
        }
        if done {
            self.glide = None;
        }
    }
}

fn lock_for(dx: f32, dy: f32, ratio: f32) -> Lock {
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax + ay < AXIS_DECIDE_MM {
        Lock::Free
    } else if ay >= ratio * ax {
        Lock::Vertical
    } else if ax >= ratio * ay {
        Lock::Horizontal
    } else {
        Lock::Free
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_state() -> ScrollState {
        ScrollState {
            active: true,
            ..ScrollState::default()
        }
    }

    fn th() -> Thresholds {
        Thresholds::default()
    }

    fn cfg() -> ScrollCfg {
        ScrollCfg {
            v_gain: 1.0,
            h_gain: 1.0,
            accel: 0.0,
            natural: true,
            axis_lock: true,
            inertia: true,
            inertia_strength: 0.5,
        }
    }

    fn ctx<'a>(
        th: &'a Thresholds,
        g: &'a super::super::super::types::GestureCfg,
        pts: &'a [Point],
        now: u64,
    ) -> Ctx<'a> {
        Ctx {
            th,
            g,
            now,
            from_frame: true,
            raw: pts.len(),
            stable: pts.len(),
            cur: pts,
            centroid: super::super::super::types::centroid(pts),
            rebase: false,
            session_start_us: 0,
            armed: None,
            dims: None,
            max_travel: 0.0,
            pressures: &[],
            peak: 0,
            button: false,
        }
    }

    #[test]
    fn accumulator_keeps_the_sum() {
        let mut a = WheelAccum::default();
        let mut sum = (0, 0);
        for _ in 0..1000 {
            let (h, v) = a.add(0.3, -0.7);
            sum = (sum.0 + h, sum.1 + v);
        }
        // 1000 * 0.3 = 300, 1000 * -0.7 = -700, with less than one unit left in the accumulator.
        assert!((sum.0 - 300).abs() <= 1, "{sum:?}");
        assert!((sum.1 + 700).abs() <= 1, "{sum:?}");
    }

    #[test]
    fn accel_is_monotonic_and_linear_at_zero() {
        let mut prev = 0.0;
        for s in (0..2000).step_by(10) {
            let g = accel_gain(s as f32, 0.6);
            assert!(g >= prev);
            prev = g;
        }
        assert!(prev <= 1.0 + 3.0 * 0.6 + 1e-3);
        for s in [0.0, 50.0, 500.0] {
            assert_eq!(accel_gain(s, 0.0), 1.0);
        }
        // More acceleration never lowers the gain.
        assert!(accel_gain(200.0, 0.8) > accel_gain(200.0, 0.3));
    }

    #[test]
    fn lock_picks_the_dominant_axis() {
        assert_eq!(lock_for(0.2, 4.0, 2.0), Lock::Vertical);
        assert_eq!(lock_for(4.0, -0.5, 2.0), Lock::Horizontal);
        assert_eq!(lock_for(3.0, 3.0, 2.0), Lock::Free);
        assert_eq!(lock_for(0.5, 0.5, 2.0), Lock::Free);
    }

    #[test]
    fn a_small_move_or_a_pinch_does_not_propose() {
        let (th, g) = (th(), Default::default());
        let mut s = ScrollState::default();
        let a = [(40.0, 50.0), (50.0, 50.0)];
        assert!(s.step(&ctx(&th, &g, &a, 0), true).is_none());
        // Within tap travel: nothing.
        let b = [(40.5, 51.0), (50.5, 51.0)];
        assert!(s.step(&ctx(&th, &g, &b, 11_000), true).is_none());
        // A pinch (spread +5 mm, centroid still): ruled out, even if it later pans.
        let c = [(35.0, 50.0), (55.0, 50.0)];
        assert!(s.step(&ctx(&th, &g, &c, 22_000), true).is_none());
        let d = [(35.0, 60.0), (55.0, 60.0)];
        assert!(s.step(&ctx(&th, &g, &d, 33_000), true).is_none());
        // Reset by the end of the session.
        s.cancel();
        assert!(s.step(&ctx(&th, &g, &a, 44_000), true).is_none());
        let e = [(40.0, 56.0), (50.0, 56.0)];
        assert!(s.step(&ctx(&th, &g, &e, 55_000), true).is_some());
    }

    #[test]
    fn pan_commits_and_directions_follow_the_setting() {
        let (th, g) = (th(), Default::default());
        for (natural, expect_sign) in [(true, 1), (false, -1)] {
            let mut s = ScrollState::default();
            let mut c = cfg();
            c.natural = natural;
            let a = [(40.0, 50.0), (50.0, 50.0)];
            assert!(s.step(&ctx(&th, &g, &a, 0), true).is_none());
            let b = [(40.0, 56.0), (50.0, 56.0)];
            let cx = ctx(&th, &g, &b, 11_000);
            assert!(s.step(&cx, true).is_some());
            let mut out = Vec::new();
            s.commit(&cx, &c, &mut out);
            // 6 mm down: 6 * 20 units, upward (positive) when natural.
            let Some(Action::Scroll { h, v }) = out.first().cloned() else {
                panic!("no scroll: {out:?}");
            };
            assert_eq!(h, 0);
            assert_eq!(v.signum(), expect_sign);
            assert_eq!(v.abs(), 120);
        }
    }

    #[test]
    fn axis_lock_drops_the_minor_axis_and_free_keeps_both() {
        let (th, g) = (th(), Default::default());
        for (lock, expect_h) in [(true, false), (false, true)] {
            let mut s = ScrollState::default();
            let mut c = cfg();
            c.axis_lock = lock;
            let a = [(40.0, 50.0), (50.0, 50.0)];
            s.step(&ctx(&th, &g, &a, 0), true);
            let b = [(41.0, 56.0), (51.0, 56.0)];
            let cx = ctx(&th, &g, &b, 11_000);
            s.step(&cx, true).unwrap();
            let mut out = Vec::new();
            s.commit(&cx, &c, &mut out);
            let Some(Action::Scroll { h, .. }) = out.first().cloned() else {
                panic!("no scroll");
            };
            assert_eq!(h != 0, expect_h, "lock {lock}");
        }
    }

    #[test]
    fn glide_decays_to_an_end_and_a_touch_stops_it() {
        let th = th();
        let c = cfg();
        let mut s = active_state();
        for i in 0..8u64 {
            s.samples.push_back((i * 11_000, 0.0, 60.0));
        }
        let lift = 7 * 11_000 + 5_000;
        s.finish(Some(&c), &th, lift);
        assert!(s.glide_deadline().is_some(), "glide did not start");
        let mut out = Vec::new();
        let mut now = lift;
        for _ in 0..10_000 {
            let Some(d) = s.glide_deadline() else { break };
            now = d;
            s.glide_tick(now, &mut out);
        }
        assert!(s.glide_deadline().is_none(), "glide never ended");
        assert!(now > lift);
        let total: i32 = out
            .iter()
            .map(|a| match a {
                Action::Scroll { v, .. } => *v,
                _ => 0,
            })
            .sum();
        assert!(total > 0);
        // A new glide stops on touch.
        s.active = true;
        for i in 0..8u64 {
            s.samples.push_back((i * 11_000, 0.0, 60.0));
        }
        s.finish(Some(&c), &th, lift);
        assert!(s.glide_deadline().is_some());
        s.stop_glide();
        assert!(s.glide_deadline().is_none());
    }

    #[test]
    fn a_pinch_after_activation_goes_quiet() {
        let (th, g) = (th(), Default::default());
        let c = cfg();
        let mut s = ScrollState::default();
        let mut out = Vec::new();
        let a = [(40.0, 50.0), (50.0, 50.0)];
        assert!(s.step(&ctx(&th, &g, &a, 0), true).is_none());
        // A pan past the commit distance activates the scroll.
        let b = [(40.0, 56.0), (50.0, 56.0)];
        let cx = ctx(&th, &g, &b, 11_000);
        assert!(s.step(&cx, true).is_some());
        s.commit(&cx, &c, &mut out);
        let before = out.len();
        assert!(before > 0);
        // The fingers then spread 8 mm while the centroid drifts 3 mm: a pinch, not a pan.
        let mut now = 22_000;
        for i in 1..=8 {
            let k = i as f32;
            let pts = [
                (40.0 - k / 2.0, 56.0 + 0.375 * k),
                (50.0 + k / 2.0, 56.0 + 0.375 * k),
            ];
            let cx = ctx(&th, &g, &pts, now);
            if s.step(&cx, true).is_some() {
                s.commit(&cx, &c, &mut out);
            }
            now += 11_000;
        }
        assert!(s.pinched);
        // Only what the drift produced before the spread reached 4 mm was sent.
        let total: i32 = out
            .iter()
            .map(|a| match a {
                Action::Scroll { v, .. } => v.abs(),
                _ => 0,
            })
            .sum();
        assert!(
            total <= 120 + 3 * 20 + 20,
            "scrolled {total} during a pinch"
        );
        // No glide from a pinch.
        s.finish(Some(&c), &th, now);
        assert!(s.glide_deadline().is_none());
    }

    #[test]
    fn a_stopped_pan_glides_at_its_own_speed_and_nan_accel_is_linear() {
        let th = th();
        let c = cfg();
        // 3 frames of 60 units / 11 ms each, then nothing for the rest of the window.
        let mut s = active_state();
        for i in 0..3u64 {
            s.samples.push_back((i * 11_000, 0.0, 60.0));
        }
        s.finish(Some(&c), &th, 3 * 11_000 + 10_000);
        let g = s.glide.expect("glide");
        // 60 / 11 ms is about 5455 units/s; dividing by the full 80 ms would give 2250.
        assert!(g.vy > 4_500.0 && g.vy < 6_000.0, "vy {}", g.vy);
        assert_eq!(accel_gain(100.0, f32::NAN), 1.0);
    }

    #[test]
    fn slow_or_stale_lift_has_no_glide() {
        let th = th();
        let c = cfg();
        let mut s = active_state();
        s.samples.push_back((0, 0.0, 0.5));
        s.finish(Some(&c), &th, 10_000);
        assert!(s.glide_deadline().is_none());
        let mut s = active_state();
        s.samples.push_back((0, 0.0, 80.0));
        s.finish(Some(&c), &th, 900_000);
        assert!(s.glide_deadline().is_none());
    }

    #[test]
    fn inertia_off_has_no_glide() {
        let th = th();
        let mut c = cfg();
        c.inertia = false;
        let mut s = active_state();
        for i in 0..8u64 {
            s.samples.push_back((i * 11_000, 0.0, 60.0));
        }
        s.finish(Some(&c), &th, 7 * 11_000 + 5_000);
        assert!(s.glide_deadline().is_none());
    }

    #[test]
    fn stronger_inertia_glides_longer() {
        assert!(inertia_rate(1.0) < inertia_rate(0.0));
    }
}
