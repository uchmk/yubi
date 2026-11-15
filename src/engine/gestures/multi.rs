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
}

/// A window zoom in progress: the pinch that started it keeps streaming its spread ratio.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ZoomRun {
    fingers: usize,
    /// The spread when the zoom began: the ratio is measured from here, so the window does not
    /// jump by the part of the pinch that was needed to recognise it.
    base_spread: f32,
}

#[derive(Debug, Default)]
pub struct MultiState {
    /// Centroid and spread when the counted fingers were first all down.
    start: Option<(Point, f32)>,
    fired: bool,
    zoom: Option<ZoomRun>,
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
        })
    }

    /// While a window zoom runs: stream the spread ratio of the fingers, and end the zoom as soon
    /// as the finger count changes (a lifted finger ends it before the lift of the rest).
    pub fn drive(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
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
        } else {
            out.push(Action::Trigger(hit.action));
        }
    }

    /// All fingers are up (or the touch was cancelled): a zoom in flight ends.
    pub fn finish(&mut self, out: &mut Vec<Action>) {
        if self.zoom.is_some() {
            out.push(Action::WindowZoomEnd);
        }
        *self = Self::default();
    }
}
