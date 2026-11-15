//! Gesture recognizer: a pure state machine from frames to actions.
//!
//! It never touches the OS and never reads a clock; time comes from `Frame::t_us` and `tick`.
//! M3-0 only tracks fingers and emits debug events (count changes, touch start / end, tap,
//! long hold). Real gestures plug in later by turning the same events into other `Action`s.

use std::collections::BTreeMap;

use super::Frame;
use crate::config::Config;

/// Every tunable of the recognizer, in millimetres and microseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// A changed finger count must hold this long before it is believed.
    pub debounce_us: u64,
    /// A tap moves less than this (largest distance of any contact from where it landed).
    pub tap_travel_mm: f32,
    /// A tap is over within this time (first touch to last lift).
    pub tap_max_us: u64,
    /// A long hold moves less than this while it waits.
    pub hold_travel_mm: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            debounce_us: 40_000,
            tap_travel_mm: 2.0,
            tap_max_us: 200_000,
            hold_travel_mm: 3.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GestureEvent {
    /// The debounced finger count changed (0 = all lifted).
    FingerCount(usize),
    TouchStart {
        fingers: usize,
    },
    TouchEnd {
        /// Most fingers that were down at once.
        fingers: usize,
        duration_us: u64,
    },
    Tap {
        fingers: usize,
    },
    LongHold {
        fingers: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Reported for the diagnostics page and tests; never acted on.
    Debug(GestureEvent),
}

/// What the UI shows about the recognizer.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Snapshot {
    pub finger_count: usize,
    /// Centroid of the active contacts in mm.
    pub centroid: Option<(f32, f32)>,
}

#[derive(Debug)]
struct Track {
    start: (f32, f32),
    travel: f32,
}

#[derive(Debug)]
struct Session {
    start_us: u64,
    started: bool,
    peak: usize,
    max_travel: f32,
    hold_us: u64,
    hold_since: Option<u64>,
    hold_fired: bool,
}

#[derive(Debug, Default)]
pub struct Recognizer {
    th: Thresholds,
    tracks: BTreeMap<u32, Track>,
    raw: usize,
    /// Count that differs from `stable`, with the time it was first seen.
    pending: Option<(usize, u64)>,
    stable: usize,
    session: Option<Session>,
    centroid: Option<(f32, f32)>,
}

impl Recognizer {
    pub fn new(th: Thresholds) -> Self {
        Self {
            th,
            ..Self::default()
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            finger_count: self.stable,
            centroid: self.centroid,
        }
    }

    /// True while fingers are on the pad (a gesture may be in flight).
    pub fn active(&self) -> bool {
        self.session.is_some()
    }

    /// Forget everything without emitting events (device gone, config changed, disabled).
    pub fn reset(&mut self) {
        let th = self.th;
        *self = Self::new(th);
    }

    pub fn step(&mut self, frame: &Frame, cfg: &Config) -> Vec<Action> {
        let mut out = Vec::new();
        let t = frame.t_us;
        let active: Vec<_> = frame
            .contacts
            .iter()
            .filter(|c| c.tip && c.confidence)
            .collect();

        let ids: Vec<u32> = active.iter().map(|c| c.id).collect();
        self.tracks.retain(|id, _| ids.contains(id));
        if !active.is_empty() && self.session.is_none() {
            self.session = Some(Session {
                start_us: t,
                started: false,
                peak: 0,
                max_travel: 0.0,
                // The config is read once per gesture.
                hold_us: u64::from(cfg.long_hold.hold_ms) * 1000,
                hold_since: None,
                hold_fired: false,
            });
        }
        for c in &active {
            let pos = (c.x_mm, c.y_mm);
            let tr = self.tracks.entry(c.id).or_insert(Track {
                start: pos,
                travel: 0.0,
            });
            tr.travel = tr.travel.max(dist(tr.start, pos));
            if let Some(s) = self.session.as_mut() {
                s.max_travel = s.max_travel.max(tr.travel);
            }
        }
        self.centroid = (!active.is_empty()).then(|| {
            let n = active.len() as f32;
            (
                active.iter().map(|c| c.x_mm).sum::<f32>() / n,
                active.iter().map(|c| c.y_mm).sum::<f32>() / n,
            )
        });

        let raw = active.len();
        if raw == 0 {
            self.pending = None;
            self.raw = 0;
            self.end_session(t, &mut out);
            return out;
        }
        if raw != self.raw {
            self.pending = (raw != self.stable).then_some((raw, t));
        }
        self.raw = raw;
        self.advance(t, &mut out);
        out
    }

