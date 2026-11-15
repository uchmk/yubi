//! Edge slider: one finger lands in a band along the left, right or top edge of the pad, rests
//! briefly and slides along the edge. Each `step_mm` of travel is one step of the target that
//! the side is bound to (volume, brightness, scroll, zoom, seek, undo/redo, tab, custom keys).
//!
//! It must not steal ordinary cursor moves, so it commits only when
//!  * the finger started inside the band,
//!  * it rested (moved at most `DWELL_SLOP_MM`) for `DWELL_US`, and
//!  * it then travelled at least `COMMIT_MM` along the edge with the along:cross ratio at least
//!    `AXIS_RATIO`.
//!
//! Anything else (a second finger, a move that is not along the edge, another gesture owning the
//! session) makes it give up quietly, and the touch stays an ordinary cursor move. Once
//! committed the cursor is frozen (`Action::Suppress`) until the finger lifts, and a second
//! finger cancels the slider.
//!
//! Forward is up along the left and right edges and right along the top edge.

use crate::engine::arbitration::{Owner, Proposal};
use crate::engine::types::{Action, Ctx, EdgeSide, Point, Suppress, dist};

/// The finger must rest this long before a slide can start.
const DWELL_US: u64 = 150_000;
/// ...moving no more than this while it rests.
const DWELL_SLOP_MM: f32 = 2.0;
/// Travel along the edge that commits the slider.
const COMMIT_MM: f32 = 3.0;
/// Along must be at least this many times the cross travel.
const AXIS_RATIO: f32 = 2.0;
/// Most steps one frame may emit (a glitchy jump must not spin the volume to the end).
const MAX_STEPS_PER_FRAME: i32 = 8;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Phase {
    /// Nothing looked at yet in this session.
    #[default]
    Fresh,
    /// The finger landed in the band of `side` at `start` at time `t0`.
    Candidate {
        side: EdgeSide,
        start: Point,
        t0: u64,
    },
    /// The slider is live. `done_mm` is the travel along the edge already turned into steps.
    Committed {
        side: EdgeSide,
        start: Point,
        done_mm: f32,
    },
    /// Not an edge slide in this session.
    Dead,
}

#[derive(Debug, Default)]
pub struct EdgeState {
    phase: Phase,
}

/// Which band `p` is in on a `dims` (width, height) pad, among the sides that have a target.
/// A corner picks the nearest edge.
pub fn band_of(
    p: Point,
    dims: (f32, f32),
    strip: f32,
    has: impl Fn(EdgeSide) -> bool,
) -> Option<EdgeSide> {
    let (w, _h) = dims;
    [
        (EdgeSide::Left, p.0),
        (EdgeSide::Right, w - p.0),
        (EdgeSide::Top, p.1),
    ]
    .into_iter()
    .filter(|&(side, d)| d <= strip && has(side))
    .min_by(|a, b| a.1.total_cmp(&b.1))
    .map(|(side, _)| side)
}

/// Travel of `p` from `start` along the edge (forward positive) and across it.
pub fn along_cross(side: EdgeSide, p: Point, start: Point) -> (f32, f32) {
    let (dx, dy) = (p.0 - start.0, p.1 - start.1);
    if side.horizontal() {
        (dx, dy)
    } else {
        (-dy, dx)
    }
}

impl EdgeState {
    pub fn is_committed(&self) -> bool {
        matches!(self.phase, Phase::Committed { .. })
    }

    /// Emit the steps for the travel since the last call (committed sliders only).
    pub fn drive(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let Phase::Committed {
            side,
            start,
            done_mm,
        } = self.phase
        else {
            return;
        };
        let Some(cfg) = ctx.g.edge.as_ref() else {
            return;
        };
        if ctx.raw != 1 || ctx.rebase {
            self.phase = Phase::Dead;
            out.push(Action::Suppress(Suppress::Off));
            return;
        }
        let target = cfg.target(side);
        let step_mm = target.step_mm(ctx.th.edge_detent_mm).max(0.5);
        let (along, _) = along_cross(side, ctx.cur[0], start);
        let n = ((along - done_mm) / step_mm).trunc() as i32;
        if n == 0 {
            return;
        }
        self.phase = Phase::Committed {
            side,
            start,
            done_mm: done_mm + n as f32 * step_mm,
        };
        let outs = target.step(n > 0);
        if outs.is_empty() {
            return;
        }
        for _ in 0..n.abs().min(MAX_STEPS_PER_FRAME) {
            out.push(Action::EdgeStep {
                side,
                outs: outs.clone(),
            });
        }
    }

