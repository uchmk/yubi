//! Arbitration: gestures run side by side and only propose to commit. The earliest proposal wins;
//! proposals in the same frame are ordered by an explicit priority, never by call order. Once a
//! gesture owns the session the others stay quiet until all fingers are lifted. Disabled gestures
//! and `None` bindings never propose, so they never compete.

/// The gestures that can own a touch session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Drag,
    Window,
    Multi,
    Edge,
    Ring,
    /// A plain long-hold action (the ring-menu choice is `Ring`).
    Hold,
    Pressure,
    Scroll,
}

/// Same-frame tie-break, strongest first. The variants without a gesture yet are reserved so the
/// order is settled before they arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(
    dead_code,
    reason = "owner, hold combo, corner and edge gestures come later"
)]
pub enum Priority {
    /// The gesture that already owns the session.
    Owner,
    HoldCombo,
    Corner,
    Edge,
    Pressure,
    Multi,
    Window,
    Drag,
    /// Two-finger scroll: weaker than a press, which also uses two fingers.
    Scroll,
}

impl Owner {
    pub fn priority(self) -> Priority {
        match self {
            Self::Pressure => Priority::Pressure,
            Self::Multi => Priority::Multi,
            Self::Window => Priority::Window,
            Self::Drag => Priority::Drag,
            Self::Edge => Priority::Edge,
            // The ring is a hold gesture; it shares the hold-combo slot.
            Self::Ring | Self::Hold => Priority::HoldCombo,
            Self::Scroll => Priority::Scroll,
        }
    }
}

/// A gesture asks to take over the session at time `at_us`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proposal {
    pub who: Owner,
    pub at_us: u64,
}

#[derive(Debug, Default)]
pub struct Arbiter {
    owner: Option<Owner>,
}

impl Arbiter {
    #[allow(dead_code, reason = "read by tests and the diagnostics that come next")]
    pub fn owner(&self) -> Option<Owner> {
        self.owner
    }

    /// True while nobody owns the session or `who` does.
    pub fn is_free_for(&self, who: Owner) -> bool {
        self.owner.is_none_or(|o| o == who)
    }

    /// Pick the winner among `proposals`: earliest time, then priority. Once the session has an
    /// owner, only the owner can win again (for example a drag resuming after a finger came back).
    pub fn resolve(&mut self, proposals: &[Proposal]) -> Option<Owner> {
        if let Some(o) = self.owner {
            return proposals.iter().any(|p| p.who == o).then_some(o);
        }
        let win = proposals
            .iter()
            .min_by_key(|p| (p.at_us, p.who.priority()))?;
        self.owner = Some(win.who);
        Some(win.who)
    }

    pub fn clear(&mut self) {
        self.owner = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(who: Owner, at_us: u64) -> Proposal {
        Proposal { who, at_us }
    }

    #[test]
    fn earliest_proposal_wins() {
        let mut a = Arbiter::default();
        assert_eq!(
            a.resolve(&[p(Owner::Drag, 10), p(Owner::Multi, 20)]),
            Some(Owner::Drag)
        );
        assert_eq!(a.owner(), Some(Owner::Drag));
    }

    #[test]
    fn same_frame_ties_follow_priority_not_order() {
        let mut a = Arbiter::default();
        assert_eq!(
            a.resolve(&[
                p(Owner::Drag, 10),
                p(Owner::Window, 10),
                p(Owner::Multi, 10)
            ]),
            Some(Owner::Multi)
        );
        let mut b = Arbiter::default();
        assert_eq!(
            b.resolve(&[p(Owner::Drag, 10), p(Owner::Window, 10)]),
            Some(Owner::Window)
        );
    }

    #[test]
    fn scroll_is_the_weakest_and_a_press_beats_it_in_the_same_frame() {
        let mut a = Arbiter::default();
        assert_eq!(
            a.resolve(&[
                p(Owner::Scroll, 10),
                p(Owner::Drag, 10),
                p(Owner::Pressure, 10)
            ]),
            Some(Owner::Pressure)
        );
        let mut b = Arbiter::default();
        assert_eq!(
            b.resolve(&[p(Owner::Scroll, 10), p(Owner::Drag, 10)]),
            Some(Owner::Drag)
        );
        assert!(Priority::Scroll > Priority::Drag);
    }

    #[test]
    fn owner_is_exclusive_until_cleared() {
        let mut a = Arbiter::default();
        a.resolve(&[p(Owner::Window, 5)]);
        assert!(a.resolve(&[p(Owner::Multi, 6)]).is_none());
        assert!(a.is_free_for(Owner::Window) && !a.is_free_for(Owner::Drag));
        a.clear();
        assert!(a.is_free_for(Owner::Drag));
        assert!(a.resolve(&[]).is_none());
    }

    #[test]
    fn priority_order_is_settled() {
        assert!(Priority::Owner < Priority::HoldCombo);
        assert!(Priority::HoldCombo < Priority::Corner);
        assert!(Priority::Corner < Priority::Edge);
        assert!(Priority::Edge < Priority::Pressure);
        assert!(Priority::Pressure < Priority::Multi);
        assert!(Priority::Multi < Priority::Window);
        assert!(Priority::Window < Priority::Drag);
    }
}
