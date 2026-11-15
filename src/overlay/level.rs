//! The level HUD (volume / brightness bar): pure state and geometry. No Win32.
//!
//! A level is shown while the user changes volume or brightness and hides itself a short time
//! after the last change. Time is passed in as milliseconds so the logic is testable.

use super::geometry::Area;
use super::model::Effect;

/// How long the HUD stays after the last update.
pub const HOLD_MS: u64 = 1200;
/// How long a notice card (pause / resume) stays before it fades.
pub const NOTICE_HOLD_MS: u64 = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelKind {
    Volume,
    Brightness,
}

/// What one HUD shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelInfo {
    pub kind: LevelKind,
    /// 0..=100. `None` when the value is unknown (a brightness that cannot be read).
    pub percent: Option<u8>,
    pub muted: bool,
}

impl LevelInfo {
    pub fn label(&self) -> &'static str {
        match (self.kind, self.muted) {
            (LevelKind::Volume, true) => "ミュート",
            (LevelKind::Volume, false) => "音量",
            (LevelKind::Brightness, _) => "明るさ",
        }
    }

    /// Segoe Fluent Icons glyph.
    pub fn icon(&self) -> char {
        match (self.kind, self.muted) {
            (LevelKind::Volume, true) => '\u{E74F}',
            (LevelKind::Volume, false) => '\u{E767}',
            (LevelKind::Brightness, _) => '\u{E706}',
        }
    }

    /// The number text (`None` = show no number).
    pub fn number(&self) -> Option<String> {
        self.percent.map(|p| p.min(100).to_string())
    }

    /// The bar fill, 0.0..=1.0 (an unknown value draws an empty bar).
    pub fn fraction(&self) -> f32 {
        self.percent.map_or(0.0, |p| f32::from(p.min(100)) / 100.0)
    }
}

#[derive(Debug, Default)]
pub struct LevelModel {
    info: Option<LevelInfo>,
    updated_ms: u64,
}

impl LevelModel {
    /// Show or update the HUD. `Open` when it was hidden, `Redraw` when it was already up.
    pub fn show(&mut self, info: LevelInfo, now_ms: u64) -> Effect {
        let fresh = self.info.is_none();
        self.info = Some(info);
        self.updated_ms = now_ms;
        if fresh { Effect::Open } else { Effect::Redraw }
    }

    /// Hide now. True if it was showing.
    pub fn hide(&mut self) -> bool {
        self.info.take().is_some()
    }

    pub fn info(&self) -> Option<LevelInfo> {
        self.info
    }

    pub fn visible(&self) -> bool {
        self.info.is_some()
    }

    /// True when it is showing and `HOLD_MS` passed since the last update.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.info.is_some() && now_ms.saturating_sub(self.updated_ms) >= HOLD_MS
    }
}

/// A one-line notice card (same card, no bar). Time is passed in, like `LevelModel`.
#[derive(Debug, Default)]
pub struct NoticeModel {
    text: Option<String>,
    shown_ms: u64,
}

impl NoticeModel {
    /// Show `text` (replacing a notice that is up) and restart the hold.
    pub fn show(&mut self, text: String, now_ms: u64) {
        self.text = Some(text);
        self.shown_ms = now_ms;
    }

    pub fn hide(&mut self) -> bool {
        self.text.take().is_some()
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// True when it is showing and `NOTICE_HOLD_MS` passed since it was shown.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.text.is_some() && now_ms.saturating_sub(self.shown_ms) >= NOTICE_HOLD_MS
    }
}

/// Window-local rectangle `(x0, y0, x1, y1)` in pixels.
pub type Rect4 = (i32, i32, i32, i32);

/// Sizes of the HUD in physical pixels for a DPI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelLayout {
    pub scale: f32,
    pub width: i32,
    pub height: i32,
    /// Corner radius of the card.
    pub radius: f32,
}

const WIDTH_96: f32 = 280.0;
const HEIGHT_96: f32 = 64.0;
const RADIUS_96: f32 = 8.0;
const PAD_96: f32 = 16.0;
/// Gap between the HUD and the bottom of the work area.
const BOTTOM_96: f32 = 40.0;