    /// Watch a fresh or candidate touch; propose once the slide is clearly along the edge.
    pub fn step(&mut self, ctx: &Ctx, free: bool) -> Option<Proposal> {
        let cfg = ctx.g.edge.as_ref()?;
        match self.phase {
            Phase::Fresh => {
                let start = match (ctx.dims, ctx.cur.first()) {
                    (Some(_), Some(&p)) if ctx.raw == 1 && !ctx.rebase => p,
                    _ => {
                        self.phase = Phase::Dead;
                        return None;
                    }
                };
                let dims = ctx.dims?;
                self.phase = match band_of(start, dims, ctx.th.edge_strip_mm, |s| {
                    *cfg.target(s) != crate::config::EdgeTarget::None
                }) {
                    Some(side) => Phase::Candidate {
                        side,
                        start,
                        t0: ctx.now,
                    },
                    None => Phase::Dead,
                };
                None
            }
            Phase::Candidate { side, start, t0 } => {
                if ctx.raw != 1 || ctx.rebase {
                    self.phase = Phase::Dead;
                    return None;
                }
                if !ctx.from_frame {
                    return None;
                }
                let p = *ctx.cur.first()?;
                if ctx.now.saturating_sub(t0) < DWELL_US {
                    if dist(p, start) > DWELL_SLOP_MM {
                        self.phase = Phase::Dead;
                    }
                    return None;
                }
                let (along, cross) = along_cross(side, p, start);
                let dominant = along.abs() >= COMMIT_MM && along.abs() >= AXIS_RATIO * cross.abs();
                if dominant {
                    if free {
                        return Some(Proposal {
                            who: Owner::Edge,
                            at_us: ctx.now,
                        });
                    }
                    self.phase = Phase::Dead;
                } else if dist(p, start) >= COMMIT_MM && along.abs() < AXIS_RATIO * cross.abs() {
                    // Moving, but not along the edge: an ordinary cursor move. A slide that is
                    // along the edge but still short of COMMIT_MM (a real finger drifts a little
                    // inward, so the distance reaches it first) keeps waiting.
                    self.phase = Phase::Dead;
                }
                None
            }
            Phase::Committed { .. } | Phase::Dead => None,
        }
    }

    /// The arbiter gave this slider the session: freeze the cursor and emit the first steps.
    pub fn commit(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let Phase::Candidate { side, start, .. } = self.phase else {
            return;
        };
        self.phase = Phase::Committed {
            side,
            start,
            done_mm: 0.0,
        };
        out.push(Action::Suppress(Suppress::Cursor));
        self.drive(ctx, out);
    }

    /// The touch ended or was cancelled: release the cursor if it was frozen.
    pub fn release(&mut self, out: &mut Vec<Action>) {
        if self.is_committed() {
            out.push(Action::Suppress(Suppress::Off));
        }
        self.phase = Phase::Fresh;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, CustomEdge, EdgeOut, EdgeTarget, Profile};
    use crate::engine::recognizer::Recognizer;
    use crate::engine::{Frame, replay};
    use crate::input::Contact;

    const DIMS: (f32, f32) = (160.0, 115.0);

    fn c(x: f32, y: f32) -> Contact {
        Contact {
            id: 1,
            x: 0.0,
            y: 0.0,
            x_mm: x,
            y_mm: y,
            tip: true,
            confidence: true,
            pressure: None,
        }
    }

    fn c2(id: u32, x: f32, y: f32) -> Contact {
        Contact { id, ..c(x, y) }
    }

    fn frame(t_ms: u64, contacts: Vec<Contact>) -> Frame {
        Frame {
            t_us: t_ms * 1000,
            button: false,
            contacts,
        }
    }

