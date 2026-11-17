//! Gesture tests over synthetic frame sequences (11 ms steps, like the real device).

use super::Frame;
use super::recognizer::{Action, SnapDir, Thresholds};
use super::replay;
use super::resolve::app_blocked;
use crate::config::{self, Config, Profile};
use crate::input::Contact;

const STEP: u64 = 11_000;

type Pts = Vec<(f32, f32)>;

/// `n` fingers in a row, 10 mm apart, centred on (cx, cy).
fn row(n: usize, cx: f32, cy: f32) -> Pts {
    (0..n)
        .map(|i| (cx + (i as f32 - (n as f32 - 1.0) / 2.0) * 10.0, cy))
        .collect()
}

/// Five fingers on a ring of radius 20 mm * `scale`.
fn ring(cx: f32, cy: f32, scale: f32) -> Pts {
    (0..5)
        .map(|k| {
            let a = (90.0 + 72.0 * k as f32).to_radians();
            (cx + 20.0 * scale * a.cos(), cy + 20.0 * scale * a.sin())
        })
        .collect()
}

fn shifted(p: &[(f32, f32)], dx: f32, dy: f32) -> Pts {
    p.iter().map(|&(x, y)| (x + dx, y + dy)).collect()
}

fn frame(t_us: u64, pts: &[(f32, f32)]) -> Frame {
    Frame {
        t_us,
        button: false,
        contacts: pts
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| Contact {
                id: i as u32 + 1,
                x: 0.0,
                y: 0.0,
                x_mm: x,
                y_mm: y,
                tip: true,
                confidence: true,
                pressure: None,
            })
            .collect(),
    }
}

/// Builds a frame list; `t` is the time of the next frame.
#[derive(Default)]
struct Seq {
    t: u64,
    frames: Vec<Frame>,
}

impl Seq {
    fn still(&mut self, p: &[(f32, f32)], ms: u64) {
        for _ in 0..(ms * 1000 / STEP).max(1) {
            self.frames.push(frame(self.t, p));
            self.t += STEP;
        }
    }

    /// Move the whole group by (dx, dy) in even steps over `ms`; returns the end positions.
    fn glide(&mut self, from: &[(f32, f32)], dx: f32, dy: f32, ms: u64) -> Pts {
        let n = (ms * 1000 / STEP).max(1);
        for i in 1..=n {
            let f = i as f32 / n as f32;
            self.frames
                .push(frame(self.t, &shifted(from, dx * f, dy * f)));
            self.t += STEP;
        }
        shifted(from, dx, dy)
    }

    /// All fingers off at the next frame, then `gap_ms` of nothing.
    fn lift(&mut self, gap_ms: u64) {
        self.frames.push(frame(self.t, &[]));
        self.t += STEP + gap_ms * 1000;
    }

    /// The ring pinched / spread to `scale` of its original size over `ms`.
    fn scale_ring(&mut self, scale: f32, ms: u64) {
        let n = (ms * 1000 / STEP).max(1);
        for i in 1..=n {
            let k = 1.0 + (scale - 1.0) * i as f32 / n as f32;
            self.frames.push(frame(self.t, &ring(80.0, 57.0, k)));
            self.t += STEP;
        }
    }

    fn run(&self, cfg: &Config) -> Vec<(u64, Action)> {
        replay::run(&self.frames, cfg)
    }
}

fn drag_cfg(fingers: u8) -> Config {
    let mut cfg = Config::default();
    cfg.drag.only(fingers as usize);
    cfg.drag.lift_timeout_ms = 300;
    cfg.drag.keep_drag_on_release = false;
    cfg
}

fn window_cfg() -> Config {
    let mut cfg = Config::default();
    cfg.drag.three = crate::config::DragRow {
        enabled: true,
        start: crate::config::DragStart::TapThenMove,
        action: crate::config::DragAction::WindowMove,
    };
    cfg.window.flick_snap = true;
    cfg
}

fn five_cfg() -> Config {
    let mut cfg = Config::default();
    cfg.five.enabled = true;
    cfg.five.pinch_in = config::Action::TaskView;
    cfg.five.pinch_out = config::Action::ShowDesktop;
    cfg.five.swipe_up = config::Action::AppSwitch;
    cfg.five.swipe_down = config::Action::Find;
    cfg.five.tap = config::Action::MiddleClick;
    cfg.five.swipe_left = config::Action::DesktopNext;
    cfg.five.swipe_right = config::Action::DesktopPrev;
    cfg
}

fn real(out: &[(u64, Action)]) -> Vec<&Action> {
    out.iter()
        .map(|(_, a)| a)
        .filter(|a| !matches!(a, Action::Debug(_)))
        .collect()
}

fn count(out: &[(u64, Action)], pred: impl Fn(&Action) -> bool) -> usize {
    out.iter().filter(|(_, a)| pred(a)).count()
}

fn drag_moves(out: &[(u64, Action)]) -> Vec<(f32, f32)> {
    out.iter()
        .filter_map(|(_, a)| match a {
            Action::DragMove { dx_mm, dy_mm } => Some((*dx_mm, *dy_mm)),
            _ => None,
        })
        .collect()
}

fn time_of(out: &[(u64, Action)], pred: impl Fn(&Action) -> bool) -> Option<u64> {
    out.iter().find(|(_, a)| pred(a)).map(|(t, _)| *t)
}

// ---- drag ----

#[test]
fn drag_begin_move_end() {
    let mut s = Seq::default();
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    s.glide(&p, 10.0, 0.0, 200);
    let lifted_at = s.t;
    s.lift(0);
    let out = s.run(&drag_cfg(3));
    let r = real(&out);
    assert!(matches!(r.first(), Some(Action::DragBegin)), "{r:?}");
    assert!(matches!(r.last(), Some(Action::DragEnd)), "{r:?}");
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 1);
    let sum: f32 = drag_moves(&out).iter().map(|m| m.0).sum();
    assert!((7.0..=10.01).contains(&sum), "moved {sum} mm");
    // The end comes on the lift timeout (300 ms), not at the lift.
    let end = time_of(&out, |a| matches!(a, Action::DragEnd)).unwrap();
    assert!(
        end >= lifted_at + 250_000 && end <= lifted_at + 400_000,
        "{end}"
    );
}

#[test]
fn drag_needs_the_configured_count() {
    let mut s = Seq::default();
    let p = row(2, 60.0, 50.0);
    s.still(&p, 100);
    s.glide(&p, 20.0, 0.0, 200);
    s.lift(0);
    let out = s.run(&drag_cfg(3));
    assert!(real(&out).is_empty(), "{:?}", real(&out));
}

#[test]
fn drag_resumes_when_fingers_return_in_time() {
    let mut s = Seq::default();
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    let p = s.glide(&p, 8.0, 0.0, 150);
    s.lift(100);
    s.still(&p, 100);
    s.glide(&p, 8.0, 0.0, 150);
    s.lift(0);
    let out = s.run(&drag_cfg(3));
    assert_eq!(
        count(&out, |a| matches!(a, Action::DragBegin)),
        1,
        "{:?}",
        real(&out)
    );
    assert_eq!(count(&out, |a| matches!(a, Action::DragEnd)), 1);
    let sum: f32 = drag_moves(&out).iter().map(|m| m.0).sum();
    assert!(sum > 12.0, "moved {sum} mm");
}

#[test]
fn drag_ends_when_fingers_stay_away() {
    let mut s = Seq::default();
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    let p = s.glide(&p, 8.0, 0.0, 150);
    s.lift(600);
    s.still(&p, 100);
    s.glide(&p, 8.0, 0.0, 150);
    s.lift(0);
    let out = s.run(&drag_cfg(3));
    // The second touch starts a second drag.
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 2);
    assert_eq!(count(&out, |a| matches!(a, Action::DragEnd)), 2);
}

#[test]
fn keep_drag_ends_on_the_next_tap() {
    let mut cfg = drag_cfg(3);
    cfg.drag.keep_drag_on_release = true;
    let mut s = Seq::default();
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    s.glide(&p, 8.0, 0.0, 150);
    s.lift(2000);
    let tap_at = s.t;
    s.still(&row(1, 30.0, 30.0), 60);
    s.lift(0);
    let out = s.run(&cfg);
    let end = time_of(&out, |a| matches!(a, Action::DragEnd)).expect("drag ends");
    assert!(end >= tap_at, "the drag outlived the lift, ended at {end}");
    assert_eq!(count(&out, |a| matches!(a, Action::DragEnd)), 1);
}

#[test]
fn drag_does_not_jump_when_the_count_changes() {
    let mut s = Seq::default();
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    s.glide(&p, 4.0, 0.0, 100);
    let p = shifted(&p, 4.0, 0.0);
    // A fourth finger lands far away: the centroid shifts by 10 mm.
    let mut four = p.clone();
    four.push((p[2].0 + 40.0, 50.0));
    s.still(&four, 150);
    s.still(&p, 150);
    s.glide(&p, 4.0, 0.0, 100);
    s.lift(0);
    let out = s.run(&drag_cfg(3));
    for (dx, dy) in drag_moves(&out) {
        assert!(dx.abs() < 2.5 && dy.abs() < 2.5, "jump of ({dx}, {dy}) mm");
    }
}

#[test]
fn drag_does_not_jump_when_three_fingers_become_two_and_back() {
    let mut s = Seq::default();
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    let p = s.glide(&p, 6.0, 0.0, 100);
    // One finger leaves: the centroid shifts by 5 mm.
    let two = p[..2].to_vec();
    s.still(&two, 100);
    // It comes back in time; the drag resumes from the new centroid.
    s.still(&p, 100);
    s.glide(&p, 4.0, 0.0, 100);
    s.lift(0);
    let out = s.run(&drag_cfg(3));
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 1);
    assert_eq!(count(&out, |a| matches!(a, Action::DragEnd)), 1);
    let sum: f32 = drag_moves(&out).iter().map(|m| m.0).sum();
    assert!(sum < 11.0, "moved {sum} mm, the cursor jumped");
    // The 4 mm after the finger came back must move the cursor too (the drag really resumed).
    assert!(sum > 6.0, "moved only {sum} mm, the drag did not resume");
    for (dx, dy) in drag_moves(&out) {
        assert!(dx.abs() < 2.5 && dy.abs() < 2.5, "jump of ({dx}, {dy}) mm");
    }
}

