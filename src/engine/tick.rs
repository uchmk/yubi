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

#[cfg(test)]
mod tests {
    use super::*;

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
