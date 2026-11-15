//! Window move and resize rows of the drag page: N fingers (touch and move, or a tap first) move
//! the window under the cursor, a flick on release snaps it; or resize it from the 3x3 cell the
//! cursor is in. Also remembers the latest tap that arms a tap-then-move row.

use std::collections::VecDeque;

use super::super::arbitration::{Owner, Proposal};
use super::super::types::{Action, Ctx, GestureCfg, Point, SnapDir, Thresholds, dist};
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
    samples: VecDeque<(u64, Point)>,
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
        if !ctx.from_frame {
            return;
        }
        let now = ctx.now;
        self.samples.push_back((now, c));
        while self.samples.front().is_some_and(|(t, _)| t + 200_000 < now) {
            self.samples.pop_front();
        }
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
                let snap = if g.flick { self.flick_dir(th, t) } else { None };
                out.push(Action::WindowMoveEnd { snap });
            }
            *self = Self::default();
        }
        self.origin = None;
        self.last = None;
        self.samples.clear();
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

    /// Direction of the flick that ended a window move, from the speed over the last
    /// `flick_window_us` before the lift.
    fn flick_dir(&self, th: &Thresholds, lift_us: u64) -> Option<SnapDir> {
        let &(t_last, p_last) = self.samples.back()?;
        if lift_us.saturating_sub(t_last) > th.flick_stale_us {
            return None;
        }
        let &(t0, p0) = self
            .samples
            .iter()
            .find(|(t, _)| t + th.flick_window_us >= t_last)?;
        let span = t_last - t0;
        if span < th.flick_min_span_us {
            return None;
        }
        let secs = span as f32 / 1e6;
        let (vx, vy) = ((p_last.0 - p0.0) / secs, (p_last.1 - p0.1) / secs);
        let (ax, ay) = (vx.abs(), vy.abs());
        if ax.hypot(ay) < th.flick_speed_mm_s {
            return None;
        }
        let ratio = th.flick_axis_ratio;
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
