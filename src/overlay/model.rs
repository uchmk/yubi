//! Pure overlay state: which ring is open, what is selected, which generation owns it.
//! Stale generations are ignored here, so the window code stays free of that logic.

/// One ring slot: a Japanese label and an optional Segoe Fluent Icons glyph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingSlot {
    pub label: String,
    pub icon: Option<char>,
}

/// Messages the engine sends. The overlay has no gesture logic of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayCmd {
    /// Show a ring centred near `anchor_px`. These are physical screen pixels (not DPI-scaled
    /// logical ones): the engine must pass physical coordinates.
    Open {
        anchor_px: (i32, i32),
        items: Vec<RingSlot>,
        generation: u64,
    },
    /// Highlight a sector (`None` = the centre / cancel zone).
    Select {
        index: Option<usize>,
        generation: u64,
    },
    /// Hide the ring. With `committed`, flash that sector for 120 ms first.
    Close {
        committed: Option<usize>,
        generation: u64,
    },
}

/// What the window has to do after a command was applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Stale or meaningless: nothing changed.
    None,
    Open,
    Redraw,
    /// Flash the committed sector, then fade out.
    Flash,
    /// Fade out (cancel).
    Hide,
}

#[derive(Debug, Default)]
pub struct Model {
    /// Highest generation seen so far; older commands are stale.
    pub generation: u64,
    pub open: bool,
    /// True once any ring has been opened (so generation 0 is a real generation too).
    started: bool,
    pub anchor: (i32, i32),
    pub items: Vec<RingSlot>,
    pub selected: Option<usize>,
}

impl Model {
    pub fn apply(&mut self, cmd: OverlayCmd) -> Effect {
        match cmd {
            OverlayCmd::Open {
                anchor_px,
                items,
                generation,
            } => {
                // Older generations are stale; a closed generation cannot be re-opened.
                if generation < self.generation
                    || (self.started && generation == self.generation && !self.open)
                    || items.is_empty()
                {
                    return Effect::None;
                }
                self.started = true;
                self.generation = generation;
                self.open = true;
                self.anchor = anchor_px;
                self.items = items;
                self.selected = None;
                Effect::Open
            }
            OverlayCmd::Select { index, generation } => {
                if !self.open || generation != self.generation {
                    return Effect::None;
                }
                let index = index.filter(|&i| i < self.items.len());
                if index == self.selected {
                    return Effect::None;
                }
                self.selected = index;
                Effect::Redraw
            }
            OverlayCmd::Close {
                committed,
                generation,
            } => {
                if !self.open || generation != self.generation {
                    return Effect::None;
                }
                self.open = false;
                match committed.filter(|&i| i < self.items.len()) {
                    Some(i) => {
                        self.selected = Some(i);
                        Effect::Flash
                    }
                    None => Effect::Hide,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<RingSlot> {
        (0..n)
            .map(|i| RingSlot {
                label: format!("項目{i}"),
                icon: None,
            })
            .collect()
    }

    fn open(g: u64) -> OverlayCmd {
        OverlayCmd::Open {
            anchor_px: (10, 20),
            items: items(8),
            generation: g,
        }
    }

    #[test]
    fn stale_generations_are_ignored() {
        let mut m = Model::default();
        assert_eq!(m.apply(open(5)), Effect::Open);
        assert_eq!(m.apply(open(4)), Effect::None);
        let sel = |index, generation| OverlayCmd::Select { index, generation };
        assert_eq!(m.apply(sel(Some(1), 4)), Effect::None);
        assert_eq!(m.selected, None);
        assert_eq!(m.apply(sel(Some(1), 5)), Effect::Redraw);
        assert_eq!(m.apply(sel(Some(1), 5)), Effect::None);
        assert_eq!(m.apply(sel(Some(99), 5)), Effect::Redraw);
        assert_eq!(m.selected, None);
        let close = |committed, generation| OverlayCmd::Close {
            committed,
            generation,
        };
        assert_eq!(m.apply(close(Some(1), 4)), Effect::None);
        assert!(m.open);
        assert_eq!(m.apply(close(Some(1), 5)), Effect::Flash);
        assert_eq!(m.selected, Some(1));
        // After closing, late messages of that generation do nothing.
        assert_eq!(m.apply(sel(Some(2), 5)), Effect::None);
        assert_eq!(m.apply(close(None, 5)), Effect::None);
        // A newer open works again; an open for an old generation stays refused.
        assert_eq!(m.apply(open(3)), Effect::None);
        assert_eq!(
            m.apply(open(5)),
            Effect::None,
            "closed generation stays closed"
        );
        assert_eq!(m.apply(open(6)), Effect::Open);
        assert_eq!(m.selected, None);
        assert_eq!(m.apply(close(None, 6)), Effect::Hide);
    }

    #[test]
    fn empty_ring_is_refused() {
        let mut m = Model::default();
        let c = OverlayCmd::Open {
            anchor_px: (0, 0),
            items: vec![],
            generation: 1,
        };
        assert_eq!(m.apply(c), Effect::None);
        assert!(!m.open);
    }
}
