//! Ring menu: N fingers rest on the pad, the ring opens at the cursor, the hand moves toward a
//! slot and lifting runs that slot's action.
//!
//! Flow (design section 5):
//!  * Idle: exactly N stable contacts (N is one of the finger counts whose long hold is set to
//!    "ring menu", see `config::Config::ring_counts`) that moved less than the hold travel for the
//!    hold time propose to open the ring.
//!  * Open: the cursor and clicks are frozen; the centroid's offset from where the ring opened
//!    picks a slot (centre = nothing). A new slot must persist for `SELECT_DWELL_US` first.
//!  * Releasing: once the count falls below N the selection is frozen, and every finger must be
//!    up within `RELEASE_US`; then the slot's action runs, exactly once.
//!  * Cancels: a release that is not complete in time, the count coming back to N or above, a
//!    changed set of contacts, Esc, a move further than `RADIUS_MM`, `TIMEOUT_US` open, an
//!    interrupted touch, or lifting with the centre (nothing) selected.
//!
//! The selection uses the translation of the hand only; rotation is not read.

use std::f32::consts::TAU;

use crate::config;
use crate::engine::arbitration::{Owner, Proposal};
use crate::engine::types::{Action, Ctx, GestureCfg, Point, Suppress};

/// The centroid must be this far from the anchor to select a slot.
pub const ENTER_MM: f32 = 8.0;
/// A selected slot clears when the centroid comes back inside this radius.
pub const CLEAR_MM: f32 = 5.0;
/// A neighbouring slot is taken only this far (degrees) past the sector boundary.
pub const HYSTERESIS_DEG: f32 = 8.0;
/// A new selection must hold this long before it is believed.
pub const SELECT_DWELL_US: u64 = 60_000;
/// After the first finger lifts, all must be up within this time.
pub const RELEASE_US: u64 = 250_000;
/// Moving the hand further than this from the anchor cancels the ring.
pub const RADIUS_MM: f32 = 45.0;
/// An open ring that is not used closes after this long.
pub const TIMEOUT_US: u64 = 5_000_000;

fn angle_of(dx: f32, dy: f32) -> f32 {
    let a = dx.atan2(-dy);
    if a < 0.0 { a + TAU } else { a }
}

fn sector_of(dx: f32, dy: f32, n: usize) -> usize {
    let n = n.max(1);
    let step = TAU / n as f32;
    (((angle_of(dx, dy) + step / 2.0) / step).floor() as usize) % n
}