    fn cfg_with(left: EdgeTarget, right: EdgeTarget, top: EdgeTarget) -> Config {
        let mut cfg = Config::default();
        cfg.edge.enabled = true;
        cfg.edge.left = left;
        cfg.edge.right = right;
        cfg.edge.top = top;
        cfg
    }

    fn recognizer(cfg: &Config) -> (Recognizer, Config) {
        let mut r = Recognizer::default();
        r.set_pad_dims(Some(DIMS));
        (r, cfg.clone())
    }

    /// Frames of one finger: rest at `from` for 200 ms, then move to `to` over 100 ms.
    fn slide(from: Point, to: Point) -> Vec<Frame> {
        let mut v = Vec::new();
        for ms in (0..=200).step_by(10) {
            v.push(frame(ms, vec![c(from.0, from.1)]));
        }
        for i in 1..=10 {
            let f = i as f32 / 10.0;
            v.push(frame(
                200 + i * 10,
                vec![c(
                    from.0 + (to.0 - from.0) * f,
                    from.1 + (to.1 - from.1) * f,
                )],
            ));
        }
        v
    }

    fn run(frames: &[Frame], cfg: &Config) -> Vec<Action> {
        let (mut r, cfg) = recognizer(cfg);
        let mut out = Vec::new();
        for f in frames {
            out.extend(r.step(f, &cfg));
        }
        out
    }

    fn steps(out: &[Action]) -> Vec<(EdgeSide, Vec<EdgeOut>)> {
        out.iter()
            .filter_map(|a| match a {
                Action::EdgeStep { side, outs } => Some((*side, outs.clone())),
                _ => None,
            })
            .collect()
    }

    fn vol(n: i8) -> Vec<EdgeOut> {
        vec![EdgeOut::Act(crate::config::Action::VolumeStep(n))]
    }