#[test]
fn a_late_touch_after_the_tap_window_starts_nothing() {
    let mut s = Seq::default();
    let p = tap_then_touch(&mut s, 100);
    // Rest well past 300 ms, then a small move: it is not a window move any more, and the
    // three-finger row is a window row, so it is not a drag either.
    s.still(&p, 400);
    s.glide(&p, 3.0, 0.0, 100);
    s.lift(0);
    let cfg = window_cfg();
    let out = s.run(&cfg);
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 0);
    assert_eq!(count(&out, |a| matches!(a, Action::WindowMoveBegin)), 0);
}

// ---- window move ----

fn tap_then_touch(s: &mut Seq, gap_ms: u64) -> Pts {
    let p = row(3, 60.0, 50.0);
    s.still(&p, 70);
    s.lift(gap_ms);
    s.still(&p, 100);
    p
}

#[test]
fn window_move_after_a_three_finger_tap() {
    let mut s = Seq::default();
    let p = tap_then_touch(&mut s, 100);
    s.glide(&p, 20.0, 0.0, 400);
    s.lift(0);
    let cfg = window_cfg();
    let out = s.run(&cfg);
    let r = real(&out);
    assert!(matches!(r.first(), Some(Action::WindowMoveBegin)), "{r:?}");
    assert!(
        matches!(r.last(), Some(Action::WindowMoveEnd { snap: None })),
        "{r:?}"
    );
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 0);
    let sum: f32 = out
        .iter()
        .filter_map(|(_, a)| match a {
            Action::WindowMove { dx_mm, .. } => Some(*dx_mm),
            _ => None,
        })
        .sum();
    assert!((8.0..=20.01).contains(&sum), "moved {sum} mm");
}

#[test]
fn drag_wins_when_window_move_is_off() {
    let mut s = Seq::default();
    let p = tap_then_touch(&mut s, 100);
    s.glide(&p, 20.0, 0.0, 400);
    s.lift(0);
    let out = s.run(&drag_cfg(3));
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 1);
    assert_eq!(count(&out, |a| matches!(a, Action::WindowMoveBegin)), 0);
}

#[test]
fn window_move_needs_the_tap_first() {
    let mut s = Seq::default();
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    s.glide(&p, 30.0, 0.0, 400);
    s.lift(0);
    let out = s.run(&window_cfg());
    assert!(real(&out).is_empty(), "{:?}", real(&out));
}

#[test]
fn window_move_too_late_is_not_a_move() {
    let mut s = Seq::default();
    let p = tap_then_touch(&mut s, 600);
    s.glide(&p, 30.0, 0.0, 400);
    s.lift(0);
    let out = s.run(&window_cfg());
    assert!(real(&out).is_empty(), "{:?}", real(&out));
}

fn flick(dx: f32, dy: f32, ms: u64) -> Option<SnapDir> {
    let mut s = Seq::default();
    let p = tap_then_touch(&mut s, 100);
    s.glide(&p, dx, dy, ms);
    s.lift(0);
    let out = s.run(&window_cfg());
    out.iter()
        .find_map(|(_, a)| match a {
            Action::WindowMoveEnd { snap } => Some(*snap),
            _ => None,
        })
        .expect("window move ended")
}

#[test]
fn flick_snaps_per_direction() {
    assert_eq!(flick(-30.0, 0.0, 66), Some(SnapDir::Left));
    assert_eq!(flick(30.0, 0.0, 66), Some(SnapDir::Right));
    assert_eq!(flick(0.0, -30.0, 66), Some(SnapDir::Up));
    assert_eq!(flick(0.0, 30.0, 66), Some(SnapDir::Down));
}

#[test]
fn slow_swipe_does_not_snap() {
    assert_eq!(flick(30.0, 0.0, 800), None);
}

#[test]
fn diagonal_flick_does_not_snap() {
    assert_eq!(flick(25.0, 25.0, 66), None);
}

#[test]
fn flick_snap_off_gives_no_snap() {
    let mut s = Seq::default();
    let p = tap_then_touch(&mut s, 100);
    s.glide(&p, 30.0, 0.0, 66);
    s.lift(0);
    let mut cfg = window_cfg();
    cfg.window.flick_snap = false;
    let out = s.run(&cfg);
    assert!(
        out.iter()
            .any(|(_, a)| matches!(a, Action::WindowMoveEnd { snap: None }))
    );
}

// ---- five fingers ----

