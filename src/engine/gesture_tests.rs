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
    cfg.drag.enabled = true;
    cfg.drag.fingers = fingers;
    cfg.drag.lift_timeout_ms = 300;
    cfg.drag.keep_drag_on_release = false;
    cfg
}

fn window_cfg() -> Config {
    let mut cfg = Config::default();
    cfg.window.enabled = true;
    cfg.window.move_with_three_finger = true;
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
fn drag_is_allowed_after_the_tap_window() {
    let mut s = Seq::default();
    let p = tap_then_touch(&mut s, 100);
    // Rest well past 300 ms, then a small drag: it is not a window move any more.
    s.still(&p, 400);
    s.glide(&p, 3.0, 0.0, 100);
    s.lift(0);
    let mut cfg = window_cfg();
    cfg.drag = drag_cfg(3).drag;
    let out = s.run(&cfg);
    assert_eq!(count(&out, |a| matches!(a, Action::DragBegin)), 1);
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
    let mut cfg = window_cfg();
    // Drag on the same finger count must lose to the window move.
    cfg.drag = drag_cfg(3).drag;
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
