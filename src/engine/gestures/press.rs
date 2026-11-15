//! Light and deep press ("Force Touch"), keyed by finger count (1 to 5).
//!
//! The signal `q` is the raw pressure: the maximum over the contacts, which arrive already smoothed
//! by a three-sample median. The thresholds are raw values too (0.01 to 0.20, per finger count);
//! the calibration wizard only suggests values for them.
//!
//! Each threshold is a Schmitt trigger (on / off with hysteresis of one hundredth for light and
//! two for deep) that must hold for the persistence time before it counts. States:
//!
//! * `Idle`: nothing latched. `q >= light_on` for 22 ms latches a light press.
//! * With only a light binding the light action fires right then (`Light` hit).
//! * With a deep binding too, the light press is provisional: it fires when the press is released
//!   (`q < light_off`, or the fingers lift), and is discarded when `q >= deep_on` holds for 22 ms,
//!   which fires the deep action instead (`Deep` hit).
//! * `Rearm`: after any outcome nothing can fire again until `q` has been below `light_off` for
//!   50 ms.
//!
//! A count change (more fingers, or fingers leaving while others stay down; all of them lifting
//! is the release), a contact swap, or centroid movement beyond 3 mm before the action fired
//! cancels the press (it goes to `Rearm`). A cancelled press leaves the touch free to be a tap;
//! only a press that fired (or a provisional one that fires on the lift) takes it. A light-only
//! press fires as soon as it is held and owns the touch, so a swipe later in the same touch does
//! nothing; with a deep binding too the latch does not take the session, so moving beyond 3 mm
//! cancels it and the swipe wins. So does travelling: if the centroid or the spread moved
//! more than 3 mm within the last 100 ms, a swipe or pinch is under way and no press fires. Nothing here talks to the OS; the recognizer commits
//! hits through the arbiter, so a press never fires while another gesture owns the session.

use super::super::arbitration::{Owner, Proposal};
use std::collections::VecDeque;

use super::super::types::{
    Action, Ctx, GestureCfg, GestureEvent, LANDING_GRACE_US, Point, dist, spread_of,
};
use crate::config;

/// Hysteresis below the "on" threshold at which a press counts as let go (raw pressure).
pub const LIGHT_OFF_DELTA: f32 = 0.01;
pub const DEEP_OFF_DELTA: f32 = 0.02;
/// Lowest "off" level: the pad's resting noise sits around it.
const MIN_OFF: f32 = 0.01;
/// Slack for comparing pressures that were scaled from integers (0.07 may arrive as 0.0699999).
const EPS: f32 = 1e-4;
/// Look-back for "the fingers are travelling": a swipe or pinch under way never counts as a press.
const MOTION_WINDOW_US: u64 = 100_000;

/// Bindings and thresholds of one finger count, latched at touchdown.
#[derive(Debug, Clone, PartialEq)]
pub struct PressCfg {
    pub light: config::Action,
    pub deep: config::Action,
    pub light_on: f32,
    pub light_off: f32,
    pub deep_on: f32,
    pub deep_off: f32,
}

impl PressCfg {
    pub fn new(light: config::Action, deep: config::Action, light_on: f32, deep_on: f32) -> Self {
        Self {
            light,
            deep,
            light_on,
            light_off: (light_on - LIGHT_OFF_DELTA).max(MIN_OFF),
            deep_on,
            deep_off: (deep_on - DEEP_OFF_DELTA).max(MIN_OFF),
        }
    }

    fn light_bound(&self) -> bool {
        self.light != config::Action::None
    }

