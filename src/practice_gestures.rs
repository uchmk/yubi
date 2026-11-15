//! Windows' own 3- and 4-finger gestures are switched off while 練習モード is on, then put back.
//!
//! The decisions are pure ([`next_step`], [`settle`]) so they can be tested; the registry work is
//! done by [`crate::win_gestures`] with a backup file of its own, apart from the one of the
//! permanent button on the 全般 page.

use std::path::Path;

use crate::win_gestures;

/// File next to the config that holds the values taken before practice switched them off.
pub const PRACTICE_BACKUP_FILE: &str = "windows-gestures-practice-backup.toml";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum State {
    /// Practice has not touched Windows' settings (and no practice backup is left).
    #[default]
    Off,
    /// Practice switched them off and holds the backup; they must be put back.
    Held,
    /// Practice is on but there was nothing to do (already off, or the permanent button owns it).
    NotNeeded,
    /// Switching off failed before or while writing (text for the page). A backup may exist if
    /// some values were already written, so ending practice still goes through `release`.
    Failed(String),
    /// Putting the values back failed: the practice backup file is still on disk. Retried by the
    /// button, at exit and at the next start.
    RestoreFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Nothing,
    Engage,
    /// Engage was not needed: remember that so the registry is not read every frame.
    Skip,
    /// The registry could not be read, so nothing was changed.
    ReadFailed(String),
    Release,
    /// Practice ended: forget `NotNeeded` / `Failed` (restoring from a backup if one was written).
    Clear,
}

/// What to do now. `want`: practice is on and yubi is running. `windows_on` and `user_backup` are
/// only asked when the answer matters: whether Windows' gestures are still on (`Err`: the
/// registry could not be read), and whether the permanent button already owns them (its backup
/// exists). A `Held` state is left only when practice ends, or by the app when the permanent
/// button is pressed, so nothing is polled per frame.
pub fn next_step(
    state: &State,
    want: bool,
    windows_on: impl FnOnce() -> Result<bool, String>,
    user_backup: impl FnOnce() -> bool,
) -> Step {
    match state {
        State::Off if want => {
            if user_backup() {
                return Step::Skip;
            }
            match windows_on() {
                Ok(true) => Step::Engage,
                Ok(false) => Step::Skip,
                Err(e) => Step::ReadFailed(e),
            }
        }
        State::Off | State::RestoreFailed(_) => Step::Nothing,
        State::Held if !want => Step::Release,
        State::Held => Step::Nothing,
        State::NotNeeded | State::Failed(_) if !want => Step::Clear,
        State::NotNeeded | State::Failed(_) => Step::Nothing,
    }
}

/// The state after `step` ran; `result` is the outcome of the step's work (Engage, Release, Clear).
pub fn settle(state: &State, step: &Step, result: &Result<(), String>) -> State {
    match (step, result) {
        (Step::Engage, Ok(())) => State::Held,
        (Step::Engage, Err(e)) => State::Failed(e.clone()),
        (Step::Skip, _) => State::NotNeeded,
        (Step::ReadFailed(e), _) => State::Failed(e.clone()),
        (Step::Release | Step::Clear, Ok(())) => State::Off,
        (Step::Release | Step::Clear, Err(e)) => State::RestoreFailed(e.clone()),
        (Step::Nothing, _) => state.clone(),
    }
}

/// What the practice page says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status<'a> {
    Quiet,
    Active,
    Failed(&'a str),
    RestoreFailed(&'a str),
}

pub fn status(state: &State) -> Status<'_> {
    match state {
        State::Held => Status::Active,
        State::Failed(e) => Status::Failed(e),
        State::RestoreFailed(e) => Status::RestoreFailed(e),
        State::Off | State::NotNeeded => Status::Quiet,
    }
}

/// Switch Windows' gestures off for practice (own backup, never overwritten).
pub fn engage(backup: &Path) -> Result<(), String> {
    win_gestures::disable(backup)
}

/// Put the values back and delete the practice backup. Does nothing without a backup file.
pub fn release(backup: &Path) -> Result<(), String> {
    if !backup.exists() {
        return Ok(());
    }
    win_gestures::restore_temporary(backup)
}

/// The permanent 無効にする may only run once practice's changes are back (a backup taken while
/// they are still off would record the zeros as the user's own values).
pub fn may_disable_permanently(release: &Result<(), String>) -> bool {
    release.is_ok()
}

