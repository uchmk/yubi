//! Contact tracking: who is touching (by driver contact ID), how far each finger has moved, the
//! raw versus debounced finger count, frame freshness and the "wait until everything is lifted"
//! state that follows a cancel. Pure data; the recognizer reads it and never touches the clock.

use std::collections::BTreeMap;

use super::types::Point;
use crate::input::Contact;

/// One finger on the pad, keyed by the driver's contact ID (never by position in the report).
#[derive(Debug, Clone, PartialEq)]
pub struct ContactTrack {
    pub landing: Point,
    #[allow(dead_code, reason = "read by the hold gesture that comes next")]
    pub landing_us: u64,
    pub pos: Point,
    /// Furthest this contact has been from where it landed.
    pub max_disp: f32,
    /// Raw pressure: the median of the last three readings, if the device reports any.
    pub pressure: Option<f32>,
    recent: [f32; 3],
    recent_n: u8,
}

impl ContactTrack {
    fn push_pressure(&mut self, p: f32) {
        if self.recent_n < 3 {
            self.recent[usize::from(self.recent_n)] = p;
            self.recent_n += 1;
        } else {
            self.recent = [self.recent[1], self.recent[2], p];
        }
        let mut v = self.recent;
        let n = usize::from(self.recent_n);
        v[..n].sort_by(f32::total_cmp);
        self.pressure = Some(match n {
            1 => v[0],
            // Two readings: the lower one, so a single spike never passes as a press.
            2 => v[0],
            _ => v[1],
        });
    }
}

/// Device and settings state captured at the first raw touchdown of a session, before any
/// gesture can emit an action. Later changes only apply to the next session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Environment {
    /// Pad width and height in mm, when known.
    pub dims_mm: Option<(f32, f32)>,
    /// Bumped by the engine whenever the config changes.
    pub config_gen: u64,
    /// The foreground app has yubi switched off.
    pub app_blocked: bool,
    /// The foreground app's exe name, which picks the per-app profile for the session.
    pub exe: Option<String>,
}

/// What one frame changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    pub raw: usize,
    /// The first contact of a new touch session appeared in this frame.
    pub touchdown: bool,
    /// Contacts were already down and the set of IDs differs from the previous frame.
    pub ids_changed: bool,
}

#[derive(Debug, Default)]
pub struct Tracker {
    tracks: BTreeMap<u32, ContactTrack>,
    prev_raw: usize,
    pub raw: usize,
    /// Count that differs from `stable`, with the time it was first seen.
    pending: Option<(usize, u64)>,
    pub stable: usize,
    last_frame_us: Option<u64>,
    frames: u32,
    await_all_up: bool,
}

impl Tracker {
    /// Take in one frame of active contacts.
    pub fn observe(&mut self, t: u64, active: &[&Contact]) -> Observed {
        let prev_ids: Vec<u32> = self.tracks.keys().copied().collect();
        self.tracks
            .retain(|id, _| active.iter().any(|c| c.id == *id));
        for c in active {
            let pos = (c.x_mm, c.y_mm);
            let tr = self.tracks.entry(c.id).or_insert(ContactTrack {
                landing: pos,
                landing_us: t,
                pos,
                max_disp: 0.0,
                pressure: None,
                recent: [0.0; 3],
                recent_n: 0,
            });
            tr.pos = pos;
            tr.max_disp = tr.max_disp.max(super::types::dist(tr.landing, pos));
            if let Some(p) = c.pressure.filter(|p| p.is_finite()) {
                tr.push_pressure(p);
            }
        }
        let raw = active.len();
        let ids: Vec<u32> = self.tracks.keys().copied().collect();
        let obs = Observed {
            raw,
            touchdown: self.prev_raw == 0 && raw > 0,
            ids_changed: !prev_ids.is_empty() && !ids.is_empty() && prev_ids != ids,
        };
        self.prev_raw = raw;
        if raw == 0 {
            self.pending = None;
            self.raw = 0;
            self.frames = 0;
            self.last_frame_us = None;
            self.tracks.clear();
        } else {
            if raw != self.raw {
                self.pending = (raw != self.stable).then_some((raw, t));
            }
            self.raw = raw;
            self.frames = self.frames.saturating_add(1);
            self.last_frame_us = Some(t);
        }
        obs
    }

    /// Largest displacement of any contact currently down.
    pub fn max_displacement(&self) -> f32 {
        self.tracks.values().map(|t| t.max_disp).fold(0.0, f32::max)
    }

    pub fn tracks(&self) -> &BTreeMap<u32, ContactTrack> {
        &self.tracks
    }