fn triggers(out: &[(u64, Action)]) -> Vec<config::Action> {
    out.iter()
        .filter_map(|(_, a)| match a {
            Action::Trigger(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn five_run(hold_ms: u64, then: impl Fn(&mut Seq, &[(f32, f32)])) -> Vec<config::Action> {
    let mut s = Seq::default();
    let p = ring(80.0, 57.0, 1.0);
    s.still(&p, hold_ms);
    then(&mut s, &p);
    s.lift(0);
    triggers(&s.run(&five_cfg()))
}

#[test]
fn five_pinch_in_and_out() {
    assert_eq!(
        five_run(100, |s, _| s.scale_ring(0.6, 200)),
        vec![config::Action::TaskView]
    );
    assert_eq!(
        five_run(100, |s, _| s.scale_ring(1.5, 200)),
        vec![config::Action::ShowDesktop]
    );
}

#[test]
fn five_swipe_up_and_down() {
    assert_eq!(
        five_run(100, |s, p| {
            s.glide(p, 0.0, -25.0, 250);
        }),
        vec![config::Action::AppSwitch]
    );
    assert_eq!(
        five_run(100, |s, p| {
            s.glide(p, 0.0, 25.0, 250);
        }),
        vec![config::Action::Find]
    );
}

#[test]
fn five_swipe_left_and_right() {
    assert_eq!(
        five_run(100, |s, p| {
            s.glide(p, -25.0, 0.0, 250);
        }),
        vec![config::Action::DesktopNext]
    );
    assert_eq!(
        five_run(100, |s, p| {
            s.glide(p, 25.0, 0.0, 250);
        }),
        vec![config::Action::DesktopPrev]
    );
}

#[test]
fn five_tap_fires_when_bound() {
    assert_eq!(five_run(60, |_, _| {}), vec![config::Action::MiddleClick]);
    // Unbound: a tap does nothing.
    let mut s = Seq::default();
    s.still(&ring(80.0, 57.0, 1.0), 60);
    s.lift(0);
    let mut cfg = five_cfg();
    cfg.five.tap = config::Action::None;
    assert!(triggers(&s.run(&cfg)).is_empty());
}

#[test]
fn five_still_hold_fires_nothing() {
    assert!(five_run(1500, |_, _| {}).is_empty());
}

#[test]
fn five_fires_once_per_session() {
    let t = five_run(100, |s, _| {
        s.scale_ring(0.6, 200);
        s.scale_ring(1.8, 300);
    });
    assert_eq!(t, vec![config::Action::TaskView]);
}

#[test]
fn five_none_action_emits_nothing() {
    let mut cfg = five_cfg();
    cfg.five.pinch_in = config::Action::None;
    let mut s = Seq::default();
    s.still(&ring(80.0, 57.0, 1.0), 100);
    s.scale_ring(0.6, 200);
    s.lift(0);
    assert!(triggers(&s.run(&cfg)).is_empty());
}

#[test]
fn five_off_by_default() {
    let mut s = Seq::default();
    s.still(&ring(80.0, 57.0, 1.0), 100);
    s.scale_ring(0.6, 200);
    s.lift(0);
    assert!(real(&s.run(&Config::default())).is_empty());
}

// ---- four fingers ----

/// `n` fingers on a ring of radius 20 mm * `scale`.
fn ring_n(n: usize, scale: f32) -> Pts {
    (0..n)
        .map(|k| {
            let a = (90.0 + 360.0 / n as f32 * k as f32).to_radians();
            (80.0 + 20.0 * scale * a.cos(), 57.0 + 20.0 * scale * a.sin())
        })
        .collect()
}

fn four_run(cfg: &Config, then: impl Fn(&mut Seq, &[(f32, f32)])) -> Vec<config::Action> {
    let mut s = Seq::default();
    let p = ring_n(4, 1.0);
    s.still(&p, 100);
    then(&mut s, &p);
    s.lift(0);
    triggers(&s.run(cfg))
}

fn four_swipe(dx: f32, dy: f32) -> Vec<config::Action> {
    four_run(&Config::default(), |s, p| {
        s.glide(p, dx, dy, 250);
    })
}

#[test]
fn four_swipes_use_the_default_bindings() {
    assert_eq!(four_swipe(-25.0, 0.0), vec![config::Action::DesktopNext]);
    assert_eq!(four_swipe(25.0, 0.0), vec![config::Action::DesktopPrev]);
    assert_eq!(four_swipe(0.0, -25.0), vec![config::Action::TaskView]);
    assert_eq!(four_swipe(0.0, 25.0), vec![config::Action::ShowDesktop]);
}

#[test]
fn four_default_swipes_dispatch_to_ctrl_win_arrows() {
    use super::dispatch::{Plan, plan_for};
    let left = four_swipe(-25.0, 0.0);
    assert!(matches!(plan_for(&left[0]), Plan::Keys(k) if k.len() == 3));
    assert_eq!(left, vec![config::Action::DesktopNext]);
}

#[test]
fn four_pinch_in_and_out() {
    let mut cfg = Config::default();
    cfg.four.pinch_in = config::Action::AppSwitch;
    cfg.four.pinch_out = config::Action::Find;
    let pinch = |scale: f32| {
        let mut s = Seq::default();
        s.still(&ring_n(4, 1.0), 100);
        for i in 1..=18u64 {
            let k = 1.0 + (scale - 1.0) * i as f32 / 18.0;
            s.frames.push(frame(s.t, &ring_n(4, k)));
            s.t += STEP;
        }
        s.lift(0);
        triggers(&s.run(&cfg))
    };
    assert_eq!(pinch(0.6), vec![config::Action::AppSwitch]);
    assert_eq!(pinch(1.5), vec![config::Action::Find]);
}

#[test]
fn four_pinch_zoom_streams_until_lift() {
    let mut cfg = Config::default();
    cfg.four.pinch_out = config::Action::WindowZoom;
    cfg.four.pinch_in = config::Action::Find;
    let run = |scale: f32| {
        let mut s = Seq::default();
        s.still(&ring_n(4, 1.0), 100);
        for i in 1..=30u64 {
            let k = 1.0 + (scale - 1.0) * i as f32 / 30.0;
            s.frames.push(frame(s.t, &ring_n(4, k)));
            s.t += STEP;
        }
        s.lift(0);
        s.run(&cfg)
    };
    let out = run(1.8);
    let kinds: Vec<&Action> = real(&out);
    let begin = kinds
        .iter()
        .position(|a| matches!(a, Action::WindowZoomBegin));
    let end = kinds
        .iter()
        .position(|a| matches!(a, Action::WindowZoomEnd));
    let (Some(b), Some(e)) = (begin, end) else {
        panic!("zoom did not begin and end: {kinds:?}");
    };
    assert!(b < e);
    assert_eq!(count(&out, |a| matches!(a, Action::WindowZoomBegin)), 1);
    assert_eq!(count(&out, |a| matches!(a, Action::WindowZoomEnd)), 1);
    assert!(triggers(&out).is_empty());
    let ratios: Vec<f32> = out
        .iter()
        .filter_map(|(_, a)| match a {
            Action::WindowZoom { ratio } => Some(*ratio),
            _ => None,
        })
        .collect();
    assert!(ratios.len() > 3, "{ratios:?}");
    assert!(ratios.windows(2).all(|w| w[1] >= w[0] - 1e-3));
    assert!(*ratios.last().unwrap() > 1.2);
    // Other pinch actions stay one-shot and never stream.
    let out = run(0.6);
    assert_eq!(triggers(&out), vec![config::Action::Find]);
    assert_eq!(count(&out, |a| matches!(a, Action::WindowZoomBegin)), 0);
}

#[test]
fn four_tap_fires_when_bound() {
    let mut cfg = Config::default();
    cfg.four.tap = config::Action::Find;
    let mut s = Seq::default();
    s.still(&ring_n(4, 1.0), 60);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![config::Action::Find]);
    // Unbound by default: a tap does nothing.
    assert!(triggers(&s.run(&Config::default())).is_empty());
}

#[test]
fn four_still_hold_fires_nothing() {
    let mut cfg = Config::default();
    cfg.four.tap = config::Action::Find;
    assert!(four_run(&cfg, |s, p| s.still(p, 1500)).is_empty());
}

#[test]
fn four_swipes_yield_to_a_four_finger_drag() {
    let mut cfg = drag_cfg(4);
    cfg.four.pinch_in = config::Action::None;
    let mut s = Seq::default();
    let p = ring_n(4, 1.0);
    s.still(&p, 100);
    s.glide(&p, -25.0, 0.0, 250);
    s.lift(0);
    let out = s.run(&cfg);
    assert!(triggers(&out).is_empty());
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 1);
}

#[test]
fn four_off_gives_nothing() {
    let mut cfg = Config::default();
    cfg.four.enabled = false;
    assert!(
        four_run(&cfg, |s, p| {
            s.glide(p, -25.0, 0.0, 250);
        })
        .is_empty()
    );
}

// ---- three fingers ----

fn three_cfg() -> Config {
    let mut cfg = Config::default();
    cfg.drag.disable_all();
    cfg.three.enabled = true;
    cfg.three.tap = config::Action::Find;
    cfg.three.swipe_left = config::Action::DesktopNext;
    cfg.three.swipe_right = config::Action::DesktopPrev;
    cfg.three.swipe_up = config::Action::TaskView;
    cfg.three.swipe_down = config::Action::ShowDesktop;
    cfg.three.pinch_in = config::Action::AppSwitch;
    cfg.three.pinch_out = config::Action::WindowZoom;
    cfg
}

fn three_swipe(cfg: &Config, dx: f32, dy: f32) -> Vec<config::Action> {
    let mut s = Seq::default();
    let p = ring_n(3, 1.0);
    s.still(&p, 100);
    s.glide(&p, dx, dy, 250);
    s.lift(0);
    triggers(&s.run(cfg))
}

#[test]
fn three_swipes_fire_the_bound_actions_when_enabled() {
    let cfg = three_cfg();
    assert_eq!(
        three_swipe(&cfg, -25.0, 0.0),
        vec![config::Action::DesktopNext]
    );
    assert_eq!(
        three_swipe(&cfg, 25.0, 0.0),
        vec![config::Action::DesktopPrev]
    );
    assert_eq!(
        three_swipe(&cfg, 0.0, -25.0),
        vec![config::Action::TaskView]
    );
    assert_eq!(
        three_swipe(&cfg, 0.0, 25.0),
        vec![config::Action::ShowDesktop]
    );
}

#[test]
fn three_fires_nothing_when_off_or_by_default() {
    let mut cfg = three_cfg();
    cfg.three.enabled = false;
    assert!(three_swipe(&cfg, -25.0, 0.0).is_empty());
    let mut s = Seq::default();
    s.still(&ring_n(3, 1.0), 60);
    s.lift(0);
    assert!(triggers(&s.run(&cfg)).is_empty());
    // The default config has the three-finger gestures off and unbound.
    assert!(three_swipe(&Config::default(), -25.0, 0.0).is_empty());
}

#[test]
fn three_tap_and_pinch() {
    let cfg = three_cfg();
    let mut s = Seq::default();
    s.still(&ring_n(3, 1.0), 60);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![config::Action::Find]);
    let mut s = Seq::default();
    s.still(&ring_n(3, 1.0), 100);
    for i in 1..=18u64 {
        let k = 1.0 + (0.6 - 1.0) * i as f32 / 18.0;
        s.frames.push(frame(s.t, &ring_n(3, k)));
        s.t += STEP;
    }
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![config::Action::AppSwitch]);
}

#[test]
fn three_swipes_yield_to_a_three_finger_drag() {
    let mut cfg = three_cfg();
    cfg.drag.only(3);
    cfg.drag.lift_timeout_ms = 300;
    cfg.drag.keep_drag_on_release = false;
    let mut s = Seq::default();
    let p = ring_n(3, 1.0);
    s.still(&p, 100);
    s.glide(&p, -25.0, 0.0, 250);
    s.lift(0);
    let out = s.run(&cfg);
    assert!(triggers(&out).is_empty());
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 1);
}

#[test]
fn three_swipe_follows_a_per_app_override() {
    let mut cfg = three_cfg();
    let mut p = Profile {
        exe_name: "app.exe".into(),
        enabled: true,
        ..Profile::default()
    };
    p.overrides
        .insert("swipe.3.left".into(), config::Action::Find);
    cfg.profiles.push(p);
    let g = super::types::GestureCfg::from_config(&cfg, Some("app.exe"));
    assert_eq!(g.multi_for(3).unwrap().swipe_left, config::Action::Find);
    let g = super::types::GestureCfg::from_config(&cfg, Some("other.exe"));
    assert_eq!(
        g.multi_for(3).unwrap().swipe_left,
        config::Action::DesktopNext
    );
}

// ---- per-app filter and thresholds ----

#[test]
fn per_app_filter_is_case_insensitive_and_only_for_disabled() {
    let profiles = vec![
        Profile {
            exe_name: "Game.EXE".into(),
            enabled: false,
            ..Profile::default()
        },
        Profile {
            exe_name: "notepad.exe".into(),
            enabled: true,
            ..Profile::default()
        },
    ];
    assert!(app_blocked(&profiles, Some("game.exe")));
    assert!(!app_blocked(&profiles, Some("notepad.exe")));
    assert!(!app_blocked(&profiles, Some("other.exe")));
    assert!(!app_blocked(&profiles, None));
    assert!(!app_blocked(&[], Some("game.exe")));
}

#[test]
fn default_thresholds_are_as_documented() {
    let th = Thresholds::default();
    assert_eq!(th.drag_px_per_mm, 12.0);
    assert_eq!(th.window_px_per_mm, 12.0);
    assert_eq!(th.window_tap_gap_us, 300_000);
}

#[test]
fn default_sensitivity_gives_the_default_thresholds() {
    assert_eq!(
        Thresholds::from_sensitivity(&config::Sensitivity::default()),
        Thresholds::default()
    );
    let s = config::Sensitivity {
        drag_gain: 20.0,
        swipe_mm: 25.0,
        tap_ms: 250,
        ..config::Sensitivity::default()
    };
    let th = Thresholds::from_sensitivity(&s);
    assert_eq!(th.drag_px_per_mm, 20.0);
    assert_eq!(th.multi_swipe_mm, 25.0);
    assert_eq!(th.tap_max_us, 250_000);
    assert_eq!(th.debounce_us, Thresholds::default().debounce_us);
}

// ---- recorded fixtures ----

#[test]
fn replay_three_finger_drag() {
    let frames = replay::parse(include_str!("fixtures/three_finger_drag.jsonl")).unwrap();
    let out = replay::run(&frames, &drag_cfg(3));
    let r = real(&out);
    assert!(matches!(r.first(), Some(Action::DragBegin)), "{r:?}");
    assert!(matches!(r.last(), Some(Action::DragEnd)), "{r:?}");
    let sum: f32 = drag_moves(&out).iter().map(|m| m.0).sum();
    assert!((3.0..=6.01).contains(&sum), "moved {sum} mm");
    // Off by default: the same recording does nothing without the drag switched on.
    let off = replay::run(&frames, &Config::default());
    assert!(real(&off).is_empty());
}

#[test]
fn replay_five_finger_pinch_in() {
    let frames = replay::parse(include_str!("fixtures/five_pinch_in.jsonl")).unwrap();
    let out = replay::run(&frames, &five_cfg());
    assert_eq!(triggers(&out), vec![config::Action::TaskView]);
}

// ---- light and deep press (raw pressure thresholds) ----