/// Whether Windows' 3/4-finger gestures are still on; `Err` when the registry cannot be read.
pub fn windows_on() -> Result<bool, String> {
    win_gestures::read_values()
        .map(|v| win_gestures::any_enabled(&v))
        .map_err(|e| format!("Windows の設定を読めません: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(state: &State, want: bool, win_on: bool, user: bool) -> Step {
        next_step(state, want, || Ok(win_on), || user)
    }

    fn run(state: &State, step: Step, r: Result<(), String>) -> State {
        settle(state, &step, &r)
    }

    #[test]
    fn engages_only_when_windows_gestures_are_on_and_the_user_has_no_backup() {
        assert_eq!(step(&State::Off, true, true, false), Step::Engage);
        assert_eq!(step(&State::Off, true, false, false), Step::Skip);
        assert_eq!(step(&State::Off, true, true, true), Step::Skip);
        assert_eq!(step(&State::Off, false, true, false), Step::Nothing);
    }

    #[test]
    fn an_unreadable_registry_is_a_failure_not_gestures_off() {
        let s = next_step(&State::Off, true, || Err("no".to_owned()), || false);
        assert_eq!(s, Step::ReadFailed("no".into()));
        assert_eq!(run(&State::Off, s, Ok(())), State::Failed("no".into()));
    }

    #[test]
    fn held_is_released_only_when_practice_ends() {
        assert_eq!(step(&State::Held, false, true, false), Step::Release);
        let boom = || -> bool { panic!("must not be polled") };
        assert_eq!(
            next_step(&State::Held, true, || Ok(false), boom),
            Step::Nothing
        );
    }

    #[test]
    fn skipped_and_failed_wait_until_practice_ends_then_clear() {
        let failed = State::Failed("x".into());
        for s in [&State::NotNeeded, &failed] {
            assert_eq!(step(s, true, true, false), Step::Nothing);
            assert_eq!(step(s, false, true, false), Step::Clear);
        }
    }

    #[test]
    fn a_partly_applied_engage_is_restored_when_practice_ends() {
        // Engage failed after the backup was written: Failed, then Clear releases from it.
        let st = run(&State::Off, Step::Engage, Err("write".into()));
        assert_eq!(st, State::Failed("write".into()));
        assert_eq!(step(&st, false, true, false), Step::Clear);
        // The release runs the restore; its failure must stay visible.
        assert_eq!(
            run(&st, Step::Clear, Err("locked".into())),
            State::RestoreFailed("locked".into())
        );
        assert_eq!(run(&st, Step::Clear, Ok(())), State::Off);
    }

    #[test]
    fn a_failed_restore_is_never_quiet_and_never_re_engages() {
        let st = run(&State::Held, Step::Release, Err("locked".into()));
        assert_eq!(st, State::RestoreFailed("locked".into()));
        assert_eq!(status(&st), Status::RestoreFailed("locked"));
        assert_eq!(step(&st, true, true, false), Step::Nothing);
        assert_eq!(step(&st, false, true, false), Step::Nothing);
    }

    #[test]
    fn registry_is_not_asked_when_the_state_does_not_need_it() {
        let boom = || -> bool { panic!("must not be asked") };
        let boom_win = || -> Result<bool, String> { panic!("must not be asked") };
        assert_eq!(next_step(&State::Off, false, boom_win, boom), Step::Nothing);
        assert_eq!(
            next_step(&State::NotNeeded, true, boom_win, boom),
            Step::Nothing
        );
    }

    #[test]
    fn full_session_goes_off_held_off() {
        let mut s = State::Off;
        let a = step(&s, true, true, false);
        s = run(&s, a, Ok(()));
        assert_eq!(status(&s), Status::Active);
        let a = step(&s, false, true, false);
        s = run(&s, a, Ok(()));
        assert_eq!(s, State::Off);
        assert_eq!(status(&State::Failed("x".into())), Status::Failed("x"));
    }

    #[test]
    fn release_without_a_backup_is_a_no_op() {
        let none = std::env::temp_dir().join("yubi-pg-none").join("none.toml");
        assert!(release(&none).is_ok());
    }

    /// The permanent button after a failed practice restore must not go on: the registry still
    /// holds practice's zeros, and a permanent backup taken now would keep them forever.
    #[test]
    fn permanent_disable_is_refused_after_a_failed_release() {
        assert!(may_disable_permanently(&Ok(())));
        assert!(!may_disable_permanently(&Err("locked".into())));
    }
}
