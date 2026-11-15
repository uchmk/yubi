//! Corner swipes: one finger lands in a corner zone of the pad and swipes out of it. Each corner
//! has three slots, picked by the direction of the swipe (measured from the corner, toward the
//! middle of the pad):
//!
//!  * horizontal: along the top or bottom edge, away from the corner (angle -20..30 degrees),
//!  * diagonal: toward the middle of the pad (30..60 degrees),
//!  * vertical: along the left or right edge, away from the corner (60..110 degrees).
//!
//! The finger must start inside the zone (`Corners::size_mm`, a square), and travel at least
//! `COMMIT_MM` within `WINDOW_US` of touching down. A slow or short move, a start outside the
//! zones, a second finger and a swipe that points out of the pad are all ordinary cursor moves.
//! Once it commits, the cursor is frozen until the finger lifts and the slot's action runs:
//! a one-shot action fires once; the window resize follows the finger live until it lifts.
//!
//! Arbitration with the edge slider: the edge slider starts from the same few millimetres, so a
//! corner zone that has any slot set belongs to the corner: an edge slide never starts there
//! (`owns_zone`). A corner whose three slots are all off leaves its zone to the edge slider. As a
//! second guard, while a corner swipe can still claim the touch the edge slider holds back
//! (`would_claim`), and if both commit in the same frame the corner wins (`Priority::Corner`).

use crate::config::{Action as Cfg, CornerDir};
use crate::engine::arbitration::{Owner, Proposal};
use crate::engine::types::{Action, Ctx, GestureEvent, Point, Suppress, dist};

/// Travel from the landing point that commits the swipe.
pub const COMMIT_MM: f32 = 6.0;
/// ...which must happen within this long of touching down.
pub const WINDOW_US: u64 = 400_000;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Fresh,
    /// Landed in the zone of `corner` at `start`, at time `t0`.
    Candidate {
        corner: usize,
        start: Point,
        t0: u64,
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

/// True when `start` is inside the zone of a corner that has at least one slot set: the edge
/// slider does not start there.
pub fn owns_zone(ctx: &Ctx, start: Point) -> bool {
    let (Some(cfg), Some(dims)) = (ctx.g.corner.as_ref(), ctx.dims) else {
        return false;
    };
    zone_of(start, dims, cfg.size_mm).is_some_and(|corner| {
        CornerDir::ALL
            .into_iter()
            .any(|dir| cfg.slot(corner, dir).is_some())
    })
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
        .is_some_and(|dir| cfg.slot(corner, dir).is_some())
}

impl CornerState {
    pub fn is_committed(&self) -> bool {
        matches!(self.phase, Phase::Committed { .. })
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

    /// Watch a fresh or candidate touch; propose once the swipe is clear.
    pub fn step(&mut self, ctx: &Ctx, free: bool) -> Option<Proposal> {
        let cfg = ctx.g.corner.as_ref()?;
        match self.phase {
            Phase::Fresh => {
                let (Some(dims), Some(&start)) = (ctx.dims, ctx.cur.first()) else {
                    self.phase = Phase::Dead;
                    return None;
                };
                self.phase = if ctx.raw != 1 || ctx.rebase {
                    Phase::Dead
                } else {
                    match zone_of(start, dims, cfg.size_mm) {
                        Some(corner) => Phase::Candidate {
                            corner,
                            start,
                            t0: ctx.now,
                        },
                        None => Phase::Dead,
                    }
                };
                None
            }
            Phase::Candidate { corner, start, t0 } => {
                if ctx.raw != 1 || ctx.rebase {
                    self.phase = Phase::Dead;
                    return None;
                }
                if !ctx.from_frame {
                    return None;
                }
                let p = *ctx.cur.first()?;
                let in_time = ctx.now.saturating_sub(t0) <= WINDOW_US;
                if dist(p, start) < COMMIT_MM {
                    if !in_time {
                        // Too slow: an ordinary cursor move.
                        self.phase = Phase::Dead;
                    }
                    return None;
                }
                let bound = in_time
                    && classify(corner, p.0 - start.0, p.1 - start.1)
                        .is_some_and(|dir| cfg.slot(corner, dir).is_some());
                if bound && free {
                    return Some(Proposal {
                        who: Owner::Corner,
                        at_us: ctx.now,
                    });
                }
                self.phase = Phase::Dead;
                None
            }
            Phase::Committed { .. } | Phase::Dead => None,
        }
    }

    /// The arbiter gave this swipe the session: freeze the cursor and run the slot's action.
    pub fn commit(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let Phase::Candidate { corner, start, .. } = self.phase else {
            return;
        };
        let (Some(cfg), Some(&p)) = (ctx.g.corner.as_ref(), ctx.cur.first()) else {
            self.phase = Phase::Dead;
            return;
        };
        let Some(dir) = classify(corner, p.0 - start.0, p.1 - start.1) else {
            self.phase = Phase::Dead;
            return;
        };
        let Some(action) = cfg.slot(corner, dir) else {
            self.phase = Phase::Dead;
            return;
        };
        let resize = *action == Cfg::ResizeFromOpposite;
        self.phase = Phase::Committed {
            corner,
            last: start,
            resize,
        };
        out.push(Action::Debug(GestureEvent::Corner { corner, dir }));
        out.push(Action::Suppress(Suppress::Cursor));
        if resize {
            out.push(Action::CornerResizeBegin { corner });
            self.drive(ctx, out);
        } else {
            out.push(Action::Trigger(action.clone()));
        }
    }

    /// The touch ended or was cancelled: end a live resize and release the cursor.
    pub fn release(&mut self, out: &mut Vec<Action>) {
        if let Phase::Committed { resize, .. } = self.phase {
            if resize {
                out.push(Action::CornerResizeEnd);
            }
            out.push(Action::Suppress(Suppress::Off));
        }
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
}