    fn deep_bound(&self) -> bool {
        self.deep != config::Action::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressKind {
    Light,
    Deep,
}

/// A press ready to commit.
#[derive(Debug, Clone, PartialEq)]
pub struct PressHit {
    pub proposal: Proposal,
    pub fingers: usize,
    pub kind: PressKind,
    pub action: config::Action,
    /// The light press was provisional (it waited for a possible deep press): its tick was
    /// already given at latch time.
    pub provisional: bool,
}

/// On / off trigger with the time it turned on.
#[derive(Debug, Default, Clone, Copy)]
struct Gate {
    above: bool,
    since: u64,
}

impl Gate {
    fn update(&mut self, q: f32, on: f32, off: f32, now: u64) {
        if !self.above && q + EPS >= on {
            self.above = true;
            self.since = now;
        } else if self.above && q + EPS < off {
            self.above = false;
        }
    }

    fn held(&self, now: u64, us: u64) -> bool {
        self.above && now.saturating_sub(self.since) >= us
    }

    fn due(&self, us: u64) -> Option<u64> {
        self.above.then(|| self.since + us)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Idle,
    /// Light press recognised, waiting for a deep press or the release.
    Latched {
        fingers: usize,
        start: Point,
        provisional: bool,
        /// Fingers are coming off (count below `fingers`).
        lifting: bool,
    },
    /// Fired or cancelled: wait for `q` to stay below light-off.
    Rearm { below_since: Option<u64> },
}

#[derive(Debug, Default)]
pub struct PressState {
    phase: Phase,
    light: Gate,
    deep: Gate,
    /// Centroid when the light gate turned on (movement is measured from here).
    arm: Option<Point>,
    touched: bool,
    /// A lower-count press is held back (drag fingers still landing) until this time.
    hold_until: Option<u64>,
    /// Recent (time, centroid, spread) while the count is settled.
    trail: VecDeque<(u64, Point, f32)>,
}

/// Highest raw pressure over the contacts, `0` without pressure.
pub fn signal(pressures: &[f32]) -> f32 {
    pressures
        .iter()
        .copied()
        .filter(|p| p.is_finite())
        .fold(0.0, f32::max)
}

impl PressState {
    /// A press fired in this session: the touch is not a tap.
    pub fn touched(&self) -> bool {
        self.touched
    }

    /// The fingers moved or spread more than the movement limit within the look-back window.
    fn travelling(&self, now: u64, c: Point, spread: f32, limit: f32) -> bool {
        self.trail.iter().any(|&(t, p, sp)| {
            now.saturating_sub(t) <= MOTION_WINDOW_US
                && (dist(p, c) > limit || (sp - spread).abs() > limit)
        })
    }

    fn cancel(&mut self) {
        self.phase = Phase::Rearm { below_since: None };
        self.arm = None;
    }

    /// Run one step. Pushes a provisional tick into `out` when a provisional light press latches.
    pub fn step(&mut self, ctx: &Ctx, free: bool, out: &mut Vec<Action>) -> Option<PressHit> {
        let now = ctx.now;
        let th = ctx.th;
        let n = match self.phase {
            Phase::Latched { fingers, .. } => fingers,
            _ => ctx.stable,
        };
        let Some(cfg) = ctx.g.press_for(n) else {
            *self = Self {
                touched: self.touched,
                ..Self::default()
            };
            return None;
        };
        let settled = ctx.raw >= 1 && ctx.raw == ctx.stable && !ctx.rebase;
        let latched = matches!(self.phase, Phase::Latched { .. });
        if !latched && !settled {
            // Counts are still changing: nothing may arm.
            self.light.above = false;
            self.deep.above = false;
            self.arm = None;
            self.trail.clear();
            if let Phase::Rearm { below_since } = &mut self.phase {
                *below_since = None;
            }
            return None;
        }
        let q = signal(ctx.pressures);
        let spread = ctx.centroid.map_or(0.0, |c| spread_of(ctx.cur, c));
        let travelling = ctx
            .centroid
            .is_some_and(|c| self.travelling(now, c, spread, th.press_move_mm));
        if let Some(c) = ctx.centroid {
            self.trail.push_back((now, c, spread));
            while self
                .trail
                .front()
                .is_some_and(|&(t, ..)| now.saturating_sub(t) > MOTION_WINDOW_US)
            {
                self.trail.pop_front();
            }
        }
        self.light.update(q, cfg.light_on, cfg.light_off, now);
        self.deep.update(q, cfg.deep_on, cfg.deep_off, now);

        match self.phase {
            Phase::Idle => {
                if !self.light.above {
                    self.arm = None;
                    return None;
                }
                let c = ctx.centroid.unwrap_or_default();
                let start = *self.arm.get_or_insert(c);
                if dist(start, c) > th.press_move_mm {
                    self.cancel();
                    return None;
                }
                if !self.light.held(now, th.press_persist_us) {
                    return None;
                }
                if !free || travelling {
                    self.cancel();
                    return None;
                }
                // The other fingers of a drag are still landing (or lifting): wait, do not fire.
                if ctx.is_drag_transient(n) {
                    if ctx.drag_reached_above(n) {
                        // Lifting from the drag's count: this press is over for now.
                        self.cancel();
                    } else {
                        self.hold_until = Some(ctx.session_start_us + LANDING_GRACE_US);
                    }
                    return None;
                }
                if !cfg.deep_bound() {
                    return Some(self.hit(n, PressKind::Light, cfg, now, false));
                }
                self.phase = Phase::Latched {
                    fingers: n,
                    start,
                    provisional: cfg.light_bound(),
                    lifting: false,
                };
                if cfg.light_bound() && ctx.g.ticks {
                    out.push(Action::PressTick {
                        deep: false,
                        provisional: true,
                    });
                }
                None
            }
            Phase::Latched {
                fingers,
                start,
                provisional,
                ..
            } => {
                // A contact swap at the same count cancels; fingers merely leaving are judged below.
                if ctx.raw > fingers || (ctx.rebase && ctx.raw >= fingers) || !free {
                    self.cancel();
                    return None;
                }
                // Some fingers left and the others stayed down (the debounced count settled
                // between 1 and `fingers - 1`): not a release, the press is off. A real lift goes
                // to zero and fires through `finish_hit`.
                if (1..fingers).contains(&ctx.stable) {
                    self.cancel();
                    return None;
                }
                let lifting = ctx.raw < fingers;
                let c = ctx.centroid.unwrap_or(start);
                if !lifting && dist(start, c) > th.press_move_mm {
                    self.cancel();
                    return None;
                }
                self.phase = Phase::Latched {
                    fingers,
                    start,
                    provisional,
                    lifting,
                };
                if travelling && !lifting {
                    self.cancel();
                    return None;
                }
                if cfg.deep_bound() && !lifting && self.deep.held(now, th.press_persist_us) {
                    return Some(self.hit(fingers, PressKind::Deep, cfg, now, provisional));
                }
                if !self.light.above {
                    return if provisional {
                        Some(self.hit(fingers, PressKind::Light, cfg, now, true))
                    } else {
                        self.cancel();
                        None
                    };
                }
                None
            }
            Phase::Rearm { below_since } => {
                if self.light.above {
                    self.phase = Phase::Rearm { below_since: None };
                } else {
                    let since = below_since.unwrap_or(now);
                    if now.saturating_sub(since) >= th.press_rearm_us {
                        self.phase = Phase::Idle;
                        self.arm = None;
                    } else {
                        self.phase = Phase::Rearm {
                            below_since: Some(since),
                        };
                    }
                }
                None
            }
        }
    }

    fn hit(
        &self,
        fingers: usize,
        kind: PressKind,
        cfg: &PressCfg,
        now: u64,
        provisional: bool,
    ) -> PressHit {
        PressHit {
            proposal: Proposal {
                who: Owner::Pressure,
                at_us: now,
            },
            fingers,
            kind,
            action: match kind {
                PressKind::Light => cfg.light.clone(),
                PressKind::Deep => cfg.deep.clone(),
            },
            provisional,
        }
    }

    /// The press won the session: report it, tick, trigger, and wait to rearm.
    pub fn commit(&mut self, hit: PressHit, ticks: bool, out: &mut Vec<Action>) {
        self.touched = true;
        let deep = hit.kind == PressKind::Deep;
        out.push(Action::Debug(GestureEvent::Press {
            fingers: hit.fingers,
            deep,
        }));
        // A provisional light press already ticked when it latched.
        if ticks && (deep || !hit.provisional) {
            out.push(Action::PressTick {
                deep,
                provisional: false,
            });
        }
        out.push(Action::Trigger(hit.action));
        self.cancel();
    }

    /// Another gesture owns the session: the press is dropped.
    pub fn lost(&mut self) {
        self.cancel();
    }

    /// When the lift arrives while a provisional light press is still latched, it fires then.
    pub fn finish_hit(&self, g: &GestureCfg, free: bool, now: u64) -> Option<PressHit> {
        let Phase::Latched {
            fingers,
            provisional: true,
            ..
        } = self.phase
        else {
            return None;
        };
        let cfg = g.press_for(fingers)?;
        free.then(|| self.hit(fingers, PressKind::Light, cfg, now, true))
    }

    /// The next time this machine needs a tick.
    pub fn deadline(&self, th: &super::super::types::Thresholds) -> Option<u64> {
        match self.phase {
            Phase::Idle if self.arm.is_some() => self
                .light
                .due(th.press_persist_us)
                .map(|d| d.max(self.hold_until.unwrap_or(0))),
            Phase::Latched { lifting: false, .. } => self.deep.due(th.press_persist_us),
            Phase::Rearm {
                below_since: Some(s),
            } => Some(s + th.press_rearm_us),
            _ => None,
        }
    }

    /// All fingers are up (or the touch was cancelled).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_levels_are_one_and_two_hundredths_below_on() {
        let c = PressCfg::new(config::Action::None, config::Action::None, 0.08, 0.14);
        assert!((c.light_off - 0.07).abs() < 1e-6);
        assert!((c.deep_off - 0.12).abs() < 1e-6);
        // Never below the resting-noise floor.
        let c = PressCfg::new(config::Action::None, config::Action::None, 0.01, 0.03);
        assert!((c.light_off - 0.01).abs() < 1e-6);
        assert!((c.deep_off - 0.01).abs() < 1e-6);
    }

    #[test]
    fn the_gate_has_hysteresis_and_tolerates_float_dust() {
        let mut g = Gate::default();
        g.update(0.0699999, 0.07, 0.06, 10); // scaled from integers: still counts as on
        assert!(g.above);
        g.update(0.06, 0.07, 0.06, 20); // exactly off: stays on
        assert!(g.above);
        g.update(0.059, 0.07, 0.06, 30);
        assert!(!g.above);
        g.update(0.065, 0.07, 0.06, 40); // between off and on: stays off
        assert!(!g.above);
    }

    #[test]
    fn the_gate_needs_the_persistence_time() {
        let mut g = Gate::default();
        g.update(0.1, 0.07, 0.06, 1_000);
        assert!(!g.held(22_999, 22_000));
        assert!(g.held(23_000, 22_000));
        assert_eq!(g.due(22_000), Some(23_000));
    }

    #[test]
    fn the_signal_is_the_max_and_ignores_nonsense() {
        assert_eq!(signal(&[]), 0.0);
        assert_eq!(signal(&[0.03, 0.09, f32::NAN]), 0.09);
    }
}
