//! The pause / resume message: a small card from the overlay window (visible about 3 s), instead
//! of a tray balloon whose duration Windows decides. The balloon stays as a fallback.

/// The card text for a pause (`true`) or resume (`false`).
pub fn text(paused: bool) -> &'static str {
    if paused {
        "yubi を一時停止しました"
    } else {
        "yubi を再開しました"
    }
}

/// Owns the overlay window used for the notice (opened on first use; never in tests).
#[derive(Default)]
pub struct PauseNotice {
    #[cfg(all(windows, not(test)))]
    overlay: Option<crate::overlay::Overlay>,
    #[cfg(all(windows, not(test)))]
    failed: bool,
}

impl PauseNotice {
    /// Show the card. False when the overlay is unusable (the caller falls back to the balloon).
    #[cfg(all(windows, not(test)))]
    pub fn show(&mut self, paused: bool) -> bool {
        use crate::overlay::{Overlay, OverlayCmd, OverlayState};
        if self.overlay.is_none() && !self.failed {
            match Overlay::start() {
                Ok(o) => self.overlay = Some(o),
                Err(_) => self.failed = true,
            }
        }
        let ok = self.overlay.as_ref().is_some_and(|o| {
            !matches!(o.state(), OverlayState::Failed(_))
                && o.send(OverlayCmd::Notice(text(paused).to_owned())).is_ok()
        });
        if !ok {
            self.failed = true;
        }
        ok
    }

    #[cfg(not(all(windows, not(test))))]
    pub fn show(&mut self, _paused: bool) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_follows_state() {
        assert_eq!(text(true), "yubi を一時停止しました");
        assert_eq!(text(false), "yubi を再開しました");
    }
}
