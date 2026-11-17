//! Multi-finger gestures, keyed by finger count (3, 4 or 5): tap, four swipe directions and
//! pinch in / out. One machine serves every count; the count only picks the bindings.
//!
//! A gesture fires once per touch session and only if its binding is not `None` (an unbound
//! gesture does not compete with the others).

use super::super::arbitration::{Owner, Proposal};
use super::super::types::{Action, Ctx, GestureCfg, GestureEvent, MultiKind, Point, spread_of};
use crate::config;

/// A recognised gesture, ready to commit.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiHit {
    pub proposal: Proposal,
    pub fingers: usize,
    pub kind: MultiKind,
    pub action: config::Action,
    /// Mean finger distance from the centroid when the gesture was recognised (0 for a tap).
    spread: f32,
    /// Centroid of the fingers when the gesture was recognised.
    at: Point,
}

/// A window zoom in progress: the pinch that started it keeps streaming its spread ratio.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ZoomRun {
    fingers: usize,
    /// The spread when the zoom began: the ratio is measured from here, so the window does not
    /// jump by the part of the pinch that was needed to recognise it.
    base_spread: f32,
}

/// Travel against the last direction (mm) that counts as a reversal. Smaller wobbles are jitter.
const REVERSE_MM: f32 = 0.5;
/// Steps one frame may send at most (a glitch must not flood the window switcher).
const MAX_STEPS_PER_FRAME: i32 = 4;

/// An app switch scrub in progress: the swipe that started it keeps stepping while it moves.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ScrubRun {
    fingers: usize,
    /// The swipe was up / down (the centroid's Y is followed), else left / right (X).
    vertical: bool,
    /// +1 when the swipe's own direction is increasing coordinates, -1 otherwise, so that
    /// "forward" is always positive.
    sign: f32,
    /// Centroid coordinate when the scrub began.
    origin: f32,
    /// Position (forward positive, from `origin`) the step count is measured from.
    done: f32,
    /// Position at the last movement of at least `REVERSE_MM`, and its direction (0 = none yet).
    last: f32,
    dir: f32,
    /// Position (forward positive, from `origin`) at the last frame, so a rebase can keep it.
    prev: f32,
}

impl ScrubRun {
    /// The contacts changed under the scrub (a finger swapped for another in the same frame):
    /// move `origin` so the centroid's jump is not counted as travel.
    fn rebase(&mut self, c: Point) {
        let pos = if self.vertical { c.1 } else { c.0 };
        self.origin = pos - self.prev * self.sign;
    }

    /// The steps for a centroid now: positive = forward. The remainder of a step is kept while
    /// the fingers keep their direction; on a reversal the count restarts from the turning
    /// point, so the way back needs one step of travel and never more.
    fn advance(&mut self, c: Point, step_mm: f32) -> i32 {
        let step = step_mm.max(1.0);
        let pos = if self.vertical { c.1 } else { c.0 };
        let along = (pos - self.origin) * self.sign;
        self.prev = along;
        let d = along - self.last;
        if d.abs() >= REVERSE_MM {
            let dir = d.signum();
            if self.dir != 0.0 && dir != self.dir {
                self.done = self.last;
            }
            self.dir = dir;
            self.last = along;
        }
        let n = ((along - self.done) / step).trunc() as i32;
        self.done += n as f32 * step;
        n
    }
}

#[derive(Debug, Default)]
pub struct MultiState {
    /// Centroid and spread when the counted fingers were first all down.
    start: Option<(Point, f32)>,
    fired: bool,
    zoom: Option<ZoomRun>,
    scrub: Option<ScrubRun>,
}