impl LevelLayout {
    pub fn new(dpi: u32) -> Self {
        let scale = dpi.max(48) as f32 / 96.0;
        Self {
            scale,
            width: (WIDTH_96 * scale).ceil() as i32,
            height: (HEIGHT_96 * scale).ceil() as i32,
            radius: RADIUS_96 * scale,
        }
    }

    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    /// The bar track `(x0, y0, x1, y1)`.
    pub fn bar(&self) -> (f32, f32, f32, f32) {
        let pad = PAD_96 * self.scale;
        (
            pad,
            42.0 * self.scale,
            self.width as f32 - pad,
            48.0 * self.scale,
        )
    }

    pub fn icon_rect(&self) -> Rect4 {
        (self.px(16.0), self.px(10.0), self.px(38.0), self.px(34.0))
    }

    pub fn label_rect(&self) -> Rect4 {
        (self.px(44.0), self.px(10.0), self.px(180.0), self.px(34.0))
    }

    /// The text area of a notice card (the whole card inside its padding).
    pub fn notice_rect(&self) -> Rect4 {
        (
            self.px(PAD_96),
            self.px(8.0),
            self.width - self.px(PAD_96),
            self.height - self.px(8.0),
        )
    }

    pub fn number_rect(&self) -> Rect4 {
        (
            self.px(190.0),
            self.px(10.0),
            self.width - self.px(PAD_96),
            self.px(34.0),
        )
    }

    /// Top-left of the window: bottom centre of `work`, like the Windows flyout.
    pub fn origin(&self, work: Area) -> (i32, i32) {
        let x = work.left + (work.right - work.left - self.width) / 2;
        let y = work.bottom - self.height - self.px(BOTTOM_96);
        (x, y.max(work.top))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(p: u8) -> LevelInfo {
        LevelInfo {
            kind: LevelKind::Volume,
            percent: Some(p),
            muted: false,
        }
    }

    #[test]
    fn show_update_hide_timing() {
        let mut m = LevelModel::default();
        assert!(!m.visible());
        assert!(!m.expired(10_000));
        assert_eq!(m.show(vol(40), 1000), Effect::Open);
        assert_eq!(m.show(vol(42), 1500), Effect::Redraw);
        // The hold counts from the last update, not the first.
        assert!(!m.expired(1500 + HOLD_MS - 1));
        assert!(m.expired(1500 + HOLD_MS));
        assert_eq!(m.info(), Some(vol(42)));
        assert!(m.hide());
        assert!(!m.hide());
        assert!(!m.expired(100_000));
        assert_eq!(m.show(vol(1), 200_000), Effect::Open);
    }

    #[test]
    fn notice_holds_three_seconds() {
        let mut n = NoticeModel::default();
        assert!(!n.expired(100_000));
        n.show("a".into(), 1000);
        assert!(!n.expired(1000 + NOTICE_HOLD_MS - 1));
        assert!(n.expired(1000 + NOTICE_HOLD_MS));
        // A newer notice restarts the hold.
        n.show("b".into(), 3000);
        assert_eq!(n.text(), Some("b"));
        assert!(!n.expired(1000 + NOTICE_HOLD_MS));
        assert!(n.hide());
        assert!(!n.hide());
        assert!(!n.expired(1_000_000));
    }

    #[test]
    fn labels_follow_state() {
        let mut i = vol(30);
        assert_eq!(i.label(), "音量");
        assert_eq!(i.number().as_deref(), Some("30"));
        i.muted = true;
        assert_eq!(i.label(), "ミュート");
        assert_eq!(i.icon(), '\u{E74F}');
        let b = LevelInfo {
            kind: LevelKind::Brightness,
            percent: None,
            muted: false,
        };
        assert_eq!(b.label(), "明るさ");
        assert_eq!(b.number(), None);
        assert_eq!(b.fraction(), 0.0);
        assert_eq!(vol(200).number().as_deref(), Some("100"));
        assert_eq!(vol(200).fraction(), 1.0);
    }

    #[test]
    fn sits_at_bottom_centre() {
        let l = LevelLayout::new(96);
        let work = Area {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let (x, y) = l.origin(work);
        assert_eq!(x, (1920 - l.width) / 2);
        assert!(y + l.height < work.bottom && y > work.bottom / 2);
        // A tiny work area still keeps the HUD inside from the top.
        let small = Area {
            left: 0,
            top: 0,
            right: 400,
            bottom: 50,
        };
        assert_eq!(l.origin(small).1, 0);
        assert!(LevelLayout::new(192).width > l.width);
    }
}
