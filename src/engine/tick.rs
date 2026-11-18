//! Haptic ticks for edge slider steps: which steps tick, how hard, and how often.

use std::time::{Duration, Instant};

use crate::config::HapticStrength;
use crate::haptic::Strength;

/// Two ticks are at least this far apart (a fast slide would otherwise buzz continuously).
pub const MIN_TICK_GAP: Duration = Duration::from_millis(40);

/// What the dispatcher saw of one volume / brightness step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelStep {
    /// The step came from an edge slider (a gesture bound to the same action does not tick).
    pub from_edge: bool,
    /// The level actually moved (not stuck at 0 % or 100 %).
    pub changed: bool,
}

impl LevelStep {
    /// Whether this step earns a tick.
    pub fn wants_tick(&self) -> bool {
        self.from_edge && self.changed
    }

    /// Fold a second step of the same action into this one (an edge may run two outputs).
    pub fn merge(self, other: Self) -> Self {
        Self {
            from_edge: self.from_edge || other.from_edge,
            changed: self.changed || other.changed,
        }
    }
}

/// True when `after` differs from `before`. An unknown value counts as changed.
pub fn level_changed(before: Option<f32>, after: Option<f32>) -> bool {
    match (before, after) {
        (Some(b), Some(a)) => (a - b).abs() > 1e-4,
        _ => true,
    }
}

/// A step tick is a light one, derived from the press strength setting: Firm gives a Medium
/// tick, Light and Medium both give a Weak one (Light cannot go any softer).
pub fn step_strength(s: HapticStrength) -> Strength {
    match s {
        HapticStrength::Light | HapticStrength::Medium => Strength::Weak,
        HapticStrength::Firm => Strength::Medium,
    }
}

#[derive(Debug, Default)]
pub struct TickLimiter {
    last: Option<Instant>,
}

impl TickLimiter {
    /// Remember a tick that was given without asking (the next one waits from here).
    pub fn reset(&mut self, now: Instant) {
        self.last = Some(now);
    }

    /// True (and remembered) when a tick at `now` is far enough from the previous one.
    pub fn allow(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < MIN_TICK_GAP)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// What the ring did, as far as the hand can feel it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingEvent {
    /// The ring appeared.
    Open,
    /// A different slot is highlighted.
    Change,
    /// An item is running.
    Run,
    /// The ring was cancelled.
    Cancel,
}

impl RingEvent {
    /// The event an engine action stands for (the recognizer only sends `RingSelect` when the
    /// highlight really moved; moving back to the centre is not a slot change).
    pub fn of(a: &super::types::Action) -> Option<Self> {
        use super::types::Action;
        match a {
            Action::RingOpen { .. } => Some(Self::Open),
            Action::RingSelect(Some(_)) => Some(Self::Change),
            Action::RingClose { committed: Some(_) } => Some(Self::Run),
            Action::RingClose { committed: None } => Some(Self::Cancel),
            _ => None,
        }
    }
}

/// Which ticks the ring gives and how hard: a medium one on open, a light one per slot change
/// (at most one per 40 ms), a firm one when an item runs, nothing on cancel.
#[derive(Debug, Default)]
pub struct RingTicks {
    limiter: TickLimiter,
}

impl RingTicks {
    pub fn strength(&mut self, ev: RingEvent, now: Instant) -> Option<Strength> {
        match ev {
            RingEvent::Open => {
                self.limiter.reset(now);
                Some(Strength::Medium)
            }
            RingEvent::Change => self.limiter.allow(now).then_some(Strength::Weak),
            RingEvent::Run => {
                self.limiter.reset(now);
                Some(Strength::Strong)
            }
            RingEvent::Cancel => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_events_follow_the_actions() {
        use super::super::types::Action;
        assert_eq!(
            RingEvent::of(&Action::RingOpen {
                labels: vec![],
                actions: vec![],
                anim: Default::default(),
                pointer_dot: true,
            }),
            Some(RingEvent::Open)
        );
        assert_eq!(
            RingEvent::of(&Action::RingSelect(Some(2))),
            Some(RingEvent::Change)
        );
        assert_eq!(RingEvent::of(&Action::RingSelect(None)), None);
        // The pointer position is not an event: no haptics.
        assert_eq!(RingEvent::of(&Action::RingPoint { dx: 3.0, dy: 1.0 }), None);
        assert_eq!(
            RingEvent::of(&Action::RingClose { committed: Some(1) }),
            Some(RingEvent::Run)
        );
        assert_eq!(
            RingEvent::of(&Action::RingClose { committed: None }),
            Some(RingEvent::Cancel)
        );
        assert_eq!(RingEvent::of(&Action::DragBegin), None);
    }

    #[test]
    fn ring_ticks_have_strengths_and_a_rate_limit() {
        let mut t = RingTicks::default();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        assert_eq!(t.strength(RingEvent::Open, ms(0)), Some(Strength::Medium));
        // A change right after the open is held back, a later one is light.
        assert_eq!(t.strength(RingEvent::Change, ms(10)), None);
        assert_eq!(t.strength(RingEvent::Change, ms(50)), Some(Strength::Weak));
        assert_eq!(t.strength(RingEvent::Change, ms(70)), None);
        assert_eq!(t.strength(RingEvent::Change, ms(90)), Some(Strength::Weak));
        // Running is firm even right after a change; cancelling is silent.
        assert_eq!(t.strength(RingEvent::Run, ms(95)), Some(Strength::Strong));
        assert_eq!(t.strength(RingEvent::Cancel, ms(500)), None);
        // Cancel does not disturb the limiter.
        assert_eq!(t.strength(RingEvent::Change, ms(500)), Some(Strength::Weak));
    }

    #[test]
    fn limiter_spaces_ticks() {
        let mut l = TickLimiter::default();
        let t0 = Instant::now();
        assert!(l.allow(t0));
        assert!(!l.allow(t0 + Duration::from_millis(10)));
        assert!(!l.allow(t0 + Duration::from_millis(39)));
        assert!(l.allow(t0 + Duration::from_millis(40)));
        // A refused tick does not push the next one back.
        assert!(!l.allow(t0 + Duration::from_millis(70)));
        assert!(l.allow(t0 + Duration::from_millis(80)));
    }

    #[test]
    fn clamped_steps_do_not_tick() {
        assert!(level_changed(Some(0.5), Some(0.52)));
        assert!(!level_changed(Some(1.0), Some(1.0)));
        assert!(!level_changed(Some(0.0), Some(0.0)));
        assert!(level_changed(None, Some(0.5)));
        assert!(level_changed(Some(0.5), None));
        let edge = |changed| LevelStep {
            from_edge: true,
            changed,
        };
        assert!(edge(true).wants_tick());
        assert!(!edge(false).wants_tick());
        let gesture = LevelStep {
            from_edge: false,
            changed: true,
        };
        assert!(!gesture.wants_tick());
        assert!(edge(false).merge(edge(true)).wants_tick());
    }

    #[test]
    fn step_ticks_are_light() {
        assert_eq!(step_strength(HapticStrength::Light), Strength::Weak);
        assert_eq!(step_strength(HapticStrength::Medium), Strength::Weak);
        assert_eq!(step_strength(HapticStrength::Firm), Strength::Medium);
    }
}