fn pframe(t_us: u64, pts: &[(f32, f32)], pressures: &[f32]) -> Frame {
    let mut f = frame(t_us, pts);
    for (c, &p) in f.contacts.iter_mut().zip(pressures) {
        c.pressure = Some(p);
    }
    f
}

impl Seq {
    /// One frame with per-contact pressures.
    fn push_p(&mut self, pts: &[(f32, f32)], pressures: &[f32]) {
        self.frames.push(pframe(self.t, pts, pressures));
        self.t += STEP;
    }

    /// `frames` frames at the same spot, every contact at pressure `p`.
    fn hold_p(&mut self, pts: &[(f32, f32)], p: f32, frames: usize) {
        for _ in 0..frames {
            self.push_p(pts, &vec![p; pts.len()]);
        }
    }

    /// `frames` frames alternating between pressures `a` and `b`.
    fn jitter_p(&mut self, pts: &[(f32, f32)], a: f32, b: f32, frames: usize) {
        for i in 0..frames {
            let p = if i % 2 == 0 { a } else { b };
            self.push_p(pts, &vec![p; pts.len()]);
        }
    }
}

/// Press on for `fingers` with the given bindings and the default thresholds.
fn press_cfg(fingers: usize, light: config::Action, deep: config::Action) -> Config {
    let mut cfg = Config::default();
    cfg.press.enabled = true;
    let f = &mut cfg.press.fingers[fingers - 1];
    f.enabled = true;
    f.light = light;
    f.deep = deep;
    cfg
}

const REST: f32 = 0.02;

#[test]
fn light_only_fires_once_at_the_raw_light_threshold() {
    use config::Action::Find;
    // 1 finger: light on at 0.08.
    let cfg = press_cfg(1, Find, config::Action::None);
    let p = row(1, 80.0, 57.0);
    let mut s = Seq::default();
    s.hold_p(&p, REST, 10);
    s.hold_p(&p, 0.07, 10); // below on: nothing
    assert!(triggers(&s.run(&cfg)).is_empty());
    s.hold_p(&p, 0.08, 10); // exactly on: fires
    s.hold_p(&p, 0.19, 20); // staying pressed does not repeat
    s.lift(0);
    let out = s.run(&cfg);
    assert_eq!(triggers(&out), vec![Find]);
    // Fired after the 22 ms persistence, not at the first sample.
    let fired = time_of(&out, |a| matches!(a, Action::Trigger(_))).unwrap();
    assert!(fired >= 20 * STEP + 22_000, "fired at {fired}");
}

#[test]
fn a_short_spike_is_filtered_out() {
    use config::Action::Find;
    let cfg = press_cfg(1, Find, config::Action::None);
    let p = row(1, 80.0, 57.0);
    let mut s = Seq::default();
    s.hold_p(&p, REST, 10);
    s.hold_p(&p, 0.18, 1); // a single noisy sample: the median drops it
    s.hold_p(&p, REST, 10);
    s.lift(0);
    assert!(triggers(&s.run(&cfg)).is_empty());
}

#[test]
fn the_range_ends_work() {
    use config::Action::{Find, TaskView};
    let mut cfg = press_cfg(1, Find, TaskView);
    cfg.press.light_threshold = 0.01;
    cfg.press.deep_threshold = 0.20;
    let p = row(1, 80.0, 57.0);
    let mut s = Seq::default();
    s.hold_p(&p, 0.0, 6);
    s.hold_p(&p, 0.01, 10);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![Find]);
    let mut s = Seq::default();
    s.hold_p(&p, 0.0, 6);
    s.hold_p(&p, 0.19, 10);
    s.hold_p(&p, 0.20, 10);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![TaskView]);
}

#[test]
fn provisional_light_fires_on_release_and_deep_discards_it() {
    use config::Action::{Find, TaskView};
    let cfg = press_cfg(1, Find, TaskView);
    let p = row(1, 80.0, 57.0);
    // Light only, then released while still touching: the light action fires on release.
    let mut s = Seq::default();
    s.hold_p(&p, REST, 8);
    s.hold_p(&p, 0.10, 10);
    s.hold_p(&p, REST, 10);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![Find]);
    // Light, then lifted straight off the pad: fires on the lift.
    let mut s = Seq::default();
    s.hold_p(&p, REST, 8);
    s.hold_p(&p, 0.10, 10);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![Find]);
    // Light then deep: only the deep action, the light one is dropped.
    let mut s = Seq::default();
    s.hold_p(&p, REST, 8);
    s.hold_p(&p, 0.10, 8);
    s.hold_p(&p, 0.16, 10);
    s.hold_p(&p, REST, 10);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![TaskView]);
}

#[test]
fn deep_only_binding_ignores_a_light_press() {
    use config::Action::{None as Off, TaskView};
    let cfg = press_cfg(1, Off, TaskView);
    let p = row(1, 80.0, 57.0);
    let mut s = Seq::default();
    s.hold_p(&p, REST, 8);
    s.hold_p(&p, 0.10, 12);
    s.hold_p(&p, REST, 10);
    s.lift(0);
    assert!(triggers(&s.run(&cfg)).is_empty());
    let mut s = Seq::default();
    s.hold_p(&p, REST, 8);
    s.hold_p(&p, 0.16, 12);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![TaskView]);
}

#[test]
fn no_repeat_until_the_pressure_stays_below_light_off() {
    use config::Action::Find;
    let cfg = press_cfg(1, Find, config::Action::None);
    let p = row(1, 80.0, 57.0);
    let mut s = Seq::default();
    s.hold_p(&p, REST, 8);
    s.hold_p(&p, 0.10, 10); // fires
    s.hold_p(&p, 0.075, 12); // eased off but above light-off (0.07)
    s.hold_p(&p, 0.10, 10); // pressed again: still the same press
    s.hold_p(&p, 0.03, 2); // a dip shorter than 50 ms
    s.hold_p(&p, 0.10, 10); // not re-armed yet
    assert_eq!(triggers(&s.run(&cfg)), vec![Find]);
    s.hold_p(&p, 0.03, 8); // 88 ms below light-off: re-armed
    s.hold_p(&p, 0.10, 10);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![Find, Find]);
}

#[test]
fn centroid_movement_before_the_commit_cancels() {
    use config::Action::Find;
    let cfg = press_cfg(1, Find, config::Action::None);
    let p = row(1, 80.0, 57.0);
    let mut s = Seq::default();
    s.hold_p(&p, REST, 8);
    s.hold_p(&p, 0.10, 2); // armed, not yet held 22 ms
    let moved = shifted(&p, 12.0, 0.0);
    s.hold_p(&moved, 0.10, 12);
    s.lift(0);
    assert!(triggers(&s.run(&cfg)).is_empty());
}

#[test]
fn a_count_change_cancels_a_latched_light_press() {
    use config::Action::{Find, TaskView};
    let cfg = press_cfg(1, Find, TaskView);
    let one = row(1, 80.0, 57.0);
    let two = row(2, 80.0, 57.0);
    let mut s = Seq::default();
    s.hold_p(&one, REST, 8);
    s.hold_p(&one, 0.10, 8); // provisional light latched
    s.hold_p(&two, 0.10, 8); // a second finger lands
    s.hold_p(&two, REST, 8);
    s.lift(0);
    assert!(triggers(&s.run(&cfg)).is_empty());
}

#[test]
fn the_signal_is_the_maximum_over_the_contacts() {
    use config::Action::Find;
    let cfg = press_cfg(2, Find, config::Action::None);
    let two = row(2, 80.0, 57.0);
    let mut s = Seq::default();
    for _ in 0..8 {
        s.push_p(&two, &[0.01, 0.01]);
    }
    for _ in 0..10 {
        s.push_p(&two, &[0.01, 0.09]); // one finger presses hard, the other rests
    }
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![Find]);
}

#[test]
fn two_and_three_finger_jitter_does_not_flicker() {
    use config::Action::{Find, TaskView};
    for n in [2usize, 3] {
        // Own pair 0.07 / 0.12: light-off 0.06, deep-off 0.10.
        let pts = row(n, 80.0, 57.0);
        // Light only, hovering on the threshold with plus or minus 0.01.
        let cfg = press_cfg(n, Find, config::Action::None);
        let mut s = Seq::default();
        s.hold_p(&pts, REST, 10);
        s.jitter_p(&pts, 0.07, 0.06, 40);
        s.hold_p(&pts, 0.065, 20);
        s.lift(0);
        assert_eq!(triggers(&s.run(&cfg)), vec![Find], "{n} fingers light");
        // Light and deep: jitter around the deep threshold is one deep press.
        let cfg = press_cfg(n, Find, TaskView);
        let mut s = Seq::default();
        s.hold_p(&pts, REST, 10);
        s.hold_p(&pts, 0.09, 8);
        s.jitter_p(&pts, 0.12, 0.11, 40);
        s.jitter_p(&pts, 0.12, 0.10, 20);
        s.hold_p(&pts, REST, 10);
        s.lift(0);
        assert_eq!(triggers(&s.run(&cfg)), vec![TaskView], "{n} fingers deep");
    }
}

#[test]
fn counts_use_their_own_pair_or_share_the_one_finger_pair() {
    use config::Action::Find;
    let two = row(2, 80.0, 57.0);
    let run = |cfg: &Config| {
        let mut s = Seq::default();
        s.hold_p(&two, REST, 8);
        s.hold_p(&two, 0.075, 12); // between the shared 0.08 and own 0.07
        s.lift(0);
        triggers(&s.run(cfg))
    };
    let mut cfg = press_cfg(2, Find, config::Action::None);
    assert_eq!(run(&cfg), vec![Find]); // own pair 0.07
    cfg.press.fingers[1].light_threshold = None;
    cfg.press.fingers[1].deep_threshold = None;
    assert!(run(&cfg).is_empty()); // shared one-finger pair 0.08
}

