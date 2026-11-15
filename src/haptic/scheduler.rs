//! Pure click scheduling: spacing, coalescing and pulse expansion (no I/O, testable with fake time).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Minimum time between two actuations (the driver itself enforces 20 ms).
pub const MIN_SPACING: Duration = Duration::from_millis(25);
/// Upper bound on queued clicks; extra ones are dropped.
const MAX_QUEUE: usize = 32;
/// Upper bound on clicks in one pulse.
const MAX_PULSE: u32 = 16;

#[derive(Debug, Clone, Copy)]
struct Item {
    due: Instant,
    value: u32,
}

#[derive(Debug, Default)]
pub struct Scheduler {
    queue: VecDeque<Item>,
    last_sent: Option<Instant>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn insert(&mut self, item: Item) {
        if self.queue.len() >= MAX_QUEUE {
            return;
        }
        let pos = self
            .queue
            .iter()
            .position(|q| q.due > item.due)
            .unwrap_or(self.queue.len());
        self.queue.insert(pos, item);
    }

    /// Queue one click for "now". Dropped when an identical click is already queued within
    /// [`MIN_SPACING`] (burst coalescing).
    pub fn push_click(&mut self, now: Instant, value: u32) {
        let dup = self
            .queue
            .iter()
            .any(|q| q.value == value && q.due <= now + MIN_SPACING);
        if !dup {
            self.insert(Item { due: now, value });
        }
    }

    /// Queue `count` clicks (clamped to 1..=16), `interval` apart (at least [`MIN_SPACING`]).
    pub fn push_pulse(&mut self, now: Instant, value: u32, count: u32, interval: Duration) {
        let step = interval.max(MIN_SPACING);
        for i in 0..count.clamp(1, MAX_PULSE) {
            self.insert(Item {
                due: now + step * i,
                value,
            });
        }
    }

    /// When the next click may be sent (respecting the spacing since the last one).
    pub fn next_due(&self) -> Option<Instant> {
        let front = self.queue.front()?;
        Some(match self.last_sent {
            Some(t) => front.due.max(t + MIN_SPACING),
            None => front.due,
        })
    }

    /// Take the next click if it may be sent at `now`.
    pub fn pop_due(&mut self, now: Instant) -> Option<u32> {
        if self.next_due()? <= now {
            self.last_sent = Some(now);
            self.queue.pop_front().map(|i| i.value)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn single_click_is_immediate() {
        let t0 = Instant::now();
        let mut s = Scheduler::new();
        s.push_click(t0, 7);
        assert_eq!(s.pop_due(t0), Some(7));
        assert!(s.is_empty());
    }

    #[test]
    fn burst_of_identical_clicks_is_coalesced() {
        let t0 = Instant::now();
        let mut s = Scheduler::new();
        for i in 0..10 {
            s.push_click(t0 + ms(i), 7);
        }
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn different_values_are_kept_and_spaced() {
        let t0 = Instant::now();
        let mut s = Scheduler::new();
        s.push_click(t0, 1);
        s.push_click(t0, 2);
        assert_eq!(s.pop_due(t0), Some(1));
        assert_eq!(s.pop_due(t0 + ms(10)), None);
        assert_eq!(s.next_due(), Some(t0 + MIN_SPACING));
        assert_eq!(s.pop_due(t0 + ms(25)), Some(2));
    }

    #[test]
    fn pulse_respects_minimum_spacing() {
        let t0 = Instant::now();
        let mut s = Scheduler::new();
        s.push_pulse(t0, 5, 3, ms(5));
        assert_eq!(s.len(), 3);
        assert_eq!(s.pop_due(t0), Some(5));
        assert_eq!(s.pop_due(t0 + ms(24)), None);
        assert_eq!(s.pop_due(t0 + ms(25)), Some(5));
        assert_eq!(s.pop_due(t0 + ms(49)), None);
        assert_eq!(s.pop_due(t0 + ms(50)), Some(5));
        assert_eq!(s.pop_due(t0 + ms(500)), None);
    }

    #[test]
    fn pulse_is_clamped_and_queue_is_bounded() {
        let t0 = Instant::now();
        let mut s = Scheduler::new();
        s.push_pulse(t0, 1, 1000, ms(30));
        assert_eq!(s.len(), 16);
        s.push_pulse(t0, 2, 16, ms(30));
        s.push_pulse(t0, 3, 16, ms(30));
        assert_eq!(s.len(), 32);
        s.push_pulse(t0, 4, 0, ms(30));
        assert_eq!(s.len(), 32);
    }

    #[test]
    fn click_during_pulse_is_inserted_in_order() {
        let t0 = Instant::now();
        let mut s = Scheduler::new();
        s.push_pulse(t0, 1, 3, ms(100));
        s.push_click(t0 + ms(10), 2);
        assert_eq!(s.pop_due(t0 + ms(10)), Some(1));
        assert_eq!(s.pop_due(t0 + ms(40)), Some(2));
        assert_eq!(s.pop_due(t0 + ms(100)), Some(1));
    }
}