    /// The pending count if it has held for `debounce_us` at `now`: it becomes the stable count.
    /// Returns the count and the time it was confirmed.
    pub fn confirm(&mut self, now: u64, debounce_us: u64) -> Option<(usize, u64)> {
        // After a cancel nothing is believed until every finger is up; the next touch starts from 0.
        if self.await_all_up {
            return None;
        }
        let (n, since) = self.pending?;
        let confirmed = since + debounce_us;
        if now < confirmed {
            return None;
        }
        self.pending = None;
        self.stable = n;
        Some((n, confirmed))
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn debounce_deadline(&self, debounce_us: u64) -> Option<u64> {
        self.pending.map(|(_, since)| since + debounce_us)
    }

    /// When the touch counts as stale. Armed only after three frames of the session, so a single
    /// report is never judged before the device's cadence has been seen.
    pub fn stale_deadline(&self, stale_us: u64) -> Option<u64> {
        if self.frames < 3 || self.raw == 0 {
            return None;
        }
        self.last_frame_us.map(|t| t + stale_us)
    }

    /// Drop everything about the current touch and ignore the fingers still down until all lift.
    pub fn cancel(&mut self) {
        self.pending = None;
        self.stable = 0;
        self.frames = 0;
        self.last_frame_us = None;
        self.tracks.clear();
        self.await_all_up = true;
    }

    pub fn awaiting_all_up(&self) -> bool {
        self.await_all_up
    }

    pub fn all_up(&mut self) {
        self.await_all_up = false;
        self.pending = None;
        self.stable = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn obs(t: &mut Tracker, at_ms: u64, cs: &[Contact]) -> Observed {
        let refs: Vec<&Contact> = cs.iter().collect();
        t.observe(at_ms * 1000, &refs)
    }

    #[test]
    fn pressure_is_a_three_sample_median() {
        let mut t = Tracker::default();
        let mut p = |ms: u64, v: f32| {
            let mut ct = c(1, 0.0, 0.0);
            ct.pressure = Some(v);
            let refs = [&ct];
            t.observe(ms * 1000, &refs);
            t.tracks()[&1].pressure.unwrap()
        };
        assert_eq!(p(0, 0.10), 0.10);
        assert_eq!(p(10, 0.12), 0.10);
        // A one-frame spike is ignored.
        assert_eq!(p(20, 0.40), 0.12);
        assert_eq!(p(30, 0.12), 0.12);
        assert_eq!(p(40, 0.13), 0.13);
    }

    #[test]
    fn displacement_is_per_contact_id() {
        let mut t = Tracker::default();
        obs(&mut t, 0, &[c(1, 0.0, 0.0), c(2, 20.0, 0.0)]);
        obs(&mut t, 10, &[c(2, 20.0, 0.0), c(1, 3.0, 4.0)]);
        // Report order does not matter: contact 1 moved 5 mm, contact 2 did not move.
        assert!((t.tracks()[&1].max_disp - 5.0).abs() < 1e-4);
        assert_eq!(t.tracks()[&2].max_disp, 0.0);
        assert!((t.max_displacement() - 5.0).abs() < 1e-4);
    }

    #[test]
    fn id_swap_with_same_count_is_a_rebase() {
        let mut t = Tracker::default();
        let first = obs(&mut t, 0, &[c(1, 0.0, 0.0), c(2, 20.0, 0.0)]);
        assert!(first.touchdown && !first.ids_changed);
        let same = obs(&mut t, 10, &[c(1, 1.0, 0.0), c(2, 21.0, 0.0)]);
        assert!(!same.ids_changed && !same.touchdown);
        // One finger lifts and another lands in the same frame: the count is unchanged.
        let swap = obs(&mut t, 20, &[c(1, 1.0, 0.0), c(3, 40.0, 0.0)]);
        assert_eq!(swap.raw, 2);
        assert!(swap.ids_changed);
        // The new contact starts its displacement from where it landed.
        assert_eq!(t.tracks()[&3].landing, (40.0, 0.0));
        assert_eq!(t.tracks()[&3].max_disp, 0.0);
    }

    #[test]
    fn raw_count_is_debounced() {
        let mut t = Tracker::default();
        obs(&mut t, 0, &[c(1, 0.0, 0.0)]);
        obs(&mut t, 10, &[c(1, 0.0, 0.0), c(2, 20.0, 0.0)]);
        assert_eq!((t.raw, t.stable), (2, 0));
        assert_eq!(t.debounce_deadline(40_000), Some(50_000));
        assert!(t.confirm(49_999, 40_000).is_none());
        assert_eq!(t.confirm(50_000, 40_000), Some((2, 50_000)));
        assert_eq!(t.stable, 2);
    }

    #[test]
    fn stale_deadline_needs_three_frames() {
        let mut t = Tracker::default();
        obs(&mut t, 0, &[c(1, 0.0, 0.0)]);
        assert_eq!(t.stale_deadline(150_000), None);
        obs(&mut t, 11, &[c(1, 0.0, 0.0)]);
        assert_eq!(t.stale_deadline(150_000), None);
        obs(&mut t, 22, &[c(1, 0.0, 0.0)]);
        assert_eq!(t.stale_deadline(150_000), Some(172_000));
        obs(&mut t, 20, &[]);
        assert_eq!(t.stale_deadline(150_000), None);
    }

    #[test]
    fn cancel_waits_for_all_up() {
        let mut t = Tracker::default();
        obs(&mut t, 0, &[c(1, 0.0, 0.0)]);
        t.cancel();
        assert!(t.awaiting_all_up());
        assert_eq!(t.stable, 0);
        obs(&mut t, 10, &[c(1, 0.0, 0.0)]);
        assert!(t.awaiting_all_up());
        let up = obs(&mut t, 20, &[]);
        assert_eq!(up.raw, 0);
        t.all_up();
        assert!(!t.awaiting_all_up());
    }

    #[test]
    fn no_count_is_confirmed_while_waiting_for_all_up() {
        let mut t = Tracker::default();
        obs(&mut t, 0, &[c(1, 0.0, 0.0)]);
        t.cancel();
        // More fingers rest during the wait: the count is not believed.
        obs(
            &mut t,
            10,
            &[c(1, 0.0, 0.0), c(2, 20.0, 0.0), c(3, 40.0, 0.0)],
        );
        assert!(t.confirm(100_000, 40_000).is_none());
        assert_eq!(t.stable, 0);
        obs(&mut t, 120, &[]);
        t.all_up();
        // The next touch starts from zero and is confirmed normally.
        obs(&mut t, 200, &[c(4, 0.0, 0.0), c(5, 20.0, 0.0)]);
        assert_eq!(t.confirm(240_000, 40_000), Some((2, 240_000)));
    }
}