    /// Fire timers that are due at `now_us`.
    pub fn tick(&mut self, now_us: u64) -> Vec<Action> {
        let mut out = Vec::new();
        self.advance(now_us, &mut out);
        out
    }

    /// Time of the next timer, if any (for the engine's wait).
    pub fn next_deadline(&self) -> Option<u64> {
        let debounce = self.pending.map(|(_, since)| since + self.th.debounce_us);
        let hold = self.hold_deadline();
        match (debounce, hold) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn hold_deadline(&self) -> Option<u64> {
        let s = self.session.as_ref()?;
        if s.hold_fired || self.stable == 0 || self.pending.is_some() {
            return None;
        }
        s.hold_since.map(|h| h + s.hold_us)
    }

    fn advance(&mut self, now: u64, out: &mut Vec<Action>) {
        if let Some((n, since)) = self.pending {
            let confirmed = since + self.th.debounce_us;
            if now >= confirmed {
                self.pending = None;
                self.stable = n;
                out.push(Action::Debug(GestureEvent::FingerCount(n)));
                if let Some(s) = self.session.as_mut() {
                    s.peak = s.peak.max(n);
                    // A new count restarts the hold timer; travel stays cumulative.
                    s.hold_since = Some(confirmed);
                    s.hold_fired = false;
                    if !s.started {
                        s.started = true;
                        out.push(Action::Debug(GestureEvent::TouchStart { fingers: n }));
                    }
                }
            }
        }
        if let Some(due) = self.hold_deadline()
            && now >= due
            && let Some(s) = self.session.as_mut()
        {
            if s.max_travel < self.th.hold_travel_mm {
                out.push(Action::Debug(GestureEvent::LongHold {
                    fingers: self.stable,
                }));
            }
            s.hold_fired = true;
        }
    }

    fn end_session(&mut self, t: u64, out: &mut Vec<Action>) {
        let Some(s) = self.session.take() else {
            return;
        };
        self.tracks.clear();
        let duration = t.saturating_sub(s.start_us);
        if s.peak > 0 {
            if !s.hold_fired
                && s.max_travel < self.th.tap_travel_mm
                && duration < self.th.tap_max_us
            {
                out.push(Action::Debug(GestureEvent::Tap { fingers: s.peak }));
            }
            out.push(Action::Debug(GestureEvent::TouchEnd {
                fingers: s.peak,
                duration_us: duration,
            }));
        }
        if self.stable != 0 {
            self.stable = 0;
            out.push(Action::Debug(GestureEvent::FingerCount(0)));
        }
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::replay;
    use crate::input::Contact;

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

    fn events(actions: &[Action]) -> Vec<GestureEvent> {
        actions
            .iter()
            .map(|Action::Debug(e)| *e)
            .collect::<Vec<_>>()
    }

    #[test]
    fn single_finger_tap() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        let mut all = Vec::new();
        all.extend(r.step(&frame(0, vec![c(1, 50.0, 50.0)]), &cfg));
        all.extend(r.step(&frame(60, vec![c(1, 50.5, 50.0)]), &cfg));
        all.extend(r.step(&frame(100, vec![]), &cfg));
        let e = events(&all);
        assert!(e.contains(&GestureEvent::Tap { fingers: 1 }), "{e:?}");
        assert_eq!(e.last(), Some(&GestureEvent::FingerCount(0)));
    }

    #[test]
    fn touch_that_moves_is_not_a_tap() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        let mut all = Vec::new();
        all.extend(r.step(&frame(0, vec![c(1, 50.0, 50.0)]), &cfg));
        all.extend(r.step(&frame(60, vec![c(1, 55.0, 50.0)]), &cfg));
        all.extend(r.step(&frame(100, vec![]), &cfg));
        assert!(
            !events(&all)
                .iter()
                .any(|e| matches!(e, GestureEvent::Tap { .. }))
        );
    }

