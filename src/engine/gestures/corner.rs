//! Corner triggers: one finger in a corner zone of the pad. Each corner has up to six slots:
//! three swipes, picked by the direction of the move (measured from the corner, toward the
//! middle of the pad), plus a tap, a press and a hold.
//!
//!  * horizontal: along the top or bottom edge, away from the corner (angle -20..30 degrees),
//!  * diagonal: toward the middle of the pad (30..60 degrees),
//!  * vertical: along the left or right edge, away from the corner (60..110 degrees),
//!  * tap: the finger touches down in the zone and lifts quickly without moving (the ordinary
//!    tap thresholds),
//!  * press: the pad's button clicks, or the finger presses past the one-finger light-press
//!    pressure, while the finger is in the zone,
//!  * hold: the finger rests in the zone for the long-hold time without moving.
//!
//! The finger must start inside the zone (`Corners::size_mm`, a square). A swipe must travel at
//! least `COMMIT_MM` within `WINDOW_US` of touching down. A slow or short move, a start outside
//! the zones, a second finger and a swipe that points out of the pad are all ordinary input.
//! Once a swipe, press or hold commits, the cursor is frozen until the finger lifts and the
//! slot's action runs: a one-shot action fires once; the window resize follows the finger live
//! until it lifts. A tap fires when the finger lifts.
//!
//! A set tap or press slot also takes the click it shadows: from touchdown in its zone, the
//! mouse hook holds back clicks (`Suppress::Clicks`) until the touch stops being a candidate, and
//! for a short while after a tap's lift (`CLICK_LINGER_US`), because Windows' tap-to-click may
//! arrive just after the lift. A touch that also becomes something else (a swipe, a second
//! finger, a long rest) lets the clicks go again at once. A set hold slot shadows the one-finger
//! long hold of the same touch.
//!
//! Arbitration with the edge slider: the edge slider starts from the same few millimetres, so a
//! corner zone that has any slot set belongs to the corner: an edge slide never starts there
//! (`owns_zone`). A corner whose slots are all off leaves its zone to the edge slider. As a
//! second guard, while a corner swipe can still claim the touch the edge slider holds back
//! (`would_claim`), and if both commit in the same frame the corner wins (`Priority::Corner`).

use crate::config::{Action as Cfg, CornerDir, CornerSlot};
use crate::engine::arbitration::{Owner, Proposal};
use crate::engine::types::{
    Action, CornerGestureCfg, Ctx, GestureCfg, GestureEvent, Point, Suppress, Thresholds, dist,
};

/// Travel from the landing point that commits the swipe.
pub const COMMIT_MM: f32 = 6.0;
/// ...which must happen within this long of touching down.
pub const WINDOW_US: u64 = 400_000;
/// How long the click hold outlives the lift of a tap (the tap-to-click may land just after it).
pub const CLICK_LINGER_US: u64 = 150_000;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Fresh,
    /// Landed in the zone of `corner` at `start`, at time `t0`. `fire` is the press or hold slot
    /// that was just recognised and waits for the arbiter.
    Candidate {
        corner: usize,
        start: Point,
        t0: u64,
        fire: Option<CornerSlot>,
    },
    /// Live. `last` is the position the resize has been applied up to.
    Committed {
        corner: usize,
        last: Point,
        resize: bool,
    },
    Dead,
}

#[derive(Debug, Default)]
pub struct CornerState {
    phase: Phase,
    /// The touch can no longer be a plain tap: it moved or clicked.
    tap_blocked: bool,
    /// Clicks are being held back through the mouse hook for this touch.
    clicks_held: bool,
    /// Since when the finger's pressure is at or over the press threshold.
    deep_since: Option<u64>,
    /// After a tap: let go of the click hold at this time (read by the recognizer).
    linger: Option<u64>,
}

/// Which corner zone `p` is in on a `dims` pad (0 = top-left, 1 = top-right, 2 = bottom-left,
/// 3 = bottom-right). The zone is a square of side `size`.
pub fn zone_of(p: Point, dims: (f32, f32), size: f32) -> Option<usize> {
    let (w, h) = dims;
    let size = size.min(w / 2.0).min(h / 2.0);
    let right = if p.0 <= size {
        false
    } else if p.0 >= w - size {
        true
    } else {
        return None;
    };
    let bottom = if p.1 <= size {
        false
    } else if p.1 >= h - size {
        true
    } else {
        return None;
    };
    Some(usize::from(bottom) * 2 + usize::from(right))
}

/// The slot a move of (`dx`, `dy`) mm (Y grows downward) out of `corner` points at, or `None`
/// when it points away from the pad or between the sectors.
pub fn classify(corner: usize, dx: f32, dy: f32) -> Option<CornerDir> {
    // Mirror so that the corner is the top-left one: +x goes along the top/bottom edge away from
    // the corner and +y along the side edge away from it.
    let ux = if corner.is_multiple_of(2) { dx } else { -dx };
    let uy = if corner < 2 { dy } else { -dy };
    let deg = uy.atan2(ux).to_degrees();
    if (-20.0..30.0).contains(&deg) {
        Some(CornerDir::Horizontal)
    } else if (30.0..=60.0).contains(&deg) {
        Some(CornerDir::Diagonal)
    } else if deg > 60.0 && deg <= 110.0 {
        Some(CornerDir::Vertical)
    } else {
        None
    }
}

