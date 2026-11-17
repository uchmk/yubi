//! Glyph codepoints from "Segoe Fluent Icons" (SegoeIcons.ttf) or "Segoe MDL2 Assets" (segmdl2.ttf).
//! All of these exist in both fonts (checked against their cmap tables).

pub const HOME: char = '\u{E80F}';
pub const SETTINGS: char = '\u{E713}';
pub const TOUCHPAD: char = '\u{EFA5}';
pub const MENU: char = '\u{E700}';
pub const SEARCH: char = '\u{E721}';
pub const CHEVRON_DOWN: char = '\u{E70D}';
pub const CHEVRON_UP: char = '\u{E70E}';
pub const CHEVRON_RIGHT: char = '\u{E76C}';
pub const DELETE: char = '\u{E74D}';
pub const FOLDER: char = '\u{E8B7}';
pub const DRAG: char = '\u{E7C9}';
pub const FOUR_FINGER: char = '\u{E815}';
pub const PRESS: char = '\u{E8AD}';
pub const LONG_HOLD: char = '\u{E81C}';
pub const FINGER_HOLD: char = '\u{E8D4}';
pub const THUMB_HOLD: char = '\u{E8BE}';
pub const CORNER: char = '\u{E8A7}';
pub const EDGE: char = '\u{E9E9}';
pub const WINDOW: char = '\u{E8A1}';
pub const RING: char = '\u{E7C4}';
pub const APPS: char = '\u{E8FD}';
pub const SENSITIVITY: char = '\u{EC4A}';
pub const HAPTIC: char = '\u{E765}';
pub const MONITOR: char = '\u{E7F4}';
pub const DIAGNOSTICS: char = '\u{E9CE}';
pub const LOG: char = '\u{E8A5}';

/// Finger-count marks: 3, 4 and 5 fingertip dots on a gentle arc. Segoe has no such glyphs, so
/// these private-use codepoints are drawn by `widgets::paint_icon`, not taken from the icon font.
pub const FINGERS_3: char = '\u{F8F3}';
pub const FINGERS_4: char = '\u{F8F4}';
pub const FINGERS_5: char = '\u{F8F5}';

/// The number of fingertip dots when `glyph` is one of the drawn finger-count marks.
pub fn finger_count(glyph: char) -> Option<u8> {
    match glyph {
        FINGERS_3 => Some(3),
        FINGERS_4 => Some(4),
        FINGERS_5 => Some(5),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_finger_marks_are_distinct_and_recognized() {
        assert_eq!(finger_count(FINGERS_3), Some(3));
        assert_eq!(finger_count(FINGERS_4), Some(4));
        assert_eq!(finger_count(FINGERS_5), Some(5));
        assert_eq!(finger_count(DRAG), None);
        assert_eq!(finger_count(FOUR_FINGER), None);
    }
}