    fn suppress(out: &[Action]) -> Vec<Suppress> {
        out.iter()
            .filter_map(|a| match a {
                Action::Suppress(s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn band_entry_picks_the_nearest_enabled_side() {
        let all = |_| true;
        assert_eq!(band_of((3.0, 50.0), DIMS, 6.0, all), Some(EdgeSide::Left));
        assert_eq!(
            band_of((157.0, 50.0), DIMS, 6.0, all),
            Some(EdgeSide::Right)
        );
        assert_eq!(band_of((80.0, 4.0), DIMS, 6.0, all), Some(EdgeSide::Top));
        assert_eq!(band_of((80.0, 50.0), DIMS, 6.0, all), None);
        // Corner: nearest edge wins.
        assert_eq!(band_of((2.0, 5.0), DIMS, 6.0, all), Some(EdgeSide::Left));
        assert_eq!(band_of((5.0, 2.0), DIMS, 6.0, all), Some(EdgeSide::Top));
        // A side without a target is not a band.
        assert_eq!(
            band_of((3.0, 50.0), DIMS, 6.0, |s| s != EdgeSide::Left),
            None
        );
    }

    /// Recorded on hardware: the finger drifts ~0.9 mm inward while the first 3 mm along the
    /// edge build up, so the straight-line distance reaches COMMIT_MM before the along part.
    #[test]
    fn slide_with_inward_drift_still_starts() {
        let cfg = cfg_with(EdgeTarget::None, EdgeTarget::Volume, EdgeTarget::None);
        let mut v = Vec::new();
        for ms in (0..=200).step_by(10) {
            v.push(frame(ms, vec![c(159.8, 25.4)]));
        }
        for i in 1..=20u64 {
            let drift = (i as f32 * 0.3).min(0.9);
            v.push(frame(
                200 + i * 10,
                vec![c(159.8 - drift, 25.4 + i as f32 * 0.98)],
            ));
        }
        let out = run(&v, &cfg);
        assert_eq!(suppress(&out).first(), Some(&Suppress::Cursor));
        assert!(!steps(&out).is_empty());
    }

    #[test]
    fn slide_up_the_left_edge_raises_the_volume_in_detents() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        // 20 mm up at a 2 mm detent.
        let out = run(&slide((3.0, 80.0), (3.0, 60.0)), &cfg);
        assert_eq!(suppress(&out).first(), Some(&Suppress::Cursor));
        let s = steps(&out);
        assert_eq!(s.len(), 10, "{s:?}");
        assert!(
            s.iter()
                .all(|(side, o)| *side == EdgeSide::Left && *o == vol(2))
        );
    }

    #[test]
    fn lost_freeze_ends_the_slide_and_stops_the_steps() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        let (mut r, cfg) = recognizer(&cfg);
        let frames = slide((3.0, 80.0), (3.0, 60.0));
        let mut out = Vec::new();
        for f in &frames {
            out.extend(r.step(f, &cfg));
        }
        assert!(!steps(&out).is_empty());
        // The hook's watchdog cancelled the freeze: the engine aborts the slide.
        let abort = r.abort_freeze();
        assert_eq!(suppress(&abort), vec![Suppress::Off]);
        // The finger keeps sliding for 10 s more: nothing is emitted any more.
        let mut after = Vec::new();
        for i in 0..1000u64 {
            let y = 60.0 - (i as f32 * 0.02);
            after.extend(r.step(&frame(400 + i * 10, vec![c(3.0, y)]), &cfg));
        }
        assert!(steps(&after).is_empty(), "{after:?}");
        assert!(r.abort_freeze().is_empty());
    }

    #[test]
    fn steps_accumulate_both_ways_without_losing_travel() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        let mut frames = slide((3.0, 80.0), (3.0, 66.0)); // 14 mm up: 7 steps
        let mut t = 300;
        for y in [68.0, 70.0, 72.0] {
            // back down 2 mm per frame: one step back each
            t += 10;
            frames.push(frame(t, vec![c(3.0, y)]));
        }
        let s = steps(&run(&frames, &cfg));
        let up = s.iter().filter(|(_, o)| *o == vol(2)).count();
        let down = s.iter().filter(|(_, o)| *o == vol(-2)).count();
        assert_eq!((up, down), (7, 3), "{s:?}");
    }

    #[test]
    fn slow_creep_below_the_detent_makes_no_step_until_it_adds_up() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        let mut frames = slide((3.0, 80.0), (3.0, 76.5)); // commits at 3 mm, 1 step
        for (i, y) in [76.0, 75.5, 75.0, 74.5].into_iter().enumerate() {
            frames.push(frame(310 + i as u64 * 10, vec![c(3.0, y)]));
        }
        let s = steps(&run(&frames, &cfg));
        // 5.5 mm of travel in total at 2 mm per step is 2 steps.
        assert_eq!(s.len(), 2, "{s:?}");
    }

    #[test]
    fn top_edge_slides_sideways_and_right_is_forward() {
        let cfg = cfg_with(
            EdgeTarget::None,
            EdgeTarget::None,
            EdgeTarget::Seek(crate::config::SeekPreset::General),
        );
        let out = run(&slide((60.0, 3.0), (72.0, 3.0)), &cfg);
        let s = steps(&out);
        assert_eq!(s.len(), 6, "{s:?}");
        let keys = |k: &str| vec![EdgeOut::Act(crate::config::Action::Keys(k.into()))];
        assert!(
            s.iter()
                .all(|(side, o)| *side == EdgeSide::Top && *o == keys("Right"))
        );
        let out = run(&slide((60.0, 3.0), (48.0, 3.0)), &cfg);
        assert!(steps(&out).iter().all(|(_, o)| *o == keys("Left")));
    }