/// True when `start` is inside the zone of a corner that has at least one slot set (of a trigger
/// that is switched on): the edge slider does not start there.
pub fn owns_zone(ctx: &Ctx, start: Point) -> bool {
    let (Some(cfg), Some(dims)) = (ctx.g.corner.as_ref(), ctx.dims) else {
        return false;
    };
    zone_of(start, dims, cfg.size_mm).is_some_and(|corner| cfg.any(corner))
}

/// True while the touch that landed at `start` at `t0` may still become a corner swipe that has
/// an action. The edge slider uses it to hold back.
pub fn would_claim(ctx: &Ctx, state: &CornerState, free: bool, start: Point, t0: u64) -> bool {
    // Already given up or running, or someone else owns the touch: nothing to wait for.
    if !free || matches!(state.phase, Phase::Dead | Phase::Committed { .. }) {
        return false;
    }
    let (Some(cfg), Some(dims), Some(&p)) = (ctx.g.corner.as_ref(), ctx.dims, ctx.cur.first())
    else {
        return false;
    };
    if ctx.now.saturating_sub(t0) > WINDOW_US {
        return false;
    }
    let Some(corner) = zone_of(start, dims, cfg.size_mm) else {
        return false;
    };
    classify(corner, p.0 - start.0, p.1 - start.1)
        .is_some_and(|dir| cfg.swipe(corner, dir).is_some())
}

impl CornerState {
    pub fn is_committed(&self) -> bool {
        matches!(self.phase, Phase::Committed { .. })
    }

    /// Clicks are held back for the current touch.
    pub fn clicks_held(&self) -> bool {
        self.clicks_held
    }

    /// Take the time at which the click hold of a finished tap ends.
    pub fn take_linger(&mut self) -> Option<u64> {
        self.linger.take()
    }

    /// True while this touch is (or became) a corner touch whose hold slot is set: the ordinary
    /// one-finger long hold stays quiet.
    pub fn claims_hold(&self, g: &GestureCfg) -> bool {
        let (Phase::Candidate { corner, .. } | Phase::Committed { corner, .. }) = self.phase else {
            return false;
        };
        g.corner
            .as_ref()
            .is_some_and(|c| c.slot(corner, CornerSlot::Hold).is_some())
    }

    /// When the hold slot of the current candidate becomes due, if it has one.
    pub fn deadline(&self, g: &GestureCfg, th: &Thresholds) -> Option<u64> {
        let Phase::Candidate {
            corner, t0, fire, ..
        } = self.phase
        else {
            return None;
        };
        let cfg = g.corner.as_ref()?;
        (fire.is_none() && cfg.slot(corner, CornerSlot::Hold).is_some()).then(|| t0 + th.hold_us)
    }

    /// Give up on this touch. Clicks held back for it are let go.
    fn die(&mut self, out: &mut Vec<Action>) {
        self.phase = Phase::Dead;
        if self.clicks_held {
            self.clicks_held = false;
            out.push(Action::Suppress(Suppress::Off));
        }
    }

    /// Follow the finger while a resize is live; give up when a second finger arrives.
    pub fn drive(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let Phase::Committed {
            corner,
            last,
            resize,
        } = self.phase
        else {
            return;
        };
        if ctx.raw != 1 || ctx.rebase {
            self.phase = Phase::Dead;
            self.clicks_held = false;
            if resize {
                out.push(Action::CornerResizeEnd);
            }
            out.push(Action::Suppress(Suppress::Off));
            return;
        }
        if !resize {
            return;
        }
        let p = ctx.cur[0];
        let (dx_mm, dy_mm) = (p.0 - last.0, p.1 - last.1);
        if dx_mm != 0.0 || dy_mm != 0.0 {
            self.phase = Phase::Committed {
                corner,
                last: p,
                resize,
            };
            out.push(Action::CornerResizeMove { dx_mm, dy_mm });
        }
    }

