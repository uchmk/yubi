//! Drag: N fingers move the cursor with the left button held.

use super::super::arbitration::{Owner, Proposal};
use super::super::types::{Action, Ctx, GestureCfg, Point, dist};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DragPhase {
    #[default]
    Idle,
    Active,
    /// Fingers are below N since this time; the drag ends when `lift_timeout` runs out.
    Lifted(u64),
    /// `keep_drag_on_release`: the drag stays until the next tap.
    Held,
}

#[derive(Debug, Default)]
pub struct DragState {
    pub phase: DragPhase,
    /// Where the centroid was when N fingers were first all down (for the start threshold).
    origin: Option<Point>,
    /// Centroid at the previous move; `None` re-bases on the next frame.
    last: Option<Point>,
}

impl DragState {
    /// The drag after release ends when the lift timeout runs out.
    pub fn expire(&mut self, now: u64, g: &GestureCfg, out: &mut Vec<Action>) {
        if let DragPhase::Lifted(since) = self.phase
            && now >= since + g.lift_us
        {
            out.push(Action::DragEnd);
            *self = Self::default();
        }
    }

    pub fn lift_deadline(&self, g: &GestureCfg) -> Option<u64> {
        match self.phase {
            DragPhase::Lifted(since) => Some(since + g.lift_us),
            _ => None,
        }
    }

    /// Run one step. `free` says nobody else owns the session. Returns a proposal when the drag
    /// wants to begin (or resume); the coordinator calls [`commit`](Self::commit) if it wins.
    pub fn step(&mut self, ctx: &Ctx, free: bool) -> Option<Proposal> {
        let g = ctx.g;
        let n = g.drag_n;
        let full = ctx.raw == n && ctx.stable == n;
        let c = ctx.centroid;
        if ctx.rebase {
            self.last = None;
            if self.phase == DragPhase::Idle {
                self.origin = None;
            }
        }
        let propose = Some(Proposal {
            who: Owner::Drag,
            at_us: ctx.now,
        });
        match self.phase {
            DragPhase::Idle => {
                // A tap-then-swipe window move wins over a 3-finger drag, but only for the tap
                // window (300 ms from the second touch); after that the touch is a plain drag.
                let in_tap_window = ctx.win_candidate
                    && ctx.now.saturating_sub(ctx.session_start_us) <= ctx.th.window_tap_gap_us;
                let blocked = !free || (in_tap_window && n == 3);
                match c {
                    Some(c) if full && !blocked => {
                        let origin = *self.origin.get_or_insert(c);
                        if dist(origin, c) > ctx.th.drag_start_mm {
                            return propose;
                        }
                    }
                    _ if !full => self.origin = None,
                    _ => {}
                }
            }
            DragPhase::Active => {}
            DragPhase::Lifted(_) | DragPhase::Held => {
                if c.is_some() && full && free {
                    return propose;
                }
            }
        }
        None
    }

    /// The drag won the arbitration.
    pub fn commit(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let Some(c) = ctx.centroid else {
            return;
        };
        if self.phase == DragPhase::Idle {
            out.push(Action::DragBegin);
        }
        self.phase = DragPhase::Active;
        self.last = Some(c);
    }

    /// Emit the movement of an active drag (the drag owns the session).
    pub fn drive(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        if self.phase != DragPhase::Active {
            return;
        }
        let n = ctx.g.drag_n;
        let full = ctx.raw == n && ctx.stable == n;
        match ctx.centroid {
            Some(c) if full && !ctx.rebase => {
                if let Some(l) = self.last {
                    let (dx, dy) = (c.0 - l.0, c.1 - l.1);
                    if dx != 0.0 || dy != 0.0 {
                        out.push(Action::DragMove {
                            dx_mm: dx,
                            dy_mm: dy,
                        });
                    }
                }
                self.last = Some(c);
            }
            _ => {
                // The count changed: re-base so the cursor does not jump.
                self.last = None;
                if ctx.stable < n {
                    self.phase = if ctx.g.keep_drag {
                        DragPhase::Held
                    } else {
                        DragPhase::Lifted(ctx.now)
                    };
                }
            }
        }
    }

    /// All fingers are up.
    pub fn finish(
        &mut self,
        g: &GestureCfg,
        is_tap: bool,
        drag_was_held: bool,
        t: u64,
        out: &mut Vec<Action>,
    ) {
        match self.phase {
            DragPhase::Active | DragPhase::Held if is_tap && drag_was_held => {
                out.push(Action::DragEnd);
                *self = Self::default();
            }
            DragPhase::Active => {
                self.phase = if g.keep_drag {
                    DragPhase::Held
                } else {
                    DragPhase::Lifted(t)
                };
            }
            _ => {}
        }
        self.origin = None;
        self.last = None;
    }

    /// Close a drag that is still open (stale cancel). Emits the end so the button is released.
    pub fn cancel(&mut self, out: &mut Vec<Action>) {
        if self.phase != DragPhase::Idle {
            out.push(Action::DragEnd);
        }
        *self = Self::default();
    }
}
