//! Window move and resize rows of the drag page: N fingers (touch and move, or a tap first) move
//! the window under the cursor, a flick on release snaps it; or resize it from the 3x3 cell the
//! cursor is in. Also remembers the latest tap that arms a tap-then-move row.

use super::super::arbitration::{Owner, Proposal};
use super::super::types::{Action, Ctx, GestureCfg, Point, Thresholds, dist};
use super::flick::FlickTracker;
use crate::config::{DragAction, DragStart};

#[derive(Debug, Default)]
pub struct WindowState {
    /// End of the latest tap and its finger count (arms a tap-then-move row of that count).
    pub tap: Option<(u64, usize)>,
    pub active: bool,
    /// Finger count and action of the row in flight (or about to start).
    n: usize,
    resize: bool,
    origin: Option<Point>,
    last: Option<Point>,
    /// Recent (time, centroid) samples with exactly three fingers, for the flick speed.
    flick: FlickTracker,
}

impl WindowState {
    /// Run one step. Returns a proposal when the swipe is long enough to start a move.
    pub fn step(&mut self, ctx: &Ctx, free: bool, out: &mut Vec<Action>) -> Option<Proposal> {
        let row = if self.active {
            Some((self.n, self.resize))
        } else {
            (3..=5).find_map(|n| {
                let r = ctx.g.startable_row(n, ctx.armed)?;
                (ctx.raw == n && ctx.stable == n && r.action != DragAction::Drag)
                    .then_some((n, r.action == DragAction::WindowResize))
            })
        };
        let full = row.is_some_and(|(n, _)| ctx.raw == n && ctx.stable == n);
        let Some(c) = ctx.centroid.filter(|_| full && !ctx.rebase) else {
            // Count or IDs changed: re-base on the next full frame.
            self.last = None;
            if !self.active {
                self.origin = None;
            }
            return None;
        };
        let mut proposal = None;
        if self.active {
            if let Some(l) = self.last {
                let (dx, dy) = (c.0 - l.0, c.1 - l.1);
                if dx != 0.0 || dy != 0.0 {
                    out.push(if self.resize {
                        Action::WindowResizeMove {
                            dx_mm: dx,
                            dy_mm: dy,
                        }
                    } else {
                        Action::WindowMove {
                            dx_mm: dx,
                            dy_mm: dy,
                        }
                    });
                }
            }
            self.last = Some(c);
            self.sample(ctx, c);
        } else if free {
            let origin = *self.origin.get_or_insert(c);
            if dist(origin, c) > ctx.th.window_start_mm {
                (self.n, self.resize) = row.unwrap_or((3, false));
                proposal = Some(Proposal {
                    who: Owner::Window,
                    at_us: ctx.now,
                });
            }
        }
        proposal
    }

    /// The window move won the arbitration.
    pub fn commit(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let Some(c) = ctx.centroid else {
            return;
        };
        out.push(if self.resize {
            Action::WindowResizeBegin
        } else {
            Action::WindowMoveBegin
        });
        self.active = true;
        self.last = Some(c);
        self.sample(ctx, c);
    }

    fn sample(&mut self, ctx: &Ctx, c: Point) {
        self.flick.sample(ctx, c);
    }

    /// All fingers are up: close what was in flight and arm what follows a tap.
    pub fn finish(
        &mut self,
        g: &GestureCfg,
        th: &Thresholds,
        is_tap: bool,
        peak: usize,
        t: u64,
        out: &mut Vec<Action>,
    ) {
        if self.active {
            if self.resize {
                out.push(Action::WindowResizeEnd);
            } else {
                let snap = if g.flick { self.flick.dir(th, t) } else { None };
                out.push(Action::WindowMoveEnd { snap });
            }
            *self = Self::default();
        }
        self.origin = None;
        self.last = None;
        self.flick.clear();
        self.tap = None;
        if is_tap
            && g.drag_row(peak)
                .is_some_and(|r| r.start == DragStart::TapThenMove)
        {
            self.tap = Some((t, peak));
        }
    }

    /// Close a window move that is still open (stale cancel): no snap.
    pub fn cancel(&mut self, out: &mut Vec<Action>) {
        if self.active {
            out.push(if self.resize {
                Action::WindowResizeEnd
            } else {
                Action::WindowMoveEnd { snap: None }
            });
        }
        *self = Self::default();
    }
}
