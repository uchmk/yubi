//! Pure overlay state: which ring is open, what is selected, which generation owns it.
//! Stale generations are ignored here, so the window code stays free of that logic.

use std::sync::Arc;

use super::level::LevelInfo;

/// A small square picture (an app's own icon): premultiplied BGRA, `side` x `side`, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingImage {
    pub side: u32,
    pub px: Arc<Vec<u32>>,
}

/// One ring slot: a Japanese label, an optional Segoe Fluent Icons glyph and an optional picture
/// (the picture wins over the glyph).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingSlot {
    pub label: String,
    pub icon: Option<char>,
    pub image: Option<RingImage>,
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
    /// Show or update the level HUD (volume / brightness) at the bottom centre of the monitor
    /// under the cursor. It hides itself shortly after the last update. Ignored while a ring is open.
    Level(LevelInfo),
    /// Hide the level HUD now.
    LevelHide,
    /// Show a one-line notice card (pause / resume) for about 3 s, then fade it. Ignored while a
    /// ring is open.
    Notice(String),
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
    /// The hand went back to the centre after pointing at a slot: the cancel disc lights up.
    pub centre: bool,
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
                self.centre = false;
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
                // Only a return to the centre counts; a ring that just opened has not been left yet.
                self.centre = index.is_none();
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
            // The level HUD has its own state in the window code.
            OverlayCmd::Level(_) | OverlayCmd::LevelHide | OverlayCmd::Notice(_) => Effect::None,
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
                image: None,
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
    fn centre_lights_up_only_after_leaving_it() {
        let mut m = Model::default();
        m.apply(open(1));
        assert!(!m.centre, "a fresh ring is not 'back in the centre'");
        let sel = |index| OverlayCmd::Select {
            index,
            generation: 1,
        };
        assert_eq!(m.apply(sel(None)), Effect::None);
        assert!(!m.centre);
        m.apply(sel(Some(3)));
        assert!(!m.centre);
        assert_eq!(m.apply(sel(None)), Effect::Redraw);
        assert!(m.centre);
        m.apply(sel(Some(0)));
        assert!(!m.centre);
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