    /// Watch a fresh or candidate touch; propose once a swipe, press or hold is clear.
    pub fn step(&mut self, ctx: &Ctx, free: bool, out: &mut Vec<Action>) -> Option<Proposal> {
        let cfg = ctx.g.corner.as_ref()?;
        match self.phase {
            Phase::Fresh => {
                let (Some(dims), Some(&start)) = (ctx.dims, ctx.cur.first()) else {
                    self.phase = Phase::Dead;
                    return None;
                };
                self.tap_blocked = false;
                self.deep_since = None;
                if ctx.raw != 1 || ctx.rebase {
                    self.phase = Phase::Dead;
                    return None;
                }
                match zone_of(start, dims, cfg.size_mm) {
                    Some(corner) if cfg.any(corner) => {
                        self.phase = Phase::Candidate {
                            corner,
                            start,
                            t0: ctx.now,
                            fire: None,
                        };
                        // A tap or press here owns the click: hold clicks back from touchdown.
                        if cfg.slot(corner, CornerSlot::Tap).is_some()
                            || cfg.slot(corner, CornerSlot::Press).is_some()
                        {
                            self.clicks_held = true;
                            out.push(Action::Suppress(Suppress::Clicks));
                        }
                        // The landing frame may already carry the button.
                        self.watch(ctx, free, cfg, corner, start, ctx.now)
                    }
                    _ => {
                        self.phase = Phase::Dead;
                        None
                    }
                }
            }
            Phase::Candidate {
                corner,
                start,
                t0,
                fire,
            } => {
                if ctx.raw != 1 || ctx.rebase {
                    self.die(out);
                    return None;
                }
                if fire.is_some() {
                    // Waiting for the arbiter: propose again until it decides.
                    return free.then_some(Proposal {
                        who: Owner::Corner,
                        at_us: ctx.now,
                    });
                }
                let p = *ctx.cur.first()?;
                if ctx.max_travel >= ctx.th.tap_travel_mm {
                    self.tap_blocked = true;
                }
                if let Some(prop) = self.watch(ctx, free, cfg, corner, start, t0) {
                    return Some(prop);
                }
                if !ctx.from_frame {
                    return None;
                }
                let age = ctx.now.saturating_sub(t0);
                let in_time = age <= WINDOW_US;
                // A set hold slot keeps the touch alive until the hold is due.
                let alive_for = if cfg.slot(corner, CornerSlot::Hold).is_some() {
                    WINDOW_US.max(ctx.th.hold_us)
                } else {
                    WINDOW_US
                };
                if dist(p, start) < COMMIT_MM {
                    if age > alive_for {
                        // Too slow: ordinary input.
                        self.die(out);
                    }
                    return None;
                }
                let bound = in_time
                    && classify(corner, p.0 - start.0, p.1 - start.1)
                        .is_some_and(|dir| cfg.swipe(corner, dir).is_some());
                if bound && free {
                    return Some(Proposal {
                        who: Owner::Corner,
                        at_us: ctx.now,
                    });
                }
                self.die(out);
                None
            }
            Phase::Committed { .. } | Phase::Dead => None,
        }
    }

    /// Look for a press or a hold in a candidate touch; mark the slot and propose.
    fn watch(
        &mut self,
        ctx: &Ctx,
        free: bool,
        cfg: &CornerGestureCfg,
        corner: usize,
        start: Point,
        t0: u64,
    ) -> Option<Proposal> {
        let mut fire = None;
        if ctx.button {
            // A click is not a tap, whatever the slots.
            self.tap_blocked = true;
        }
        if cfg.slot(corner, CornerSlot::Press).is_some() {
            let deep = ctx
                .pressures
                .first()
                .is_some_and(|&p| p >= cfg.press_threshold);
            if deep {
                let since = *self.deep_since.get_or_insert(ctx.now);
                if ctx.now.saturating_sub(since) >= ctx.th.press_persist_us {
                    fire = Some(CornerSlot::Press);
                }
            } else {
                self.deep_since = None;
            }
            if ctx.button && ctx.from_frame {
                fire = Some(CornerSlot::Press);
            }
        }
        if fire.is_none()
            && cfg.slot(corner, CornerSlot::Hold).is_some()
            && ctx.max_travel < ctx.th.hold_travel_mm
            && ctx.now.saturating_sub(t0) >= ctx.th.hold_us
            && dist(*ctx.cur.first()?, start) < ctx.th.hold_travel_mm
        {
            fire = Some(CornerSlot::Hold);
        }
        let slot = fire?;
        self.phase = Phase::Candidate {
            corner,
            start,
            t0,
            fire: Some(slot),
        };
        free.then_some(Proposal {
            who: Owner::Corner,
            at_us: ctx.now,
        })
    }

    /// The arbiter gave this touch the session: freeze the cursor and run the slot's action.
    pub fn commit(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let Phase::Candidate {
            corner,
            start,
            fire,
            ..
        } = self.phase
        else {
            return;
        };
        let (Some(cfg), Some(&p)) = (ctx.g.corner.as_ref(), ctx.cur.first()) else {
            self.die(out);
            return;
        };
        let slot = match fire {
            Some(s) => s,
            None => match classify(corner, p.0 - start.0, p.1 - start.1) {
                Some(dir) => CornerSlot::Swipe(dir),
                None => {
                    self.die(out);
                    return;
                }
            },
        };
        let Some(action) = cfg.slot(corner, slot) else {
            self.die(out);
            return;
        };
        let swipe = matches!(slot, CornerSlot::Swipe(_));
        let resize = swipe && *action == Cfg::ResizeFromOpposite;
        self.phase = Phase::Committed {
            corner,
            last: start,
            resize,
        };
        out.push(Action::Debug(GestureEvent::Corner { corner, slot }));
        // A press or hold keeps the click hold; a swipe lets the clicks go.
        let keep_clicks = self.clicks_held && !swipe;
        self.clicks_held = keep_clicks;
        out.push(Action::Suppress(if keep_clicks {
            Suppress::CursorAndClicks
        } else {
            Suppress::Cursor
        }));
        if resize {
            out.push(Action::CornerResizeBegin { corner });
            self.drive(ctx, out);
        } else {
            out.push(Action::Trigger(action.clone()));
        }
    }