    #[test]
    fn center_start_never_triggers() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::Volume, EdgeTarget::Volume);
        let out = run(&slide((80.0, 80.0), (80.0, 40.0)), &cfg);
        assert!(steps(&out).is_empty());
        assert!(suppress(&out).is_empty());
    }

    #[test]
    fn a_move_across_the_edge_is_a_cursor_move() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        // Leaves the left band sideways: along/cross is 0:1.
        let out = run(&slide((3.0, 80.0), (40.0, 78.0)), &cfg);
        assert!(steps(&out).is_empty());
        assert!(suppress(&out).is_empty());
        // A diagonal move (along no more than twice the cross) does not commit either.
        let out = run(&slide((3.0, 80.0), (13.0, 70.0)), &cfg);
        assert!(steps(&out).is_empty());
    }

    #[test]
    fn moving_before_the_dwell_is_not_a_slide() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        let frames: Vec<Frame> = (0..=20)
            .map(|i| frame(i * 10, vec![c(3.0, 90.0 - i as f32 * 1.5)]))
            .collect();
        let out = run(&frames, &cfg);
        assert!(steps(&out).is_empty());
        assert!(suppress(&out).is_empty());
    }

    #[test]
    fn second_finger_cancels_the_slider_and_releases_the_cursor() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        let mut frames = slide((3.0, 80.0), (3.0, 66.0));
        frames.push(frame(320, vec![c2(1, 3.0, 66.0), c2(2, 80.0, 60.0)]));
        frames.push(frame(330, vec![c2(1, 3.0, 50.0), c2(2, 80.0, 60.0)]));
        let out = run(&frames, &cfg);
        assert_eq!(suppress(&out), vec![Suppress::Cursor, Suppress::Off]);
        let n = steps(&out).len();
        assert_eq!(n, 7, "no steps after the second finger");
    }

    #[test]
    fn lift_releases_the_cursor() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        let mut frames = slide((3.0, 80.0), (3.0, 66.0));
        frames.push(frame(320, vec![]));
        let out = run(&frames, &cfg);
        assert_eq!(suppress(&out), vec![Suppress::Cursor, Suppress::Off]);
    }

    #[test]
    fn disabled_edges_do_nothing() {
        let mut cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::Volume, EdgeTarget::Volume);
        cfg.edge.enabled = false;
        let out = run(&slide((3.0, 80.0), (3.0, 60.0)), &cfg);
        assert!(steps(&out).is_empty());
    }

    #[test]
    fn unknown_pad_size_never_triggers() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        let out = replay::run(&slide((3.0, 80.0), (3.0, 60.0)), &cfg);
        assert!(
            out.iter()
                .all(|(_, a)| !matches!(a, Action::EdgeStep { .. }))
        );
    }

    #[test]
    fn per_app_override_picks_the_target() {
        let mut cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        let mut p = Profile::default();
        p.edge_overrides
            .insert("edge.left".into(), EdgeTarget::Brightness);
        p.exe_name = "app.exe".into();
        p.enabled = true;
        cfg.profiles.push(p);
        let frames = slide((3.0, 80.0), (3.0, 70.0));
        let (mut r, cfg) = recognizer(&cfg);
        r.set_foreground(Some("app.exe"), false);
        let mut out = Vec::new();
        for f in &frames {
            out.extend(r.step(f, &cfg));
        }
        let s = steps(&out);
        assert!(!s.is_empty());
        assert!(
            s.iter()
                .all(|(_, o)| *o == vec![EdgeOut::Act(crate::config::Action::BrightnessStep(5))])
        );
    }

    #[test]
    fn custom_target_uses_its_own_step_distance() {
        let cfg = cfg_with(
            EdgeTarget::Custom(CustomEdge {
                forward: "Ctrl+Plus".into(),
                back: "Ctrl+Minus".into(),
                step_mm: 5.0,
            }),
            EdgeTarget::None,
            EdgeTarget::None,
        );
        let out = run(&slide((3.0, 80.0), (3.0, 60.0)), &cfg);
        assert_eq!(steps(&out).len(), 4);
    }

    #[test]
    fn blocked_app_never_triggers() {
        let cfg = cfg_with(EdgeTarget::Volume, EdgeTarget::None, EdgeTarget::None);
        let (mut r, cfg) = recognizer(&cfg);
        r.set_foreground(Some("game.exe"), true);
        let mut out = Vec::new();
        for f in &slide((3.0, 80.0), (3.0, 60.0)) {
            out.extend(r.step(f, &cfg));
        }
        assert!(steps(&out).is_empty());
    }
}