    #[test]
    fn slow_touch_is_not_a_tap() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        let mut all = Vec::new();
        all.extend(r.step(&frame(0, vec![c(1, 50.0, 50.0)]), &cfg));
        all.extend(r.step(&frame(250, vec![]), &cfg));
        assert!(
            !events(&all)
                .iter()
                .any(|e| matches!(e, GestureEvent::Tap { .. }))
        );
    }

    #[test]
    fn finger_count_waits_for_debounce() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        // One finger lands, the second follows 10 ms later: only "2" is reported.
        let mut all = r.step(&frame(0, vec![c(1, 10.0, 10.0)]), &cfg);
        all.extend(r.step(&frame(10, vec![c(1, 10.0, 10.0), c(2, 30.0, 10.0)]), &cfg));
        assert!(all.is_empty());
        assert_eq!(r.next_deadline(), Some(50_000));
        all.extend(r.step(&frame(55, vec![c(1, 10.0, 10.0), c(2, 30.0, 10.0)]), &cfg));
        let e = events(&all);
        assert_eq!(
            e,
            vec![
                GestureEvent::FingerCount(2),
                GestureEvent::TouchStart { fingers: 2 }
            ]
        );
        assert_eq!(r.snapshot().finger_count, 2);
        assert_eq!(r.snapshot().centroid, Some((20.0, 10.0)));
    }

    #[test]
    fn long_hold_fires_once_from_tick() {
        let mut cfg = Config::default();
        cfg.long_hold.hold_ms = 500;
        let mut r = Recognizer::default();
        let two = vec![c(1, 10.0, 10.0), c(2, 30.0, 10.0)];
        r.step(&frame(0, two.clone()), &cfg);
        assert!(r.tick(100_000).len() == 2); // count + touch start
        assert_eq!(r.next_deadline(), Some(540_000));
        assert!(r.tick(539_999).is_empty());
        assert_eq!(
            events(&r.tick(540_000)),
            vec![GestureEvent::LongHold { fingers: 2 }]
        );
        assert!(r.tick(900_000).is_empty());
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn moving_cancels_long_hold() {
        let mut cfg = Config::default();
        cfg.long_hold.hold_ms = 300;
        let mut r = Recognizer::default();
        r.step(&frame(0, vec![c(1, 10.0, 10.0)]), &cfg);
        r.tick(100_000);
        r.step(&frame(150, vec![c(1, 16.0, 10.0)]), &cfg);
        assert!(r.tick(400_000).is_empty());
    }

    #[test]
    fn untracked_contacts_are_ignored() {
        let cfg = Config::default();
        let mut r = Recognizer::default();
        let mut hover = c(1, 10.0, 10.0);
        hover.tip = false;
        let mut weak = c(2, 20.0, 10.0);
        weak.confidence = false;
        assert!(r.step(&frame(0, vec![hover, weak]), &cfg).is_empty());
        assert!(!r.active());
    }

    #[test]
    fn replay_two_finger_tap() {
        let frames = replay::parse(include_str!("fixtures/two_finger_tap.jsonl")).unwrap();
        let out = replay::run(&frames, &Config::default());
        let e: Vec<_> = out
            .iter()
            .map(|(_, a)| events(std::slice::from_ref(a))[0])
            .collect();
        assert!(e.contains(&GestureEvent::FingerCount(2)), "{e:?}");
        assert!(e.contains(&GestureEvent::Tap { fingers: 2 }), "{e:?}");
        assert!(!e.iter().any(|x| matches!(x, GestureEvent::LongHold { .. })));
    }

    #[test]
    fn replay_three_finger_long_hold() {
        let frames = replay::parse(include_str!("fixtures/three_finger_hold.jsonl")).unwrap();
        let mut cfg = Config::default();
        cfg.long_hold.hold_ms = 500;
        let out = replay::run(&frames, &cfg);
        let hold: Vec<_> = out
            .iter()
            .filter(|(_, a)| matches!(a, Action::Debug(GestureEvent::LongHold { fingers: 3 })))
            .collect();
        assert_eq!(hold.len(), 1, "{out:?}");
        // Held from the 40 ms debounce, so it fires at 540 ms (on the timer, between frames).
        assert_eq!(hold[0].0, 540_000);
        assert!(
            !out.iter()
                .any(|(_, a)| matches!(a, Action::Debug(GestureEvent::Tap { .. })))
        );
    }
}