/// The slot picked by the offset (`dx`, `dy`) of the hand from the ring centre, given the slot
/// picked before (`prev`). Slot 0 is up, then clockwise. Pure; the hysteresis makes the choice
/// steady at the sector boundaries.
pub fn select(prev: Option<usize>, dx: f32, dy: f32, n: usize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let r = dx.hypot(dy);
    let Some(prev) = prev.filter(|&p| p < n) else {
        return (r >= ENTER_MM).then(|| sector_of(dx, dy, n));
    };
    if r < CLEAR_MM {
        return None;
    }
    let step = TAU / n as f32;
    let centre = prev as f32 * step;
    let mut d = (angle_of(dx, dy) - centre).abs();
    if d > TAU / 2.0 {
        d = TAU - d;
    }
    if d <= step / 2.0 + HYSTERESIS_DEG.to_radians() {
        Some(prev)
    } else {
        Some(sector_of(dx, dy, n))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Phase {
    /// Counting the hold; `since` is when the full count was first seen at rest.
    #[default]
    Idle,
    Open {
        /// The finger count that opened the ring.
        fingers: usize,
        anchor: Point,
        opened: u64,
        selected: Option<usize>,
        /// A different slot that is waiting for its dwell: (slot, since).
        pending: Option<(Option<usize>, u64)>,
    },
    Releasing {
        fingers: usize,
        selected: Option<usize>,
        since: u64,
    },
    /// Not a ring in this session (or it was cancelled).
    Dead,
}

#[derive(Debug, Default)]
pub struct RingState {
    phase: Phase,
    /// Idle: when the full count of still fingers was first seen, and that count.
    since: Option<(u64, usize)>,
}

impl RingState {
    /// The ring is on screen (open or releasing).
    pub fn is_open(&self) -> bool {
        matches!(self.phase, Phase::Open { .. } | Phase::Releasing { .. })
    }

    /// Time of the next thing that needs a timer, if any.
    pub fn deadline(&self, g: &GestureCfg) -> Option<u64> {
        match self.phase {
            Phase::Idle => {
                let cfg = g.ring.as_ref()?;
                self.since.map(|(s, _)| s + cfg.hold_us)
            }
            Phase::Open {
                opened, pending, ..
            } => Some(
                [
                    Some(opened + TIMEOUT_US),
                    pending.map(|(_, t)| t + SELECT_DWELL_US),
                ]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(opened + TIMEOUT_US),
            ),
            Phase::Releasing { since, .. } => Some(since + RELEASE_US + 1),
            Phase::Dead => None,
        }
    }

    /// Count the hold and propose to open once it is long enough.
    pub fn step(&mut self, ctx: &Ctx, free: bool) -> Option<Proposal> {
        let cfg = ctx.g.ring.as_ref()?;
        if self.phase != Phase::Idle {
            return None;
        }
        // Another gesture owns the session (a drag that began): the ring never opens in it, not
        // even when the hand pauses later. Moving as far as a drag start ends the candidate too.
        if ctx.raw > cfg.max_fingers() || ctx.max_travel >= ctx.g.hold_travel_limit(ctx.th) || !free
        {
            self.phase = Phase::Dead;
            return None;
        }
        if !cfg.opens_with(ctx.raw) || ctx.stable != ctx.raw || ctx.rebase {
            self.since = None;
            return None;
        }
        let since = match self.since {
            Some((t, n)) if n == ctx.raw => t,
            _ => {
                self.since = Some((ctx.now, ctx.raw));
                ctx.now
            }
        };
        (ctx.now >= since + cfg.hold_us && free).then_some(Proposal {
            who: Owner::Ring,
            at_us: ctx.now,
        })
    }

    /// The arbiter gave the ring the session: show it.
    pub fn commit(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let (Some(cfg), Some(anchor)) = (ctx.g.ring.as_ref(), ctx.centroid) else {
            return;
        };
        if self.phase != Phase::Idle {
            return;
        }
        let actions = cfg.items.iter().map(|i| i.action.clone()).collect();
        let labels = cfg
            .items
            .iter()
            .map(|i| {
                if i.label.trim().is_empty() {
                    i.action.label()
                } else {
                    i.label.clone()
                }
            })
            .collect();
        self.phase = Phase::Open {
            fingers: ctx.raw,
            anchor,
            opened: ctx.now,
            selected: None,
            pending: None,
        };
        out.push(Action::Suppress(Suppress::CursorAndClicks));
        out.push(Action::RingOpen { labels, actions });
    }

    /// Move the selection with the hand, freeze it on release and enforce the cancels.
    pub fn drive(&mut self, ctx: &Ctx, out: &mut Vec<Action>) {
        let Some(cfg) = ctx.g.ring.as_ref() else {
            return;
        };
        match self.phase {
            Phase::Open {
                fingers,
                anchor,
                opened,
                selected,
                pending,
            } => {
                if ctx.now.saturating_sub(opened) >= TIMEOUT_US || ctx.raw > fingers {
                    return self.cancel(out);
                }
                if ctx.raw < fingers {
                    // The hand is lifting: freeze what is selected.
                    self.phase = Phase::Releasing {
                        fingers,
                        selected,
                        since: ctx.now,
                    };
                    return;
                }
                if ctx.rebase {
                    return self.cancel(out);
                }
                let Some(c) = ctx.centroid else {
                    return;
                };
                let (dx, dy) = (c.0 - anchor.0, c.1 - anchor.1);
                if dx.hypot(dy) > RADIUS_MM {
                    return self.cancel(out);
                }
                let wanted = select(selected, dx, dy, cfg.items.len());
                let (selected, pending) = if wanted == selected {
                    (selected, None)
                } else {
                    match pending {
                        Some((p, since)) if p == wanted => {
                            if ctx.now.saturating_sub(since) >= SELECT_DWELL_US {
                                out.push(Action::RingSelect(wanted));
                                (wanted, None)
                            } else {
                                (selected, pending)
                            }
                        }
                        _ => (selected, Some((wanted, ctx.now))),
                    }
                };
                self.phase = Phase::Open {
                    fingers,
                    anchor,
                    opened,
                    selected,
                    pending,
                };
            }
            Phase::Releasing { fingers, since, .. } => {
                if ctx.raw >= fingers || ctx.now.saturating_sub(since) > RELEASE_US {
                    self.cancel(out);
                }
            }
            Phase::Idle | Phase::Dead => {}
        }
    }

    /// Every finger is up at `t`: run the selected slot if the release was complete in time.
    pub fn finish(&mut self, g: &GestureCfg, t: u64, out: &mut Vec<Action>) {
        let chosen = match self.phase {
            Phase::Open { selected, .. } => Some(selected),
            Phase::Releasing {
                selected, since, ..
            } if t.saturating_sub(since) <= RELEASE_US => Some(selected),
            Phase::Releasing { .. } => Some(None),
            Phase::Idle | Phase::Dead => None,
        };
        let was_open = self.is_open();
        self.phase = Phase::Idle;
        self.since = None;
        if !was_open {
            return;
        }
        let action = chosen
            .flatten()
            .and_then(|i| {
                g.ring
                    .as_ref()
                    .and_then(|r| r.items.get(i))
                    .map(|it| (i, &it.action))
            })
            .filter(|(_, a)| **a != config::Action::None);
        match action {
            Some((i, a)) => {
                out.push(Action::RingClose { committed: Some(i) });
                out.push(Action::Suppress(Suppress::Off));
                out.push(Action::Trigger(a.clone()));
            }
            None => {
                out.push(Action::RingClose { committed: None });
                out.push(Action::Suppress(Suppress::Off));
            }
        }
    }

    /// Close the ring without running anything (Esc, a cancelled touch, a failed overlay).
    pub fn cancel(&mut self, out: &mut Vec<Action>) {
        if self.is_open() {
            out.push(Action::RingClose { committed: None });
            out.push(Action::Suppress(Suppress::Off));
        }
        self.phase = Phase::Dead;
        self.since = None;
    }

    /// A new touch session starts.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, RingItem};
    use crate::engine::Frame;
    use crate::engine::recognizer::Recognizer;
    use crate::input::Contact;

    fn contact(id: u32, x: f32, y: f32) -> Contact {
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

    /// `n` fingers in a row around (x, y).
    fn hand(n: u32, x: f32, y: f32) -> Vec<Contact> {
        (0..n)
            .map(|i| contact(i + 1, x + i as f32 * 12.0, y))
            .collect()
    }

    fn cfg() -> Config {
        let mut cfg = Config::default();
        cfg.ring.enabled = true;
        cfg.long_hold.enabled = true;
        *cfg.long_hold.actions.get_mut(5) = config::Action::RingMenu;
        cfg.sensitivity.hold_ms = 500;
        cfg.ring.items = (0..4)
            .map(|i| RingItem {
                label: format!("項目{i}"),
                action: config::Action::Keys(format!("F{}", i + 1)),
            })
            .collect();
        cfg
    }

    /// Feed `frames` every 10 ms and return everything that came out, with `tick`s in between.
    struct Rig {
        r: Recognizer,
        cfg: Config,
        out: Vec<Action>,
        t: u64,
    }

    impl Rig {
        fn new(cfg: Config) -> Self {
            Self {
                r: Recognizer::default(),
                cfg,
                out: Vec::new(),
                t: 0,
            }
        }

        fn frame(&mut self, cs: Vec<Contact>) {
            self.out.extend(self.r.step(&frame(self.t, cs), &self.cfg));
            self.t += 10;
        }

        fn hold(&mut self, ms: u64, cs: &[Contact]) {
            for _ in 0..ms / 10 {
                self.frame(cs.to_vec());
                self.out.extend(self.r.tick(self.t * 1000));
            }
        }

        fn ring_actions(&self) -> Vec<Action> {
            self.out
                .iter()
                .filter(|a| {
                    matches!(
                        a,
                        Action::RingOpen { .. }
                            | Action::RingSelect(_)
                            | Action::RingClose { .. }
                            | Action::Suppress(_)
                            | Action::Trigger(_)
                    )
                })
                .cloned()
                .collect()
        }

        fn triggers(&self) -> Vec<config::Action> {
            self.out
                .iter()
                .filter_map(|a| match a {
                    Action::Trigger(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        }
    }

    fn open_rig() -> Rig {
        let mut rig = Rig::new(cfg());
        rig.hold(700, &hand(5, 40.0, 60.0));
        rig
    }

    #[test]
    fn selection_picks_slot_by_direction() {
        // 4 slots: up, right, down, left.
        assert_eq!(select(None, 0.0, -20.0, 4), Some(0));
        assert_eq!(select(None, 20.0, 0.0, 4), Some(1));
        assert_eq!(select(None, 0.0, 20.0, 4), Some(2));
        assert_eq!(select(None, -20.0, 0.0, 4), Some(3));
    }

    #[test]
    fn selection_needs_a_minimum_distance_and_clears_with_hysteresis() {
        assert_eq!(select(None, 0.0, -7.0, 4), None);
        assert_eq!(select(None, 0.0, -8.0, 4), Some(0));
        // Once selected it stays until the hand is back inside the clear radius.
        assert_eq!(select(Some(0), 0.0, -6.0, 4), Some(0));
        assert_eq!(select(Some(0), 0.0, -4.0, 4), None);
        assert_eq!(select(None, 0.0, 0.0, 4), None);
        assert_eq!(select(None, 10.0, 0.0, 0), None);
    }

    #[test]
    fn boundary_has_angular_hysteresis() {
        // 4 slots: the boundary between up and right is 45 degrees.
        let at = |deg: f32| {
            let a = deg.to_radians();
            (a.sin() * 20.0, -a.cos() * 20.0)
        };
        let (x, y) = at(50.0);
        assert_eq!(select(Some(0), x, y, 4), Some(0), "5 degrees past: stay");
        assert_eq!(select(None, x, y, 4), Some(1), "fresh: new slot");
        let (x, y) = at(57.0);
        assert_eq!(select(Some(0), x, y, 4), Some(1), "12 degrees past: switch");
        let (x, y) = at(40.0);
        assert_eq!(select(Some(1), x, y, 4), Some(1), "5 degrees back: stay");
    }

    #[test]
    fn selection_wraps_around_the_top() {
        // 8 slots: slot 7 is up-left.
        assert_eq!(select(None, -14.0, -14.0, 8), Some(7));
        assert_eq!(select(Some(0), -5.0, -20.0, 8), Some(0));
    }

    #[test]
    fn hold_opens_the_ring_with_labels_and_freezes_the_cursor() {
        let rig = open_rig();
        let acts = rig.ring_actions();
        assert_eq!(acts[0], Action::Suppress(Suppress::CursorAndClicks));
        match &acts[1] {
            Action::RingOpen { labels, .. } => {
                assert_eq!(labels, &["項目0", "項目1", "項目2", "項目3"]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(acts.len(), 2);
    }

    #[test]
    fn short_hold_does_not_open() {
        let mut rig = Rig::new(cfg());
        rig.hold(300, &hand(5, 40.0, 60.0));
        rig.frame(vec![]);
        assert!(rig.ring_actions().is_empty());
    }

    #[test]
    fn moving_hand_does_not_open() {
        let mut rig = Rig::new(cfg());
        rig.hold(150, &hand(5, 40.0, 60.0));
        rig.hold(600, &hand(5, 50.0, 60.0));
        assert!(rig.ring_actions().is_empty());
    }

    #[test]
    fn four_fingers_do_not_open_a_five_finger_ring() {
        let mut rig = Rig::new(cfg());
        rig.hold(900, &hand(4, 40.0, 60.0));
        assert!(rig.ring_actions().is_empty());
    }

    #[test]
    fn move_then_lift_runs_the_selected_item_once() {
        let mut rig = open_rig();
        // Move the hand right, 20 mm: slot 1.
        rig.hold(200, &hand(5, 60.0, 60.0));
        assert!(rig.out.contains(&Action::RingSelect(Some(1))));
        // All fingers up in the same frame.
        rig.frame(vec![]);
        rig.frame(vec![]);
        assert_eq!(
            rig.triggers(),
            vec![config::Action::Keys("F2".into())],
            "{:?}",
            rig.ring_actions()
        );
        let acts = rig.ring_actions();
        let close = acts
            .iter()
            .position(|a| matches!(a, Action::RingClose { .. }))
            .unwrap();
        assert_eq!(acts[close], Action::RingClose { committed: Some(1) });
        assert!(acts.contains(&Action::Suppress(Suppress::Off)));
    }

    #[test]
    fn fingers_lifting_one_by_one_within_the_window_still_commit() {
        let mut rig = open_rig();
        rig.hold(200, &hand(5, 40.0, 30.0)); // up: slot 0
        rig.frame(hand(4, 40.0, 30.0));
        rig.frame(hand(3, 40.0, 30.0));
        rig.frame(hand(1, 40.0, 30.0));
        rig.frame(vec![]);
        assert_eq!(rig.triggers(), vec![config::Action::Keys("F1".into())]);
    }

    #[test]
    fn a_slow_release_cancels() {
        let mut rig = open_rig();
        rig.hold(200, &hand(5, 40.0, 30.0));
        rig.hold(400, &hand(4, 40.0, 30.0));
        rig.frame(vec![]);
        assert!(rig.triggers().is_empty());
        let acts = rig.ring_actions();
        assert!(acts.contains(&Action::RingClose { committed: None }));
        assert!(acts.contains(&Action::Suppress(Suppress::Off)));
    }

    #[test]
    fn a_finger_coming_back_during_the_release_cancels() {
        let mut rig = open_rig();
        rig.hold(200, &hand(5, 40.0, 30.0));
        rig.frame(hand(4, 40.0, 30.0));
        rig.frame(hand(5, 40.0, 30.0));
        rig.frame(vec![]);
        assert!(rig.triggers().is_empty());
        assert!(
            rig.ring_actions()
                .contains(&Action::RingClose { committed: None })
        );
    }

    #[test]
    fn lifting_from_the_centre_cancels() {
        let mut rig = open_rig();
        rig.hold(100, &hand(5, 40.0, 60.0));
        rig.frame(vec![]);
        assert!(rig.triggers().is_empty());
        assert!(
            rig.ring_actions()
                .contains(&Action::RingClose { committed: None })
        );
    }

    #[test]
    fn moving_too_far_cancels_and_a_later_lift_runs_nothing() {
        let mut rig = open_rig();
        rig.hold(100, &hand(5, 40.0, 5.0)); // 55 mm up
        let n = rig.ring_actions().len();
        assert!(
            rig.ring_actions()
                .contains(&Action::RingClose { committed: None })
        );
        rig.frame(vec![]);
        assert!(rig.triggers().is_empty());
        assert_eq!(rig.ring_actions().len(), n, "no second close");
    }

    #[test]
    fn the_ring_times_out() {
        let mut rig = open_rig();
        rig.hold(5_200, &hand(5, 40.0, 60.0));
        assert!(
            rig.ring_actions()
                .contains(&Action::RingClose { committed: None })
        );
        rig.frame(vec![]);
        assert!(rig.triggers().is_empty());
    }

    #[test]
    fn external_cancel_closes_once() {
        let mut rig = open_rig();
        rig.hold(100, &hand(5, 60.0, 60.0));
        let extra = rig.r.cancel_ring();
        assert_eq!(
            extra,
            vec![
                Action::RingClose { committed: None },
                Action::Suppress(Suppress::Off)
            ]
        );
        assert!(rig.r.cancel_ring().is_empty());
        rig.frame(vec![]);
        assert!(rig.triggers().is_empty());
    }

    #[test]
    fn a_stale_touch_closes_the_ring() {
        let mut rig = open_rig();
        let due = rig.r.next_deadline().unwrap();
        let out = rig.r.tick(due.max(rig.t * 1000) + 1_000_000);
        assert!(
            out.contains(&Action::RingClose { committed: None }),
            "{out:?}"
        );
    }

    #[test]
    fn disabled_ring_never_opens_and_empty_ring_uses_defaults() {
        let mut c = cfg();
        c.ring.enabled = false;
        let mut rig = Rig::new(c);
        rig.hold(900, &hand(5, 40.0, 60.0));
        assert!(rig.ring_actions().is_empty());
        // No runnable item: the default items are offered instead of nothing.
        let mut c = cfg();
        c.ring.items.clear();
        let mut rig = Rig::new(c);
        rig.hold(900, &hand(5, 40.0, 60.0));
        assert!(!rig.ring_actions().is_empty());
    }

    #[test]
    fn a_slot_without_an_action_does_not_run() {
        let mut c = cfg();
        c.ring.items[1].action = config::Action::None;
        let mut rig = Rig::new(c);
        rig.hold(700, &hand(5, 40.0, 60.0));
        rig.hold(200, &hand(5, 60.0, 60.0));
        rig.frame(vec![]);
        assert!(rig.triggers().is_empty());
        assert!(
            rig.ring_actions()
                .contains(&Action::RingClose { committed: None })
        );
    }

    #[test]
    fn blocked_app_never_opens() {
        let mut rig = Rig::new(cfg());
        rig.r.set_foreground(Some("game.exe"), true);
        rig.hold(900, &hand(5, 40.0, 60.0));
        assert!(rig.ring_actions().is_empty());
    }

    /// Ring menu on 3 fingers, a plain action on 4, nothing on 5.
    fn mixed_cfg() -> Config {
        let mut c = cfg();
        *c.long_hold.actions.get_mut(5) = config::Action::None;
        *c.long_hold.actions.get_mut(3) = config::Action::RingMenu;
        *c.long_hold.actions.get_mut(4) = config::Action::Find;
        c
    }

    #[test]
    fn a_three_finger_long_hold_set_to_the_ring_opens_it() {
        let mut rig = Rig::new(mixed_cfg());
        rig.hold(700, &hand(3, 40.0, 60.0));
        assert!(rig.out.iter().any(|a| matches!(a, Action::RingOpen { .. })));
        rig.hold(200, &hand(3, 60.0, 60.0));
        rig.frame(vec![]);
        rig.frame(vec![]);
        assert_eq!(rig.triggers(), vec![config::Action::Keys("F2".into())]);
    }

    #[test]
    fn another_count_runs_its_own_action_and_not_the_ring() {
        let mut rig = Rig::new(mixed_cfg());
        rig.hold(700, &hand(4, 40.0, 60.0));
        assert!(!rig.out.iter().any(|a| matches!(a, Action::RingOpen { .. })));
        assert_eq!(rig.triggers(), vec![config::Action::Find]);
        // Lifting does not run it a second time.
        rig.frame(vec![]);
        rig.frame(vec![]);
        assert_eq!(rig.triggers(), vec![config::Action::Find]);
        // Five fingers have no binding: nothing happens.
        let mut rig = Rig::new(mixed_cfg());
        rig.hold(900, &hand(5, 40.0, 60.0));
        assert!(rig.ring_actions().is_empty());
    }

    #[test]
    fn long_hold_off_or_ring_off_keeps_the_ring_closed() {
        let mut c = cfg();
        c.long_hold.enabled = false;
        let mut rig = Rig::new(c);
        rig.hold(900, &hand(5, 40.0, 60.0));
        assert!(rig.ring_actions().is_empty());
        let mut c = cfg();
        c.ring.enabled = false;
        let mut rig = Rig::new(c);
        rig.hold(900, &hand(5, 40.0, 60.0));
        assert!(rig.ring_actions().is_empty());
    }
}