    /// The finger lifted: a quick, still touch in a zone with a tap slot fires it. Returns
    /// whether a tap fired (the click hold then lingers, see `release`).
    pub fn finish_tap(
        &mut self,
        g: &GestureCfg,
        th: &Thresholds,
        now: u64,
        free: bool,
        out: &mut Vec<Action>,
    ) -> bool {
        let Phase::Candidate {
            corner, t0, fire, ..
        } = self.phase
        else {
            return false;
        };
        let Some(action) = g
            .corner
            .as_ref()
            .and_then(|c| c.slot(corner, CornerSlot::Tap))
        else {
            return false;
        };
        if fire.is_some() || self.tap_blocked || !free || now.saturating_sub(t0) >= th.tap_max_us {
            return false;
        }
        out.push(Action::Debug(GestureEvent::Corner {
            corner,
            slot: CornerSlot::Tap,
        }));
        out.push(Action::Trigger(action.clone()));
        true
    }

    /// The touch ended or was cancelled: end a live resize and release the cursor and the
    /// clicks. After a tap (`linger_until`) the clicks are let go later, by the recognizer.
    pub fn release(&mut self, out: &mut Vec<Action>, linger_until: Option<u64>) {
        match self.phase {
            Phase::Committed { resize, .. } => {
                if resize {
                    out.push(Action::CornerResizeEnd);
                }
                out.push(Action::Suppress(Suppress::Off));
            }
            _ if self.clicks_held => match linger_until {
                Some(at) => self.linger = Some(at),
                None => out.push(Action::Suppress(Suppress::Off)),
            },
            _ => {}
        }
        self.clicks_held = false;
        self.deep_since = None;
        self.tap_blocked = false;
        self.phase = Phase::Fresh;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, EdgeTarget};
    use crate::engine::Frame;
    use crate::engine::recognizer::Recognizer;
    use crate::input::Contact;

    const DIMS: (f32, f32) = (160.0, 115.0);

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

    fn cfg_with(corner: usize, h: Cfg, v: Cfg, d: Cfg) -> Config {
        let mut cfg = Config::default();
        cfg.corner.enabled = true;
        let c = &mut cfg.corner.corners[corner];
        c.horizontal = h;
        c.vertical = v;
        c.diagonal = d;
        cfg
    }

    /// One finger lands at `from`, moves in a straight line to `to` over `ms` and (optionally)
    /// lifts.
    fn swipe(from: Point, to: Point, ms: u64, lift: bool) -> Vec<Frame> {
        let mut v = vec![frame(0, vec![c(1, from.0, from.1)])];
        for i in 1..=10u64 {
            let f = i as f32 / 10.0;
            v.push(frame(
                ms * i / 10,
                vec![c(
                    1,
                    from.0 + (to.0 - from.0) * f,
                    from.1 + (to.1 - from.1) * f,
                )],
            ));
        }
        if lift {
            v.push(frame(ms + 10, vec![]));
        }
        v
    }

    fn run(frames: &[Frame], cfg: &Config) -> Vec<Action> {
        let mut r = Recognizer::default();
        r.set_pad_dims(Some(DIMS));
        let mut out = Vec::new();
        for f in frames {
            out.extend(r.step(f, cfg));
        }
        out
    }

    fn triggers(out: &[Action]) -> Vec<Cfg> {
        out.iter()
            .filter_map(|a| match a {
                Action::Trigger(a) => Some(a.clone()),
                _ => None,
            })
            .collect()
    }