impl MultiState {
    /// Run one step: look for a pinch or swipe of the current finger count.
    pub fn step(&mut self, ctx: &Ctx, free: bool) -> Option<MultiHit> {
        let n = ctx.stable;
        let Some(cfg) = ctx.g.multi_for(n) else {
            self.start = None;
            return None;
        };
        let Some(c) = ctx
            .centroid
            .filter(|_| ctx.raw == n && ctx.stable == n && !ctx.rebase)
        else {
            self.start = None;
            return None;
        };
        if self.fired || !free {
            return None;
        }
        let spread = spread_of(ctx.cur, c);
        let (start_c, start_spread) = *self.start.get_or_insert((c, spread));
        if start_spread < 1.0 {
            return None;
        }
        let th = ctx.th;
        let ratio = spread / start_spread;
        let delta = spread - start_spread;
        let (lo, hi) = th.multi_swipe_band;
        let (dx, dy) = (c.0 - start_c.0, c.1 - start_c.1);
        let (ax, ay) = (dx.abs(), dy.abs());
        let kind = if ratio <= th.multi_pinch_in && -delta >= th.multi_pinch_min_mm {
            MultiKind::PinchIn
        } else if ratio >= th.multi_pinch_out && delta >= th.multi_pinch_min_mm {
            MultiKind::PinchOut
        } else if !(lo..=hi).contains(&ratio) {
            return None;
        } else if ay > th.multi_swipe_mm && ay >= th.multi_axis_ratio * ax {
            // Y grows downward on the pad.
            if dy < 0.0 {
                MultiKind::SwipeUp
            } else {
                MultiKind::SwipeDown
            }
        } else if ax > th.multi_swipe_mm && ax >= th.multi_axis_ratio * ay {
            if dx < 0.0 {
                MultiKind::SwipeLeft
            } else {
                MultiKind::SwipeRight
            }
        } else {
            return None;
        };
        let action = cfg.bound(kind)?.clone();
        Some(MultiHit {
            proposal: Proposal {
                who: Owner::Multi,
                at_us: ctx.now,
            },
            fingers: n,
            kind,
            action,
            spread,
            at: c,
        })
    }

    /// While a window zoom runs: stream the spread ratio of the fingers, and end the zoom as soon
    /// as the finger count changes (a lifted finger ends it before the lift of the rest).
    pub fn drive(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        self.drive_scrub(ctx, out);
        let Some(run) = self.zoom else {
            return;
        };
        if ctx.raw != run.fingers || ctx.stable != run.fingers {
            self.zoom = None;
            out.push(Action::WindowZoomEnd);
            return;
        }
        if !ctx.from_frame || ctx.rebase {
            return;
        }
        let Some(c) = ctx.centroid else {
            return;
        };
        let ratio = spread_of(ctx.cur, c) / run.base_spread;
        if ratio.is_finite() && ratio > 0.0 {
            out.push(Action::WindowZoom { ratio });
        }
    }

    /// While an app switch scrub runs: one `AppSwitchStep` per step of travel along the swipe's
    /// axis, and the scrub ends (Alt up) as soon as the finger count changes.
    fn drive_scrub(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let Some(run) = self.scrub.as_mut() else {
            return;
        };
        if ctx.raw != run.fingers || ctx.stable != run.fingers {
            self.scrub = None;
            out.push(Action::AppSwitchEnd { cancel: false });
            return;
        }
        if !ctx.from_frame {
            return;
        }
        let Some(c) = ctx.centroid else {
            return;
        };
        if ctx.rebase {
            run.rebase(c);
            return;
        }
        let n = run.advance(c, ctx.th.app_switch_step_mm);
        for _ in 0..n.abs().min(MAX_STEPS_PER_FRAME) {
            out.push(Action::AppSwitchStep { forward: n > 0 });
        }
    }

    /// A touch of `peak` fingers ended as a tap: the tap binding of that count, if any and if
    /// nothing else fired or owns the session.
    pub fn tap(&self, g: &GestureCfg, peak: usize, free: bool) -> Option<MultiHit> {
        if self.fired || !free {
            return None;
        }
        let action = g.multi_for(peak)?.bound(MultiKind::Tap)?.clone();
        Some(MultiHit {
            proposal: Proposal {
                who: Owner::Multi,
                at_us: 0,
            },
            fingers: peak,
            kind: MultiKind::Tap,
            action,
            spread: 0.0,
            at: (0.0, 0.0),
        })
    }