#[test]
fn a_disabled_count_or_switch_fires_nothing() {
    use config::Action::Find;
    let p = row(1, 80.0, 57.0);
    let mut s = Seq::default();
    s.hold_p(&p, REST, 8);
    s.hold_p(&p, 0.12, 12);
    s.lift(0);
    let mut cfg = press_cfg(1, Find, config::Action::None);
    assert_eq!(triggers(&s.run(&cfg)), vec![Find]);
    cfg.press.fingers[0].enabled = false;
    assert!(triggers(&s.run(&cfg)).is_empty());
    cfg.press.fingers[0].enabled = true;
    cfg.press.enabled = false;
    assert!(triggers(&s.run(&cfg)).is_empty());
}

#[test]
fn a_per_app_override_replaces_the_press_binding() {
    use super::types::GestureCfg;
    use config::Action::{Find, TaskView};
    let mut cfg = press_cfg(1, Find, config::Action::None);
    cfg.profiles.push(Profile {
        exe_name: "app.exe".into(),
        enabled: true,
        overrides: [("press.1.light".to_owned(), TaskView)].into(),
        ..Profile::default()
    });
    let mut g = GestureCfg::default();
    g.set_press(&cfg, Some("APP.exe"));
    assert_eq!(g.press_for(1).unwrap().light, TaskView);
    g.set_press(&cfg, Some("other.exe"));
    assert_eq!(g.press_for(1).unwrap().light, Find);
}

#[test]
fn a_four_finger_swipe_is_not_hijacked_by_a_press() {
    use config::Action::Find;
    let mut cfg = press_cfg(4, Find, config::Action::None);
    cfg.four.enabled = true;
    let p = ring_n(4, 1.0);
    let mut s = Seq::default();
    s.still(&p, 100);
    // Pressing harder part-way through the swipe.
    let n = 250 / 11;
    for i in 1..=n {
        let f = i as f32 / n as f32;
        let pts = shifted(&p, -25.0 * f, 0.0);
        let pr = if i > n / 3 { 0.12 } else { REST };
        s.push_p(&pts, &[pr; 4]);
    }
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![config::Action::DesktopNext]);
}

#[test]
fn a_four_finger_pinch_is_not_hijacked_by_a_press() {
    use config::Action::Find;
    let mut cfg = press_cfg(4, Find, config::Action::None);
    cfg.four.enabled = true;
    cfg.four.pinch_in = config::Action::AppSwitch;
    let mut s = Seq::default();
    s.still(&ring_n(4, 1.0), 100);
    for i in 1..=18u64 {
        let k = 1.0 - 0.4 * i as f32 / 18.0;
        let pr = if i > 6 { 0.12 } else { REST };
        s.push_p(&ring_n(4, k), &[pr; 4]);
    }
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![config::Action::AppSwitch]);
}

#[test]
fn a_light_four_finger_tap_is_still_a_tap() {
    use config::Action::Find;
    let mut cfg = press_cfg(4, config::Action::AppSwitch, Find);
    cfg.four.enabled = true;
    cfg.four.tap = config::Action::TaskView;
    let mut s = Seq::default();
    s.hold_p(&ring_n(4, 1.0), 0.04, 6); // resting touch: well below light-on 0.07
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![config::Action::TaskView]);
    // A firm press is not a tap.
    let mut s = Seq::default();
    s.hold_p(&ring_n(4, 1.0), 0.04, 4);
    s.hold_p(&ring_n(4, 1.0), 0.15, 8);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![Find]);
}

#[test]
fn a_light_only_press_owns_the_touch_so_a_later_swipe_does_nothing() {
    use config::Action::Find;
    let mut cfg = press_cfg(4, Find, config::Action::None);
    cfg.four.enabled = true;
    let p = ring_n(4, 1.0);
    let mut s = Seq::default();
    s.hold_p(&p, REST, 6);
    s.hold_p(&p, 0.12, 6); // fires after 22 ms
    for i in 1..=25 {
        let pts = shifted(&p, -25.0 * i as f32 / 25.0, 0.0);
        s.push_p(&pts, &[0.12; 4]);
    }
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![Find]);
}

#[test]
fn with_a_deep_binding_the_latched_press_gives_way_to_a_swipe() {
    use config::Action::{Find, TaskView};
    let mut cfg = press_cfg(4, Find, TaskView);
    cfg.four.enabled = true;
    let p = ring_n(4, 1.0);
    let mut s = Seq::default();
    s.hold_p(&p, REST, 6);
    s.hold_p(&p, 0.09, 6); // light latched, provisional
    for i in 1..=25 {
        let pts = shifted(&p, -25.0 * i as f32 / 25.0, 0.0);
        s.push_p(&pts, &[0.09; 4]);
    }
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![config::Action::DesktopNext]);
}

#[test]
fn a_cancelled_latch_does_not_swallow_the_tap() {
    use config::Action::TaskView;
    // Deep only: a short light touch latches, the pressure eases off (cancel), then the lift is a tap.
    let mut cfg = press_cfg(4, config::Action::None, config::Action::Find);
    cfg.four.enabled = true;
    cfg.four.tap = TaskView;
    let p = ring_n(4, 1.0);
    let mut s = Seq::default();
    s.hold_p(&p, 0.04, 2);
    s.hold_p(&p, 0.10, 5);
    s.hold_p(&p, 0.03, 3);
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![TaskView]);
}

#[test]
fn a_finger_leaving_while_others_stay_cancels_a_latched_press() {
    use config::Action::{Find, TaskView};
    let cfg = press_cfg(2, Find, TaskView);
    let two = row(2, 80.0, 57.0);
    let one = &two[..1];
    let mut s = Seq::default();
    s.hold_p(&two, REST, 8);
    s.hold_p(&two, 0.10, 8); // provisional light latched
    s.hold_p(one, 0.10, 10); // one finger lifts, the other stays down
    s.lift(0);
    assert!(triggers(&s.run(&cfg)).is_empty());
}

#[test]
fn fingers_lifting_one_after_the_other_still_release_the_press() {
    use config::Action::{Find, TaskView};
    let cfg = press_cfg(2, Find, TaskView);
    let two = row(2, 80.0, 57.0);
    let one = &two[..1];
    let mut s = Seq::default();
    s.hold_p(&two, REST, 8);
    s.hold_p(&two, 0.10, 8);
    s.hold_p(one, 0.10, 2); // 22 ms with one finger: within the debounce
    s.lift(0);
    assert_eq!(triggers(&s.run(&cfg)), vec![Find]);
}

// ---- two-finger scroll ----

fn scroll_cfg() -> Config {
    let mut cfg = Config::default();
    cfg.scroll.enabled = true;
    cfg.scroll.direction = config::ScrollDirection::Natural;
    cfg.scroll.accel = 0.0;
    cfg.scroll.inertia = false;
    cfg
}

/// The wheel totals (horizontal, vertical) of all scroll actions, and how many there were.
fn wheel_sum(out: &[(u64, Action)]) -> (i32, i32, usize) {
    out.iter().fold((0, 0, 0), |(h, v, n), (_, a)| match a {
        Action::Scroll { h: dh, v: dv } => (h + dh, v + dv, n + 1),
        _ => (h, v, n),
    })
}

#[test]
fn two_finger_pan_sends_the_expected_wheel_sum() {
    let mut s = Seq::default();
    let start = row(2, 40.0, 40.0);
    s.still(&start, 60);
    // 30 mm down over 300 ms, at the default 20 units per mm: 600 units, upward when natural.
    s.glide(&start, 0.0, 30.0, 300);
    s.lift(0);
    let out = s.run(&scroll_cfg());
    let (h, v, n) = wheel_sum(&out);
    assert_eq!(h, 0);
    assert!((595..=600).contains(&v), "vertical sum {v}");
    assert!(n > 5, "output came in {n} pieces");
    // At most one output per input frame.
    let mut times: Vec<u64> = out
        .iter()
        .filter(|(_, a)| matches!(a, Action::Scroll { .. }))
        .map(|(t, _)| *t)
        .collect();
    times.dedup();
    assert_eq!(times.len(), n);
    // Traditional direction flips the sign and keeps the magnitude.
    let mut cfg = scroll_cfg();
    cfg.scroll.direction = config::ScrollDirection::Traditional;
    let (_, tv, _) = wheel_sum(&s.run(&cfg));
    assert!((595..=600).contains(&-tv), "traditional sum {tv}");
}

#[test]
fn two_finger_pan_speed_settings_scale_the_output() {
    let mut s = Seq::default();
    let start = row(2, 40.0, 40.0);
    s.still(&start, 60);
    s.glide(&start, 20.0, 20.0, 300);
    s.lift(0);
    let mut cfg = scroll_cfg();
    cfg.scroll.axis_lock = false;
    cfg.scroll.vertical_speed = 2.0;
    cfg.scroll.horizontal_speed = 0.5;
    let (h, v, _) = wheel_sum(&s.run(&cfg));
    // 20 mm = 400 units; vertical x2, horizontal x0.5 and flipped (natural: right scrolls left).
    assert!((795..=800).contains(&v), "v {v}");
    assert!((-200..=-195).contains(&h), "h {h}");
}

#[test]
fn two_finger_scroll_is_off_by_default_and_ignores_taps_and_pinches() {
    let mut pan = Seq::default();
    let start = row(2, 40.0, 40.0);
    pan.still(&start, 60);
    pan.glide(&start, 0.0, 30.0, 300);
    pan.lift(0);
    assert_eq!(wheel_sum(&pan.run(&Config::default())).2, 0);

    // A tap with a little wobble.
    let mut tap = Seq::default();
    tap.still(&start, 50);
    tap.glide(&start, 0.0, 1.0, 40);
    tap.lift(0);
    assert_eq!(wheel_sum(&tap.run(&scroll_cfg())).2, 0);

    // A pinch: the fingers spread 12 mm while the centre barely moves.
    let mut pinch = Seq::default();
    pinch.still(&start, 60);
    for i in 1..=25u32 {
        let k = i as f32 / 25.0 * 6.0;
        let pts = [
            (40.0 - 5.0 - k, 40.0 + 0.1 * k),
            (40.0 + 5.0 + k, 40.0 + 0.1 * k),
        ];
        pinch.frames.push(frame(pinch.t, &pts));
        pinch.t += STEP;
    }
    pinch.lift(0);
    assert_eq!(wheel_sum(&pinch.run(&scroll_cfg())).2, 0);
}

