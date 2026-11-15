//! Pause: a runtime switch (never saved) that makes the engine pass everything through.
//!
//! `PauseState` is the pure state machine; `PauseCtl` shares it between the UI, the tray, the
//! shortcut and the engine. The engine is the one that ticks the clock (it wakes at least every
//! 100 ms), so a timed resume needs no timer thread. Pausing mid-gesture is the engine's job:
//! when it sees the switch flip it releases everything it holds (see `Worker::sync_pause`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PauseState {
    /// Paused by the user (shortcut, tray, button).
    manual: bool,
    /// When a timed pause ends.
    resume_at: Option<Instant>,
    /// A full-screen exclusive app is in front and the setting asks to pause for it.
    auto: bool,
    /// The user resumed on purpose while `auto` was on: stay running until the full-screen app
    /// is gone.
    auto_overridden: bool,
}

impl PauseState {
    pub fn is_paused(&self) -> bool {
        self.manual || self.auto
    }

    /// Pause until `resume`, or for `after` when given.
    pub fn pause(&mut self, now: Instant, after: Option<Duration>) {
        self.manual = true;
        self.resume_at = after.map(|d| now + d);
    }

    /// Stop the manual pause (and any pending timer). An automatic pause stays unless the user
    /// asked to override it.
    pub fn resume(&mut self) {
        self.manual = false;
        self.resume_at = None;
    }

    /// The shortcut / tray item: resume when paused for any reason, pause otherwise.
    pub fn toggle(&mut self, now: Instant) {
        if self.is_paused() {
            self.resume();
            if self.auto {
                self.auto = false;
                self.auto_overridden = true;
            }
        } else {
            self.pause(now, None);
        }
    }

    /// A full-screen exclusive app is (not) in front. Returns nothing; read `is_paused`.
    pub fn set_fullscreen(&mut self, fullscreen: bool) {
        if fullscreen {
            if !self.auto_overridden {
                self.auto = true;
            }
        } else {
            self.auto = false;
            self.auto_overridden = false;
        }
    }

    /// End a timed pause whose time has come. True when it did.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.manual && self.resume_at.is_some_and(|t| now >= t) {
            self.resume();
            return true;
        }
        false
    }

    /// Time left of a timed pause.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.manual
            .then_some(self.resume_at)
            .flatten()
            .map(|t| t.saturating_duration_since(now))
    }

    /// Paused by the user rather than by a full-screen app.
    pub fn is_manual(&self) -> bool {
        self.manual
    }
}

/// What the UI shows about the pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PauseView {
    pub paused: bool,
    pub manual: bool,
    pub remaining: Option<Duration>,
}

#[derive(Debug, Default)]
pub struct PauseCtl {
    state: Mutex<PauseState>,
    /// Bumped on every change of `is_paused` (so a watcher shows one toast per change).
    generation: AtomicU64,
}

impl PauseCtl {
    fn edit(&self, f: impl FnOnce(&mut PauseState)) {
        let Ok(mut s) = self.state.lock() else {
            return;
        };
        let before = s.is_paused();
        f(&mut s);
        if s.is_paused() != before {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn toggle(&self) {
        self.edit(|s| s.toggle(Instant::now()));
    }

    /// Pause for `after`, then resume by itself.
    pub fn pause_for(&self, after: Duration) {
        self.edit(|s| s.pause(Instant::now(), Some(after)));
    }

    pub fn set_fullscreen(&self, fullscreen: bool) {
        self.edit(|s| s.set_fullscreen(fullscreen));
    }

    /// Called by the engine on every loop.
    pub fn tick(&self) {
        self.edit(|s| {
            s.tick(Instant::now());
        });
    }

    pub fn paused(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.is_paused())
    }

    pub fn view(&self) -> PauseView {
        let now = Instant::now();
        self.state.lock().map_or_else(
            |_| PauseView::default(),
            |s| PauseView {
                paused: s.is_paused(),
                manual: s.is_manual(),
                remaining: s.remaining(now),
            },
        )
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

/// "あと 12 分" for a timed pause (rounded up), `None` for an open-ended one.
pub fn remaining_label(left: Duration) -> String {
    format!("あと {} 分", left.as_secs().div_ceil(60).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn toggle_pauses_and_resumes() {
        let now = Instant::now();
        let mut s = PauseState::default();
        assert!(!s.is_paused());
        s.toggle(now);
        assert!(s.is_paused() && s.is_manual());
        s.toggle(now);
        assert!(!s.is_paused());
    }

    #[test]
    fn a_timed_pause_resumes_exactly_when_due() {
        let now = Instant::now();
        let mut s = PauseState::default();
        s.pause(now, Some(15 * MIN));
        assert_eq!(s.remaining(now), Some(15 * MIN));
        assert!(!s.tick(now + 14 * MIN));
        assert!(s.is_paused());
        assert_eq!(s.remaining(now + 14 * MIN), Some(MIN));
        assert!(s.tick(now + 15 * MIN));
        assert!(!s.is_paused());
        assert!(!s.tick(now + 16 * MIN), "nothing left to end");
    }

    #[test]
    fn an_open_pause_never_times_out_and_resuming_clears_the_timer() {
        let now = Instant::now();
        let mut s = PauseState::default();
        s.pause(now, None);
        assert!(!s.tick(now + 1000 * MIN));
        assert_eq!(s.remaining(now), None);
        s.pause(now, Some(MIN));
        s.resume();
        assert_eq!(s.remaining(now), None);
        s.toggle(now);
        assert!(
            !s.tick(now + 2 * MIN),
            "the old timer must not end the new pause"
        );
    }

    #[test]
    fn fullscreen_pauses_and_the_user_can_override_until_it_ends() {
        let now = Instant::now();
        let mut s = PauseState::default();
        s.set_fullscreen(true);
        assert!(s.is_paused() && !s.is_manual());
        // The user resumes on purpose: the full-screen app stays, yubi runs.
        s.toggle(now);
        assert!(!s.is_paused());
        s.set_fullscreen(true);
        assert!(!s.is_paused(), "polling again must not undo the override");
        // The full-screen app closes, then another one opens: pause again.
        s.set_fullscreen(false);
        s.set_fullscreen(true);
        assert!(s.is_paused());
    }

    #[test]
    fn a_manual_pause_survives_the_fullscreen_app_leaving() {
        let now = Instant::now();
        let mut s = PauseState::default();
        s.set_fullscreen(true);
        s.pause(now, None);
        s.set_fullscreen(false);
        assert!(s.is_paused());
    }

    #[test]
    fn the_controller_counts_changes_of_the_paused_state_only() {
        let c = PauseCtl::default();
        assert_eq!(c.generation(), 0);
        c.toggle();
        assert!(c.paused());
        assert_eq!(c.generation(), 1);
        // Same state again: no new generation.
        c.set_fullscreen(true);
        assert_eq!(c.generation(), 1);
        c.toggle();
        assert!(!c.paused());
        assert_eq!(c.generation(), 2);
        c.pause_for(Duration::ZERO);
        assert_eq!(c.generation(), 3);
        c.tick();
        assert!(!c.paused());
        assert_eq!(c.generation(), 4);
    }

    #[test]
    fn remaining_label_rounds_up() {
        assert_eq!(remaining_label(Duration::from_secs(1)), "あと 1 分");
        assert_eq!(remaining_label(Duration::from_secs(61)), "あと 2 分");
        assert_eq!(remaining_label(15 * MIN), "あと 15 分");
    }
}