    /// The gesture won (or, for a tap, is allowed): report it and trigger its action.
    pub fn commit(&mut self, hit: MultiHit, out: &mut Vec<Action>) {
        self.fired = true;
        out.push(Action::Debug(GestureEvent::Multi {
            fingers: hit.fingers,
            kind: hit.kind,
        }));
        let pinch = matches!(hit.kind, MultiKind::PinchIn | MultiKind::PinchOut);
        if pinch && hit.action == config::Action::WindowZoom {
            // The window zoom is not one-shot: it follows the fingers until they lift. Without a
            // usable spread it cannot start, and then nothing happens (it is no trigger).
            if hit.spread >= 1.0 {
                self.zoom = Some(ZoomRun {
                    fingers: hit.fingers,
                    base_spread: hit.spread,
                });
                out.push(Action::WindowZoomBegin);
            }
        } else if hit.action == config::Action::AppSwitchScrub
            && let Some((vertical, sign)) = match hit.kind {
                MultiKind::SwipeUp => Some((true, -1.0)),
                MultiKind::SwipeDown => Some((true, 1.0)),
                MultiKind::SwipeLeft => Some((false, -1.0)),
                MultiKind::SwipeRight => Some((false, 1.0)),
                _ => None,
            }
        {
            // Like the zoom: not one-shot. It follows the fingers until they lift. Anywhere but a
            // swipe it is a plain Alt+Tab (the trigger below).
            self.scrub = Some(ScrubRun {
                fingers: hit.fingers,
                vertical,
                sign,
                origin: if vertical { hit.at.1 } else { hit.at.0 },
                done: 0.0,
                last: 0.0,
                dir: 0.0,
                prev: 0.0,
            });
            out.push(Action::AppSwitchBegin);
        } else {
            out.push(Action::Trigger(hit.action));
        }
    }

    /// All fingers are up: a zoom in flight ends, and a scrub lets go of Alt (the window the
    /// selection is on opens).
    pub fn finish(&mut self, out: &mut Vec<Action>) {
        if self.zoom.is_some() {
            out.push(Action::WindowZoomEnd);
        }
        if self.scrub.is_some() {
            out.push(Action::AppSwitchEnd { cancel: false });
        }
        *self = Self::default();
    }

    /// The touch was cancelled (stale, interrupted): a scrub backs out with Esc so nothing
    /// switches, then lets go of Alt. Everything else ends as in `finish`.
    pub fn cancel(&mut self, out: &mut Vec<Action>) {
        if self.scrub.take().is_some() {
            out.push(Action::AppSwitchEnd { cancel: true });
        }
        self.finish(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> ScrubRun {
        ScrubRun {
            fingers: 3,
            vertical: false,
            sign: 1.0,
            origin: 50.0,
            done: 0.0,
            last: 0.0,
            dir: 0.0,
            prev: 0.0,
        }
    }

    #[test]
    fn a_swapped_finger_does_not_count_the_centroid_jump_as_travel() {
        let mut r = run();
        assert_eq!(r.advance((56.0, 0.0), 10.0), 0);
        // One finger swapped for another: the centroid jumps 20 mm in the same frame.
        r.rebase((76.0, 0.0));
        assert_eq!(r.advance((76.0, 0.0), 10.0), 0);
        // Travel after the swap counts from where the fingers were: 6 + 5 = 11 mm, one step.
        assert_eq!(r.advance((81.0, 0.0), 10.0), 1);
    }

    #[test]
    fn a_swap_on_a_leftward_scrub_keeps_the_position_too() {
        let mut r = ScrubRun {
            sign: -1.0,
            ..run()
        };
        assert_eq!(r.advance((42.0, 0.0), 10.0), 0);
        r.rebase((20.0, 0.0));
        assert_eq!(r.advance((20.0, 0.0), 10.0), 0);
        assert_eq!(r.advance((17.0, 0.0), 10.0), 1);
    }
}