#[test]
fn two_finger_scroll_leaves_three_finger_swipes_alone() {
    let mut s = Seq::default();
    let start = row(3, 40.0, 40.0);
    s.still(&start, 60);
    s.glide(&start, 0.0, 30.0, 300);
    s.lift(0);
    assert_eq!(wheel_sum(&s.run(&scroll_cfg())).2, 0);
}

#[test]
fn inertia_continues_after_the_lift_and_ends() {
    let mut s = Seq::default();
    let start = row(2, 40.0, 40.0);
    s.still(&start, 60);
    s.glide(&start, 0.0, 40.0, 200);
    s.lift(0);
    let mut cfg = scroll_cfg();
    let (_, plain, _) = wheel_sum(&s.run(&cfg));
    cfg.scroll.inertia = true;
    let out = s.run(&cfg);
    let (_, glide, n) = wheel_sum(&out);
    assert!(glide > plain + 100, "glide {glide} vs {plain}");
    // The glide stops by itself, within a few seconds of the lift.
    let last = out.last().map(|(t, _)| *t).unwrap();
    assert!(last < s.t + 6_000_000, "still gliding at {last}");
    assert!(n > 10);
}

#[test]
fn a_new_touch_stops_the_glide() {
    let mut s = Seq::default();
    let start = row(2, 40.0, 40.0);
    s.still(&start, 60);
    s.glide(&start, 0.0, 40.0, 200);
    s.lift(60);
    // Put one finger down 60 ms after the lift: whatever is still gliding stops.
    s.still(&[(40.0, 40.0)], 100);
    s.lift(0);
    let mut cfg = scroll_cfg();
    cfg.scroll.inertia = true;
    let out = s.run(&cfg);
    let touch_us = s.frames[s.frames.len() - 10].t_us;
    // The glide really existed: output after the lift and before the new touch.
    let lift_us = s.frames[s.frames.len() - 11].t_us;
    assert!(
        out.iter()
            .any(|(t, a)| matches!(a, Action::Scroll { .. }) && *t > lift_us && *t < touch_us),
        "no glide output before the touch"
    );
    assert!(
        out.iter()
            .filter(|(_, a)| matches!(a, Action::Scroll { .. }))
            .all(|(t, _)| *t < touch_us + 20_000),
        "scrolled after the touch"
    );
}

#[test]
fn a_two_finger_firm_press_beats_scroll_and_a_pan_still_scrolls() {
    use config::Action::Find;
    let mut cfg = press_cfg(2, Find, config::Action::None);
    cfg.scroll.enabled = true;
    cfg.scroll.inertia = false;
    let two = row(2, 40.0, 40.0);
    // A firm press without moving: the press fires, nothing scrolls.
    let mut s = Seq::default();
    s.hold_p(&two, REST, 8);
    s.hold_p(&two, 0.10, 12);
    s.lift(0);
    let out = s.run(&cfg);
    assert_eq!(triggers(&out), vec![Find]);
    assert_eq!(wheel_sum(&out).2, 0, "a press must not scroll");
    // A pan at rest pressure: it scrolls, and no press fires.
    let mut s = Seq::default();
    s.hold_p(&two, REST, 8);
    s.glide(&two, 0.0, 30.0, 300);
    s.lift(0);
    let out = s.run(&cfg);
    assert!(triggers(&out).is_empty());
    assert!(wheel_sum(&out).2 > 5);
}

// ---- drag and long hold / ring on the same finger count ----

/// The user's setup: 3-finger drag, ring menu on 3 and 5 fingers, a 1-finger deep press bound to
/// tab switch, a 4-finger swipe up.
fn overlap_cfg() -> Config {
    let mut cfg = drag_cfg(3);
    cfg.long_hold.enabled = true;
    cfg.long_hold.actions.three = config::Action::RingMenu;
    cfg.long_hold.actions.five = config::Action::RingMenu;
    cfg.press.enabled = true;
    cfg.press.fingers[0].enabled = true;
    cfg.press.fingers[0].light = config::Action::None;
    cfg.press.fingers[0].deep = config::Action::TabSwitch;
    cfg.four.enabled = true;
    cfg.four.swipe_up = config::Action::TaskView;
    cfg
}

impl Seq {
    /// Fingers land one after another (`gap_ms` apart); the first one lands with `first_p`,
    /// the others with `rest_p`. Returns the final positions.
    fn land(&mut self, p: &[(f32, f32)], gap_ms: u64, first_p: f32, rest_p: f32) {
        for k in 1..=p.len() {
            let pr: Vec<f32> = (0..k)
                .map(|i| if i == 0 { first_p } else { rest_p })
                .collect();
            for _ in 0..(gap_ms * 1000 / STEP).max(1) {
                self.push_p(&p[..k], &pr);
            }
        }
    }

    /// `ms` of rest with every contact at pressure `p`.
    fn rest_p(&mut self, p: &[(f32, f32)], pr: f32, ms: u64) {
        self.hold_p(p, pr, (ms * 1000 / STEP).max(1) as usize);
    }

    fn glide_p(&mut self, from: &[(f32, f32)], dx: f32, ms: u64, pr: f32) -> Pts {
        let n = (ms * 1000 / STEP).max(1);
        for i in 1..=n {
            let f = i as f32 / n as f32;
            self.push_p(&shifted(from, dx * f, 0.0), &vec![pr; from.len()]);
        }
        shifted(from, dx, 0.0)
    }
}

fn ring_opened(out: &[(u64, Action)]) -> usize {
    count(out, |a| matches!(a, Action::RingOpen { .. }))
}

/// 3 fingers land `gap_ms` apart (hard first finger), then `then` runs, then all lift.
fn overlap_run(
    cfg: &Config,
    gap_ms: u64,
    then: impl Fn(&mut Seq, &[(f32, f32)]),
) -> Vec<(u64, Action)> {
    let p = row(3, 60.0, 50.0);
    let mut s = Seq::default();
    s.land(&p, gap_ms, 0.15, 0.06);
    then(&mut s, &p);
    s.lift(0);
    s.run(cfg)
}

#[test]
fn overlap_resting_still_opens_the_ring() {
    let out = overlap_run(&overlap_cfg(), 8, |s, p| s.rest_p(p, 0.06, 900));
    assert_eq!(ring_opened(&out), 1);
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 0);
}

#[test]
fn overlap_a_drift_below_the_drag_start_still_opens_the_ring() {
    // Test a: 1.2 mm over 400 ms, then rest. Not a movement yet, so it is still a hold.
    let out = overlap_run(&overlap_cfg(), 8, |s, p| {
        let q = s.glide_p(p, 1.2, 400, 0.06);
        s.rest_p(&q, 0.06, 900);
    });
    assert_eq!(ring_opened(&out), 1);
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 0);
}

#[test]
fn overlap_a_drag_that_pauses_never_opens_the_ring() {
    // Tests b, b2, c: the drag starts at 1.5 mm; resting for longer than the hold time after
    // 2, 3 or 10 mm of movement must not open the ring (it used to spin the timer for 2 mm).
    for dx in [2.0, 3.0, 10.0] {
        let out = overlap_run(&overlap_cfg(), 8, |s, p| {
            s.rest_p(p, 0.06, 50);
            let q = s.glide_p(p, dx, 200, 0.06);
            s.rest_p(&q, 0.06, 900);
        });
        assert_eq!(ring_opened(&out), 0, "dx {dx}");
        assert_eq!(
            count(&out, |a| matches!(a, Action::DragBegin)),
            1,
            "dx {dx}"
        );
    }
}

#[test]
fn overlap_one_finger_drifting_past_the_drag_start_ends_the_hold() {
    // The centroid stays under the drag start (no drag), but a finger moved 2.2 mm: that is
    // movement, so no ring (before, only 3 mm ended the hold).
    let out = overlap_run(&overlap_cfg(), 8, |s, p| {
        let mut q = p.to_vec();
        q[0].0 += 2.2;
        s.rest_p(&q, 0.06, 900);
    });
    assert_eq!(ring_opened(&out), 0);
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 0);
    // Without a drag on this count the old hold travel (3 mm) still applies.
    let mut cfg = overlap_cfg();
    cfg.drag.disable_all();
    let out = overlap_run(&cfg, 8, |s, p| {
        let mut q = p.to_vec();
        q[0].0 += 2.2;
        s.rest_p(&q, 0.06, 900);
    });
    assert_eq!(ring_opened(&out), 1);
}

#[test]
fn overlap_a_plain_long_hold_follows_the_same_rules() {
    let mut cfg = overlap_cfg();
    cfg.long_hold.actions.three = config::Action::Find;
    let still = overlap_run(&cfg, 8, |s, p| s.rest_p(p, 0.06, 900));
    assert_eq!(triggers(&still), vec![config::Action::Find]);
    let moved = overlap_run(&cfg, 8, |s, p| {
        let q = s.glide_p(p, 2.0, 200, 0.06);
        s.rest_p(&q, 0.06, 900);
    });
    assert!(triggers(&moved).is_empty(), "{:?}", triggers(&moved));
    assert_eq!(count(&moved, |a| matches!(a, Action::DragBegin)), 1);
}

#[test]
fn overlap_landing_fingers_do_not_fire_the_one_finger_deep_press() {
    // Test d: the hard first finger is alone for 66 ms or more while the others land.
    for gap in [8, 44, 66, 88] {
        let out = overlap_run(&overlap_cfg(), gap, |s, p| s.rest_p(p, 0.06, 200));
        assert!(triggers(&out).is_empty(), "gap {gap}: {:?}", triggers(&out));
    }
}

#[test]
fn overlap_lifting_fingers_do_not_fire_the_one_finger_deep_press() {
    let out = overlap_run(&overlap_cfg(), 8, |s, p| {
        s.rest_p(p, 0.06, 100);
        s.push_p(&p[..2], &[0.15, 0.06]);
        s.rest_p(&p[..1], 0.15, 200);
    });
    assert!(triggers(&out).is_empty(), "{:?}", triggers(&out));
}