    fn suppress(out: &[Action]) -> Vec<Suppress> {
        out.iter()
            .filter_map(|a| match a {
                Action::Suppress(s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    /// Rest 200 ms (the edge dwell) at the top-left, then slide right along the top edge.
    fn rest_then_slide_along_top() -> Vec<Frame> {
        let mut fr = Vec::new();
        for ms in (0..=200).step_by(10) {
            fr.push(frame(ms, vec![c(1, 3.0, 3.0)]));
        }
        for i in 1..=10u64 {
            fr.push(frame(200 + i * 10, vec![c(1, 3.0 + 2.0 * i as f32, 3.0)]));
        }
        fr.push(frame(400, vec![]));
        fr
    }

    #[test]
    fn classification_covers_all_twelve_directions() {
        for corner in 0usize..4 {
            let sx = if corner.is_multiple_of(2) { 1.0 } else { -1.0 };
            let sy = if corner < 2 { 1.0 } else { -1.0 };
            let at = |deg: f32| {
                let (s, co) = deg.to_radians().sin_cos();
                classify(corner, 10.0 * co * sx, 10.0 * s * sy)
            };
            assert_eq!(at(0.0), Some(CornerDir::Horizontal), "corner {corner}");
            assert_eq!(at(15.0), Some(CornerDir::Horizontal), "corner {corner}");
            assert_eq!(at(45.0), Some(CornerDir::Diagonal), "corner {corner}");
            assert_eq!(at(75.0), Some(CornerDir::Vertical), "corner {corner}");
            assert_eq!(at(90.0), Some(CornerDir::Vertical), "corner {corner}");
        }
    }

    #[test]
    fn classification_boundary_angles() {
        let at = |deg: f32| {
            let (s, co) = deg.to_radians().sin_cos();
            classify(0, 10.0 * co, 10.0 * s)
        };
        assert_eq!(at(-19.0), Some(CornerDir::Horizontal));
        assert_eq!(at(-21.0), None);
        assert_eq!(at(29.0), Some(CornerDir::Horizontal));
        assert_eq!(at(31.0), Some(CornerDir::Diagonal));
        assert_eq!(at(59.0), Some(CornerDir::Diagonal));
        assert_eq!(at(61.0), Some(CornerDir::Vertical));
        assert_eq!(at(109.0), Some(CornerDir::Vertical));
        assert_eq!(at(111.0), None);
        // Straight out of the pad, and straight back along an edge.
        assert_eq!(classify(0, -10.0, 0.0), None);
        assert_eq!(classify(0, 0.0, -10.0), None);
        assert_eq!(classify(3, 10.0, 10.0), None);
    }

    #[test]
    fn zones_are_squares_in_the_four_corners() {
        assert_eq!(zone_of((5.0, 5.0), DIMS, 12.0), Some(0));
        assert_eq!(zone_of((155.0, 5.0), DIMS, 12.0), Some(1));
        assert_eq!(zone_of((5.0, 110.0), DIMS, 12.0), Some(2));
        assert_eq!(zone_of((155.0, 110.0), DIMS, 12.0), Some(3));
        assert_eq!(zone_of((5.0, 60.0), DIMS, 12.0), None);
        assert_eq!(zone_of((80.0, 5.0), DIMS, 12.0), None);
        assert_eq!(zone_of((13.0, 5.0), DIMS, 12.0), None);
    }

    #[test]
    fn each_slot_fires_its_own_action() {
        let cfg = cfg_with(0, Cfg::Find, Cfg::ShowDesktop, Cfg::TaskView);
        let from = (3.0, 3.0);
        let cases = [
            ((23.0, 3.0), Cfg::Find),
            ((3.0, 23.0), Cfg::ShowDesktop),
            ((17.0, 17.0), Cfg::TaskView),
        ];
        for (to, want) in cases {
            let out = run(&swipe(from, to, 200, true), &cfg);
            assert_eq!(triggers(&out), vec![want], "to {to:?}");
            assert_eq!(
                suppress(&out),
                vec![Suppress::Cursor, Suppress::Off],
                "the cursor is frozen while the finger is down"
            );
        }
    }

    #[test]
    fn mirrored_corners_fire_too() {
        let cfg = cfg_with(3, Cfg::Find, Cfg::None, Cfg::None);
        let out = run(&swipe((157.0, 112.0), (137.0, 112.0), 200, true), &cfg);
        assert_eq!(triggers(&out), vec![Cfg::Find]);
    }

    #[test]
    fn off_slot_does_nothing() {
        let cfg = cfg_with(0, Cfg::Find, Cfg::None, Cfg::None);
        let out = run(&swipe((3.0, 3.0), (3.0, 23.0), 200, true), &cfg);
        assert!(triggers(&out).is_empty());
        assert!(suppress(&out).is_empty());
    }

    #[test]
    fn slow_short_and_outside_starts_do_not_fire() {
        let cfg = cfg_with(0, Cfg::Find, Cfg::Find, Cfg::Find);
        // Too slow: 6 mm only after 0.6 s.
        let out = run(&swipe((3.0, 3.0), (23.0, 3.0), 2000, true), &cfg);
        assert!(triggers(&out).is_empty(), "slow");
        // Too short: stays under 6 mm.
        let out = run(&swipe((3.0, 3.0), (8.0, 3.0), 200, true), &cfg);
        assert!(triggers(&out).is_empty(), "short");
        // Starts outside the zone.
        let out = run(&swipe((40.0, 3.0), (60.0, 3.0), 200, true), &cfg);
        assert!(triggers(&out).is_empty(), "outside");
        let out = run(&swipe((3.0, 60.0), (3.0, 80.0), 200, true), &cfg);
        assert!(triggers(&out).is_empty(), "side edge, not a corner");
    }

    #[test]
    fn swipe_pointing_out_of_the_pad_does_nothing() {
        let cfg = cfg_with(0, Cfg::Find, Cfg::Find, Cfg::Find);
        let out = run(&swipe((8.0, 8.0), (0.0, 0.0), 200, true), &cfg);
        assert!(triggers(&out).is_empty());
    }

    #[test]
    fn second_finger_cancels_the_swipe() {
        let cfg = cfg_with(0, Cfg::Find, Cfg::Find, Cfg::Find);
        let mut fr = swipe((3.0, 3.0), (3.0, 3.0), 50, false);
        fr.push(frame(100, vec![c(1, 3.0, 3.0), c(2, 80.0, 60.0)]));
        fr.push(frame(150, vec![c(1, 20.0, 3.0), c(2, 80.0, 60.0)]));
        let out = run(&fr, &cfg);
        assert!(triggers(&out).is_empty());
    }

    #[test]
    fn disabled_corners_do_nothing() {
        let mut cfg = cfg_with(0, Cfg::Find, Cfg::Find, Cfg::Find);
        cfg.corner.enabled = false;
        let out = run(&swipe((3.0, 3.0), (23.0, 3.0), 200, true), &cfg);
        assert!(triggers(&out).is_empty());
    }

    #[test]
    fn the_zone_size_is_honoured() {
        let mut cfg = cfg_with(0, Cfg::Find, Cfg::None, Cfg::None);
        cfg.corner.size_mm = 6.0;
        let out = run(&swipe((9.0, 3.0), (29.0, 3.0), 200, true), &cfg);
        assert!(triggers(&out).is_empty(), "outside the small zone");
        cfg.corner.size_mm = 12.0;
        let out = run(&swipe((9.0, 3.0), (29.0, 3.0), 200, true), &cfg);
        assert_eq!(triggers(&out), vec![Cfg::Find]);
    }

    #[test]
    fn corner_wins_over_the_edge_slider_when_the_slot_is_set() {
        let mut cfg = cfg_with(0, Cfg::Find, Cfg::None, Cfg::None);
        cfg.edge.enabled = true;
        cfg.edge.top = EdgeTarget::Volume;
        cfg.edge.left = EdgeTarget::Volume;
        let out = run(&rest_then_slide_along_top(), &cfg);
        assert_eq!(triggers(&out), vec![Cfg::Find]);
        assert!(
            !out.iter().any(|a| matches!(a, Action::EdgeStep { .. })),
            "the edge slider stayed out of it"
        );
    }

    #[test]
    fn edge_never_starts_in_a_corner_zone_that_has_a_slot() {
        // Only the vertical slot is set, and the slide goes along the top: not the corner's
        // swipe, but its zone is still kept from the edge slider.
        let mut cfg = cfg_with(0, Cfg::None, Cfg::Find, Cfg::None);
        cfg.edge.enabled = true;
        cfg.edge.top = EdgeTarget::Volume;
        let out = run(&rest_then_slide_along_top(), &cfg);
        assert!(triggers(&out).is_empty());
        assert!(!out.iter().any(|a| matches!(a, Action::EdgeStep { .. })));
    }

    #[test]
    fn edge_takes_the_zone_of_a_corner_with_every_slot_off() {
        // Another corner is set; the top-left one is all off.
        let mut cfg = cfg_with(1, Cfg::Find, Cfg::Find, Cfg::Find);
        cfg.edge.enabled = true;
        cfg.edge.top = EdgeTarget::Volume;
        let out = run(&rest_then_slide_along_top(), &cfg);
        assert!(triggers(&out).is_empty());
        assert!(out.iter().any(|a| matches!(a, Action::EdgeStep { .. })));
    }

    #[test]
    fn resize_lifecycle_begin_move_end() {
        let cfg = cfg_with(0, Cfg::None, Cfg::None, Cfg::ResizeFromOpposite);
        let mut fr = swipe((3.0, 3.0), (13.0, 13.0), 100, false);
        for i in 1..=5u64 {
            let p = 13.0 + 2.0 * i as f32;
            fr.push(frame(100 + i * 20, vec![c(1, p, p)]));
        }
        fr.push(frame(300, vec![]));
        let out = run(&fr, &cfg);
        let kinds: Vec<&str> = out
            .iter()
            .filter_map(|a| match a {
                Action::CornerResizeBegin { corner: 0 } => Some("begin"),
                Action::CornerResizeMove { .. } => Some("move"),
                Action::CornerResizeEnd => Some("end"),
                _ => None,
            })
            .collect();
        assert_eq!(kinds.first(), Some(&"begin"));
        assert_eq!(kinds.last(), Some(&"end"));
        assert_eq!(kinds.iter().filter(|k| **k == "end").count(), 1);
        assert!(kinds.iter().filter(|k| **k == "move").count() >= 5);
        // The moves add up to the travel from the landing point (no Trigger for a resize).
        let (sx, sy) = out.iter().fold((0.0, 0.0), |(x, y), a| match a {
            Action::CornerResizeMove { dx_mm, dy_mm } => (x + dx_mm, y + dy_mm),
            _ => (x, y),
        });
        assert!(sx > 10.0 && sy > 10.0, "{sx} {sy}");
        assert!(triggers(&out).is_empty());
        assert_eq!(suppress(&out), vec![Suppress::Cursor, Suppress::Off]);
    }

    #[test]
    fn a_second_finger_ends_the_resize_once() {
        let cfg = cfg_with(0, Cfg::None, Cfg::None, Cfg::ResizeFromOpposite);
        let mut fr = swipe((3.0, 3.0), (13.0, 13.0), 100, false);
        fr.push(frame(150, vec![c(1, 14.0, 14.0), c(2, 80.0, 60.0)]));
        fr.push(frame(200, vec![c(1, 15.0, 15.0), c(2, 80.0, 60.0)]));
        fr.push(frame(250, vec![]));
        let out = run(&fr, &cfg);
        let ends = out
            .iter()
            .filter(|a| matches!(a, Action::CornerResizeEnd))
            .count();
        assert_eq!(ends, 1);
    }

    // ---- tap / press / hold -------------------------------------------------------------

    fn pressed(ms: u64, x: f32, y: f32, button: bool, pressure: Option<f32>) -> Frame {
        let mut ct = c(1, x, y);
        ct.pressure = pressure;
        Frame {
            t_us: ms * 1000,
            button,
            contacts: vec![ct],
        }
    }

    /// One finger resting at `at` from 0 to `rest_ms` (every 10 ms), then lifted.
    fn rest(at: Point, rest_ms: u64) -> Vec<Frame> {
        let mut v: Vec<Frame> = (0..=rest_ms / 10)
            .map(|i| frame(i * 10, vec![c(1, at.0, at.1)]))
            .collect();
        v.push(frame(rest_ms + 10, vec![]));
        v
    }

    fn with_slot(corner: usize, slot: CornerSlot, a: Cfg) -> Config {
        let mut cfg = Config::default();
        cfg.corner.enabled = true;
        *cfg.corner.corners[corner].get_mut(slot) = a;
        *cfg.corner.trigger_enabled_mut(slot.trigger()) = true;
        cfg
    }

    #[test]
    fn a_quick_still_touch_taps_and_the_click_hold_outlives_the_lift() {
        let cfg = with_slot(2, CornerSlot::Tap, Cfg::Find);
        let mut r = Recognizer::default();
        r.set_pad_dims(Some(DIMS));
        let mut out = Vec::new();
        for f in rest((4.0, 110.0), 80) {
            out.extend(r.step(&f, &cfg));
        }
        assert_eq!(triggers(&out), vec![Cfg::Find], "one tap");
        // Clicks are held from touchdown; they are not let go at the lift.
        assert_eq!(suppress(&out), vec![Suppress::Clicks]);
        assert!(out.iter().any(|a| matches!(
            a,
            Action::Debug(GestureEvent::Corner {
                corner: 2,
                slot: CornerSlot::Tap
            })
        )));
        // ...but a moment later, by the timer.
        let due = r.next_deadline().expect("a deadline for the hold release");
        assert!(due >= 90_000 + CLICK_LINGER_US - 20_000);
        assert!(suppress(&r.tick(due - 1)).is_empty());
        assert_eq!(suppress(&r.tick(due)), vec![Suppress::Off]);
        assert!(r.tick(due + 1000).is_empty(), "only once");
    }

    #[test]
    fn a_tap_needs_a_still_and_quick_touch() {
        let cfg = with_slot(0, CornerSlot::Tap, Cfg::Find);
        // Moves 4 mm: more than the tap travel.
        let out = run(&swipe((3.0, 3.0), (7.0, 3.0), 100, true), &cfg);
        assert!(triggers(&out).is_empty(), "moved");
        // Too long: a rest of 300 ms.
        let out = run(&rest((3.0, 3.0), 300), &cfg);
        assert!(triggers(&out).is_empty(), "slow");
        // Outside the zone.
        let out = run(&rest((60.0, 60.0), 80), &cfg);
        assert!(triggers(&out).is_empty(), "outside");
        // Another corner, with nothing set.
        let out = run(&rest((156.0, 4.0), 80), &cfg);
        assert!(triggers(&out).is_empty(), "other corner");
        // Just inside and just outside the zone edge.
        assert_eq!(
            triggers(&run(&rest((11.9, 3.0), 80), &cfg)),
            vec![Cfg::Find]
        );
        assert!(triggers(&run(&rest((12.5, 3.0), 80), &cfg)).is_empty());
    }

    #[test]
    fn a_tap_that_clicked_is_not_a_tap() {
        let cfg = with_slot(0, CornerSlot::Tap, Cfg::Find);
        let fr = vec![
            pressed(0, 3.0, 3.0, false, None),
            pressed(30, 3.0, 3.0, true, None),
            pressed(60, 3.0, 3.0, true, None),
            frame(80, vec![]),
        ];
        assert!(triggers(&run(&fr, &cfg)).is_empty());
    }

    #[test]
    fn a_second_finger_cancels_a_tap_and_lets_the_clicks_go() {
        let cfg = with_slot(0, CornerSlot::Tap, Cfg::Find);
        let fr = vec![
            frame(0, vec![c(1, 3.0, 3.0)]),
            frame(30, vec![c(1, 3.0, 3.0), c(2, 80.0, 60.0)]),
            frame(60, vec![c(1, 3.0, 3.0), c(2, 80.0, 60.0)]),
            frame(90, vec![]),
        ];
        let out = run(&fr, &cfg);
        assert!(triggers(&out).is_empty());
        assert_eq!(suppress(&out), vec![Suppress::Clicks, Suppress::Off]);
    }

    #[test]
    fn a_hold_fires_once_after_the_hold_time_and_not_before() {
        let cfg = with_slot(3, CornerSlot::Hold, Cfg::Find);
        let hold_ms = u64::from(cfg.sensitivity.hold_ms);
        let at = (157.0, 112.0);
        let out = run(&rest(at, hold_ms - 60), &cfg);
        assert!(triggers(&out).is_empty(), "too short");
        let out = run(&rest(at, hold_ms + 300), &cfg);
        assert_eq!(triggers(&out), vec![Cfg::Find], "once");
        assert_eq!(suppress(&out), vec![Suppress::Cursor, Suppress::Off]);
        // A finger that shifts 4 mm early on is not holding still.
        let mut fr = vec![frame(0, vec![c(1, 157.0, 112.0)])];
        for ms in (50..=hold_ms + 300).step_by(10) {
            fr.push(frame(ms, vec![c(1, 153.0, 112.0)]));
        }
        fr.push(frame(hold_ms + 400, vec![]));
        let out = run(&fr, &cfg);
        assert!(triggers(&out).is_empty(), "moved 4 mm");
    }

    #[test]
    fn the_hold_slot_shadows_the_one_finger_long_hold() {
        let mut cfg = with_slot(0, CornerSlot::Hold, Cfg::Find);
        cfg.long_hold.enabled = true;
        *cfg.long_hold.actions.get_mut(0) = Cfg::TaskView;
        let hold_ms = u64::from(cfg.sensitivity.hold_ms);
        let out = run(&rest((3.0, 3.0), hold_ms + 300), &cfg);
        assert_eq!(triggers(&out), vec![Cfg::Find]);
        // Outside the zone the ordinary long hold still works.
        let out = run(&rest((80.0, 60.0), hold_ms + 300), &cfg);
        assert_eq!(triggers(&out), vec![Cfg::TaskView]);
    }

    #[test]
    fn a_second_finger_cancels_a_hold() {
        let cfg = with_slot(0, CornerSlot::Hold, Cfg::Find);
        let mut fr = Vec::new();
        for i in 0..=80u64 {
            let mut contacts = vec![c(1, 3.0, 3.0)];
            if i >= 20 {
                contacts.push(c(2, 80.0, 60.0));
            }
            fr.push(frame(i * 10, contacts));
        }
        fr.push(frame(900, vec![]));
        assert!(triggers(&run(&fr, &cfg)).is_empty());
    }

    #[test]
    fn a_physical_click_in_the_zone_fires_press_once_and_no_tap() {
        let mut cfg = with_slot(2, CornerSlot::Press, Cfg::Find);
        cfg.corner.corners[2].tap = Cfg::TaskView;
        cfg.corner.tap_enabled = true;
        let mut fr = vec![pressed(0, 4.0, 110.0, false, None)];
        for ms in [30, 60, 90, 120] {
            fr.push(pressed(ms, 4.0, 110.0, true, None));
        }
        fr.push(frame(150, vec![]));
        let out = run(&fr, &cfg);
        assert_eq!(triggers(&out), vec![Cfg::Find]);
        // Clicks are held from touchdown and stay held with the cursor until the lift.
        assert_eq!(
            suppress(&out),
            vec![Suppress::Clicks, Suppress::CursorAndClicks, Suppress::Off]
        );
    }

    #[test]
    fn a_click_outside_every_zone_is_not_touched() {
        let cfg = with_slot(2, CornerSlot::Press, Cfg::Find);
        let fr = vec![
            pressed(0, 80.0, 60.0, false, None),
            pressed(30, 80.0, 60.0, true, None),
            frame(90, vec![]),
        ];
        let out = run(&fr, &cfg);
        assert!(triggers(&out).is_empty());
        assert!(suppress(&out).is_empty());
    }

    #[test]
    fn pressing_deep_in_the_zone_counts_as_a_press() {
        let cfg = with_slot(1, CornerSlot::Press, Cfg::Find);
        let thr = cfg.press.thresholds(1).0;
        let mut fr = vec![pressed(0, 156.0, 4.0, false, Some(thr * 0.3))];
        for ms in [20, 40, 60, 80, 100] {
            fr.push(pressed(ms, 156.0, 4.0, false, Some(thr * 1.5)));
        }
        fr.push(frame(140, vec![]));
        assert_eq!(triggers(&run(&fr, &cfg)), vec![Cfg::Find]);
        // A light touch never does.
        let mut fr = vec![pressed(0, 156.0, 4.0, false, Some(thr * 0.3))];
        for ms in [20, 40, 60, 80, 100] {
            fr.push(pressed(ms, 156.0, 4.0, false, Some(thr * 0.5)));
        }
        fr.push(frame(140, vec![]));
        assert!(triggers(&run(&fr, &cfg)).is_empty());
    }

    #[test]
    fn a_trigger_with_its_switch_off_does_nothing_and_does_not_own_the_zone() {
        let mut cfg = with_slot(0, CornerSlot::Hold, Cfg::Find);
        cfg.corner.hold_enabled = false;
        cfg.edge.enabled = true;
        cfg.edge.top = EdgeTarget::Volume;
        let hold_ms = u64::from(cfg.sensitivity.hold_ms);
        let out = run(&rest((3.0, 3.0), hold_ms + 300), &cfg);
        assert!(triggers(&out).is_empty());
        // The edge slider is free to start in that zone.
        let out = run(&rest_then_slide_along_top(), &cfg);
        assert!(out.iter().any(|a| matches!(a, Action::EdgeStep { .. })));
    }

    #[test]
    fn a_set_tap_slot_keeps_the_edge_slider_out_of_its_zone() {
        let mut cfg = with_slot(0, CornerSlot::Tap, Cfg::Find);
        cfg.edge.enabled = true;
        cfg.edge.top = EdgeTarget::Volume;
        let out = run(&rest_then_slide_along_top(), &cfg);
        assert!(!out.iter().any(|a| matches!(a, Action::EdgeStep { .. })));
    }

    #[test]
    fn swipes_still_work_next_to_a_tap_slot_and_release_the_clicks() {
        let mut cfg = cfg_with(0, Cfg::Find, Cfg::None, Cfg::None);
        cfg.corner.corners[0].tap = Cfg::TaskView;
        cfg.corner.tap_enabled = true;
        let out = run(&swipe((3.0, 3.0), (23.0, 3.0), 200, true), &cfg);
        assert_eq!(triggers(&out), vec![Cfg::Find]);
        assert_eq!(
            suppress(&out),
            vec![Suppress::Clicks, Suppress::Cursor, Suppress::Off]
        );
    }
}