#[test]
fn overlap_a_real_one_finger_deep_press_still_fires() {
    let p = row(1, 60.0, 50.0);
    let mut s = Seq::default();
    s.rest_p(&p, 0.18, 300);
    s.lift(0);
    let out = s.run(&overlap_cfg());
    assert_eq!(triggers(&out), vec![config::Action::TabSwitch]);
}

#[test]
fn overlap_a_brief_fourth_contact_is_no_four_finger_swipe() {
    // Test e: a drag that briefly reads 4 contacts, with and without the move before it.
    let out = overlap_run(&overlap_cfg(), 8, |s, p| {
        let q = s.glide_p(p, 8.0, 200, 0.06);
        let mut f = q.clone();
        f.push((q[2].0 + 10.0, q[2].1));
        s.rest_p(&f, 0.06, 80);
        s.rest_p(&q, 0.06, 100);
        s.glide_p(&q, 5.0, 100, 0.06);
    });
    assert!(triggers(&out).is_empty(), "{:?}", triggers(&out));
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 1);
}

// ---- drag rows: any count, any start, any action ----

fn rows_cfg(n: usize, start: config::DragStart, action: config::DragAction) -> Config {
    let mut cfg = Config::default();
    cfg.drag.disable_all();
    *cfg.drag.row_mut(n).unwrap() = config::DragRow {
        enabled: true,
        start,
        action,
    };
    cfg
}

#[test]
fn a_tap_then_move_row_does_not_steal_a_plain_tap() {
    use config::{DragAction::WindowMove, DragStart::TapThenMove};
    for n in [4usize, 5] {
        let mut cfg = rows_cfg(n, TapThenMove, WindowMove);
        match n {
            4 => cfg.four.tap = config::Action::Find,
            _ => {
                cfg.five.enabled = true;
                cfg.five.tap = config::Action::Find;
            }
        }
        let mut s = Seq::default();
        s.still(&ring_n(n, 1.0), 60);
        s.lift(0);
        let out = s.run(&cfg);
        // The tap still fires (after the short wait) and nothing starts.
        assert_eq!(triggers(&out), vec![config::Action::Find], "n={n}");
        assert_eq!(count(&out, |a| matches!(a, Action::WindowMoveBegin)), 0);
        assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 0);
    }
}

#[test]
fn the_tap_wait_is_dropped_when_a_move_follows() {
    use config::{DragAction::WindowMove, DragStart::TapThenMove};
    let mut cfg = rows_cfg(4, TapThenMove, WindowMove);
    cfg.four.tap = config::Action::Find;
    let mut s = Seq::default();
    let p = row(4, 60.0, 50.0);
    s.still(&p, 70);
    s.lift(100);
    s.still(&p, 100);
    s.glide(&p, 20.0, 0.0, 400);
    s.lift(0);
    let out = s.run(&cfg);
    assert!(triggers(&out).is_empty(), "{:?}", triggers(&out));
    let r = real(&out);
    assert!(matches!(r.first(), Some(Action::WindowMoveBegin)), "{r:?}");
    assert!(
        matches!(r.last(), Some(Action::WindowMoveEnd { .. })),
        "{r:?}"
    );
}

#[test]
fn touch_move_rows_start_at_once_for_each_action() {
    use config::{DragAction, DragStart::TouchMove};
    for n in [3usize, 4, 5] {
        for (action, pred) in [
            (
                DragAction::Drag,
                (|a: &Action| matches!(a, Action::DragBegin)) as fn(&Action) -> bool,
            ),
            (DragAction::WindowMove, |a| {
                matches!(a, Action::WindowMoveBegin)
            }),
            (DragAction::WindowResize, |a| {
                matches!(a, Action::WindowResizeBegin)
            }),
        ] {
            let cfg = rows_cfg(n, TouchMove, action);
            let mut s = Seq::default();
            let p = row(n, 60.0, 50.0);
            s.still(&p, 60);
            s.glide(&p, 20.0, 0.0, 400);
            s.lift(0);
            let out = s.run(&cfg);
            assert_eq!(count(&out, pred), 1, "n={n} {action:?}");
        }
    }
}

#[test]
fn only_a_touch_move_row_shadows_the_swipes() {
    use config::{DragAction::Drag, DragStart};
    let cfg = rows_cfg(4, DragStart::TapThenMove, Drag);
    assert!(cfg.four.enabled);
    let g = super::types::GestureCfg::from_config(&cfg, None);
    let t = rows_cfg(4, DragStart::TouchMove, Drag);
    let gt = super::types::GestureCfg::from_config(&t, None);
    assert!(gt.touch_rows().any(|(n, _)| n == 4));
    assert!(!g.touch_rows().any(|(n, _)| n == 4));
}

#[test]
fn a_tap_survives_a_different_count_touch_in_the_gap() {
    use config::{DragAction::WindowMove, DragStart::TapThenMove};
    let mut cfg = rows_cfg(4, TapThenMove, WindowMove);
    cfg.four.tap = config::Action::Find;
    let mut s = Seq::default();
    s.still(&row(4, 60.0, 50.0), 70);
    s.lift(100);
    // A one-finger touch that lifts again: the four-finger tap is not lost.
    s.still(&row(1, 60.0, 50.0), 60);
    s.lift(0);
    let out = s.run(&cfg);
    assert_eq!(triggers(&out), vec![config::Action::Find]);
    assert_eq!(count(&out, |a| matches!(a, Action::WindowMoveBegin)), 0);
}

#[test]
fn a_tap_survives_a_same_count_touch_that_never_moves() {
    use config::{DragAction::WindowMove, DragStart::TapThenMove};
    let mut cfg = rows_cfg(4, TapThenMove, WindowMove);
    cfg.four.tap = config::Action::Find;
    let mut s = Seq::default();
    let p = row(4, 60.0, 50.0);
    s.still(&p, 70);
    s.lift(100);
    s.still(&p, 500);
    s.lift(0);
    let out = s.run(&cfg);
    assert_eq!(triggers(&out), vec![config::Action::Find]);
    assert_eq!(count(&out, |a| matches!(a, Action::WindowMoveBegin)), 0);
}

// ---- title-bar flick at the end of a drag ----

/// A drag that glides (`dx`, `dy`) in `ms` and lifts; returns the actions.
fn drag_flick_run(cfg: &Config, dx: f32, dy: f32, ms: u64) -> Vec<(u64, Action)> {
    let mut s = Seq::default();
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    s.glide(&p, dx, dy, ms);
    s.lift(0);
    s.run(cfg)
}

fn flicks(out: &[(u64, Action)]) -> Vec<config::Action> {
    out.iter()
        .filter_map(|(_, a)| match a {
            Action::DragFlick(x) => Some(x.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_flick_ends_the_drag_at_once_and_runs_the_direction_action() {
    let cfg = drag_cfg(3);
    for (dx, dy, want) in [
        (-30.0, 0.0, config::Action::AppSwitch),
        (30.0, 0.0, config::Action::AppSwitch),
        (0.0, -30.0, config::Action::TaskView),
        (0.0, 30.0, config::Action::ShowDesktop),
    ] {
        let out = drag_flick_run(&cfg, dx, dy, 66);
        assert_eq!(flicks(&out), vec![want], "{dx},{dy}");
        // DragEnd comes right before the flick, at the lift: no lift timeout to wait for.
        let end = time_of(&out, |a| matches!(a, Action::DragEnd)).expect("drag ends");
        let flick = time_of(&out, |a| matches!(a, Action::DragFlick(_))).unwrap();
        assert_eq!(end, flick);
        assert_eq!(count(&out, |a| matches!(a, Action::DragEnd)), 1);
        let pos = |f: &dyn Fn(&Action) -> bool| out.iter().position(|(_, a)| f(a)).unwrap();
        assert_eq!(
            pos(&|a| matches!(a, Action::DragFlick(_))),
            pos(&|a| matches!(a, Action::DragEnd)) + 1
        );
    }
}

#[test]
fn a_flick_with_fingers_leaving_one_by_one_still_counts() {
    let mut s = Seq::default();
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    let p = s.glide(&p, 0.0, -30.0, 66);
    // Two fingers stay a moment after the third left.
    s.still(&p[..2], 100);
    s.lift(0);
    let out = s.run(&drag_cfg(3));
    assert_eq!(flicks(&out), vec![config::Action::TaskView]);
    assert_eq!(count(&out, |a| matches!(a, Action::DragEnd)), 1);
}

#[test]
fn a_slow_lift_keeps_the_lift_timeout() {
    let out = drag_flick_run(&drag_cfg(3), 30.0, 0.0, 800);
    assert!(flicks(&out).is_empty());
    let end = time_of(&out, |a| matches!(a, Action::DragEnd)).expect("drag ends");
    let lift = out
        .iter()
        .rev()
        .find_map(|(t, a)| matches!(a, Action::DragMove { .. }).then_some(*t))
        .unwrap();
    assert!(end >= lift + 300_000, "ended {end}, last move {lift}");
}

#[test]
fn a_diagonal_flick_is_not_a_flick() {
    let out = drag_flick_run(&drag_cfg(3), 25.0, 25.0, 66);
    assert!(flicks(&out).is_empty());
}

#[test]
fn flick_off_or_unassigned_leaves_the_drag_alone() {
    let mut cfg = drag_cfg(3);
    cfg.drag.flick.enabled = false;
    let out = drag_flick_run(&cfg, 30.0, 0.0, 66);
    assert!(flicks(&out).is_empty());
    let end = time_of(&out, |a| matches!(a, Action::DragEnd)).unwrap();
    let last = out.last().unwrap().0;
    assert_eq!(end, last, "ended by the lift timeout");
    // Only the unassigned direction falls back to the normal end.
    let mut cfg = drag_cfg(3);
    cfg.drag.flick.right = config::Action::None;
    assert!(flicks(&drag_flick_run(&cfg, 30.0, 0.0, 66)).is_empty());
    assert_eq!(
        flicks(&drag_flick_run(&cfg, -30.0, 0.0, 66)),
        vec![config::Action::AppSwitch]
    );
}

#[test]
fn a_flick_ends_a_kept_drag_too() {
    let mut cfg = drag_cfg(3);
    cfg.drag.keep_drag_on_release = true;
    let out = drag_flick_run(&cfg, 0.0, 30.0, 66);
    assert_eq!(flicks(&out), vec![config::Action::ShowDesktop]);
    assert_eq!(count(&out, |a| matches!(a, Action::DragEnd)), 1);
}

#[test]
fn window_move_rows_do_not_flick() {
    let mut s = Seq::default();
    let p = tap_then_touch(&mut s, 100);
    s.glide(&p, 30.0, 0.0, 66);
    s.lift(0);
    let out = s.run(&window_cfg());
    assert!(flicks(&out).is_empty());
    assert_eq!(
        count(&out, |a| matches!(a, Action::WindowMoveEnd { .. })),
        1
    );
}

#[test]
fn a_per_app_override_changes_the_flick_action() {
    let mut cfg = drag_cfg(3);
    let mut p = Profile {
        exe_name: "a.exe".to_owned(),
        enabled: true,
        ..Profile::default()
    };
    p.overrides
        .insert("drag.flick.up".to_owned(), config::Action::Find);
    cfg.profiles.push(p);
    let g = super::types::GestureCfg::from_config(&cfg, Some("A.exe"));
    let f = g.drag_flick.expect("flick on");
    assert_eq!(f.up, config::Action::Find);
    assert_eq!(f.down, config::Action::ShowDesktop);
    let g = super::types::GestureCfg::from_config(&cfg, Some("b.exe"));
    assert_eq!(g.drag_flick.unwrap().up, config::Action::TaskView);
}

// ---- app switch scrub (Alt held, Tab / Shift+Tab with the fingers) ----

fn scrub_cfg() -> Config {
    let mut cfg = Config::default();
    cfg.three.enabled = true;
    cfg.three.swipe_right = config::Action::AppSwitchScrub;
    cfg.three.swipe_left = config::Action::AppSwitchScrub;
    cfg.three.swipe_up = config::Action::AppSwitchScrub;
    cfg.three.swipe_down = config::Action::AppSwitchScrub;
    cfg
}

/// What the scrub did, in order: B(egin), F / R (forward / reverse step), E (end), C (cancelled end).
fn scrub_log(out: &[(u64, Action)]) -> String {
    out.iter()
        .filter_map(|(_, a)| match a {
            Action::AppSwitchBegin => Some('B'),
            Action::AppSwitchStep { forward: true } => Some('F'),
            Action::AppSwitchStep { forward: false } => Some('R'),
            Action::AppSwitchEnd { cancel: false } => Some('E'),
            Action::AppSwitchEnd { cancel: true } => Some('C'),
            _ => None,
        })
        .collect()
}

fn n_of(log: &str, c: char) -> usize {
    log.chars().filter(|&x| x == c).count()
}

/// Three fingers rest, then glide by (dx, dy) in steps of at most 5 mm; returns the end positions.
fn scrub_swipe(s: &mut Seq, dx: f32, dy: f32) -> Pts {
    let p = row(3, 60.0, 50.0);
    s.still(&p, 100);
    let ms = ((dx.abs() + dy.abs()) * 6.0) as u64;
    s.glide(&p, dx, dy, ms)
}

#[test]
fn a_swipe_bound_to_the_scrub_begins_steps_forward_and_ends_at_the_lift() {
    for (dx, dy) in [(60.0, 0.0), (-60.0, 0.0), (0.0, 40.0), (0.0, -40.0)] {
        let mut s = Seq::default();
        scrub_swipe(&mut s, dx, dy);
        s.lift(0);
        let out = s.run(&scrub_cfg());
        let log = scrub_log(&out);
        assert!(log.starts_with('B'), "{dx},{dy}: {log}");
        assert!(log.ends_with('E'), "{dx},{dy}: {log}");
        assert_eq!(n_of(&log, 'B'), 1, "{log}");
        assert_eq!(n_of(&log, 'E'), 1, "{log}");
        assert_eq!(n_of(&log, 'R') + n_of(&log, 'C'), 0, "{log}");
        // The first Tab comes with the Begin; the rest of the travel adds one step per 10 mm.
        let travel = dx.abs() + dy.abs() - 15.0;
        let want = (travel / 10.0) as usize;
        let got = n_of(&log, 'F');
        assert!(
            got + 1 >= want && got <= want + 1,
            "{dx},{dy}: {got} steps for ~{travel} mm: {log}"
        );
        // It is the scrub, not a one-shot trigger.
        assert!(triggers(&out).is_empty());
    }
}

#[test]
fn going_back_steps_the_other_way_with_at_most_one_step_of_dead_travel() {
    let mut s = Seq::default();
    let p = scrub_swipe(&mut s, 60.0, 0.0);
    let log_before = scrub_log(&s.run(&scrub_cfg()));
    // Less than one step back: nothing yet.
    let p = s.glide(&p, -9.0, 0.0, 120);
    let log_small = scrub_log(&s.run(&scrub_cfg()));
    assert_eq!(n_of(&log_small, 'R'), 0, "{log_small}");
    // Then on past one step in all (11 mm from the turning point): the first reverse step.
    let p = s.glide(&p, -2.5, 0.0, 60);
    let log_one = scrub_log(&s.run(&scrub_cfg()));
    assert_eq!(n_of(&log_one, 'R'), 1, "{log_one}");
    // Forward again straight away: it turns at once, one step of travel later.
    let p = s.glide(&p, 13.0, 0.0, 120);
    let log_fwd = scrub_log(&s.run(&scrub_cfg()));
    assert_eq!(
        n_of(&log_fwd, 'F'),
        n_of(&log_before, 'F') + 1,
        "{log_fwd} vs {log_before}"
    );
    assert_eq!(n_of(&log_fwd, 'R'), 1, "{log_fwd}");
    s.glide(&p, 0.0, 0.0, 22);
    s.lift(0);
    assert!(scrub_log(&s.run(&scrub_cfg())).ends_with('E'));
}

#[test]
fn a_slow_creep_adds_up_to_the_same_steps_as_a_fast_move() {
    let steps = |ms: u64| {
        let mut s = Seq::default();
        let p = row(3, 60.0, 50.0);
        s.still(&p, 100);
        let p = s.glide(&p, 30.0, 0.0, 200);
        s.glide(&p, 40.0, 0.0, ms);
        s.lift(0);
        n_of(&scrub_log(&s.run(&scrub_cfg())), 'F')
    };
    assert_eq!(steps(100), steps(3000));
    assert!(steps(100) >= 3);
}

#[test]
fn the_step_distance_follows_the_setting() {
    let steps = |mm: f32| {
        let mut cfg = scrub_cfg();
        cfg.sensitivity.app_switch_step_mm = mm;
        let mut s = Seq::default();
        scrub_swipe(&mut s, 80.0, 0.0);
        s.lift(0);
        n_of(&scrub_log(&s.run(&cfg)), 'F')
    };
    assert!(steps(5.0) > steps(10.0) && steps(10.0) > steps(20.0));
}

#[test]
fn a_finger_lifting_early_ends_the_scrub_once_and_does_not_cancel() {
    let mut s = Seq::default();
    let p = scrub_swipe(&mut s, 40.0, 0.0);
    // One finger leaves, the other two stay on a while, then they leave too.
    s.still(&p[..2], 150);
    s.lift(0);
    let log = scrub_log(&s.run(&scrub_cfg()));
    assert_eq!(n_of(&log, 'E'), 1, "{log}");
    assert_eq!(n_of(&log, 'C'), 0, "{log}");
    assert!(log.ends_with('E'));
}

#[test]
fn fingers_that_go_stale_cancel_the_scrub_so_nothing_switches() {
    // The pad stops reporting with the fingers still down (no lift frame ever comes).
    let mut s = Seq::default();
    scrub_swipe(&mut s, 40.0, 0.0);
    let log = scrub_log(&s.run(&scrub_cfg()));
    assert_eq!(n_of(&log, 'B'), 1, "{log}");
    assert!(log.ends_with('C'), "{log}");
    assert_eq!(n_of(&log, 'E'), 0, "{log}");
}

#[test]
fn a_frame_after_a_long_silence_cancels_the_scrub() {
    let mut s = Seq::default();
    let p = scrub_swipe(&mut s, 40.0, 0.0);
    s.t += 2_000_000;
    s.still(&p, 50);
    s.lift(0);
    let log = scrub_log(&s.run(&scrub_cfg()));
    assert_eq!(n_of(&log, 'C'), 1, "{log}");
    assert_eq!(n_of(&log, 'E'), 0, "{log}");
    // Every Begin got exactly one end.
    assert_eq!(n_of(&log, 'B'), 1, "{log}");
}

#[test]
fn every_begin_is_closed_exactly_once() {
    for lift_ms in [0u64, 50, 400] {
        let mut s = Seq::default();
        let p = scrub_swipe(&mut s, 50.0, 0.0);
        s.still(&p, 100);
        s.lift(lift_ms);
        let log = scrub_log(&s.run(&scrub_cfg()));
        assert_eq!(n_of(&log, 'B'), n_of(&log, 'E') + n_of(&log, 'C'), "{log}");
    }
}

#[test]
fn the_scrub_in_a_tap_or_pinch_slot_is_a_plain_trigger() {
    let mut cfg = scrub_cfg();
    cfg.three.tap = config::Action::AppSwitchScrub;
    cfg.three.pinch_in = config::Action::AppSwitchScrub;
    let mut s = Seq::default();
    s.still(&row(3, 60.0, 50.0), 60);
    s.lift(0);
    let out = s.run(&cfg);
    assert_eq!(triggers(&out), vec![config::Action::AppSwitchScrub]);
    assert_eq!(scrub_log(&out), "");
}

#[test]
fn an_unbound_direction_does_not_start_a_scrub() {
    let mut cfg = scrub_cfg();
    cfg.three.swipe_right = config::Action::None;
    let mut s = Seq::default();
    scrub_swipe(&mut s, 60.0, 0.0);
    s.lift(0);
    assert_eq!(scrub_log(&s.run(&cfg)), "");
}
