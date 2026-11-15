//! Pure ring geometry: sector lookup, label positions, work-area clamping. No Win32.
//!
//! Angles are radians measured clockwise from "up" (screen -y). Slot 0 is centred on "up", slot
//! `i` on `i * 2π / n`, so every sector is `2π / n` wide and symmetric around its slot angle.

use std::f32::consts::TAU;

/// Ring radius at 96 DPI (outer edge of the sectors).
pub const RADIUS_96: f32 = 140.0;
/// Inner radius as a fraction of the outer one (the centre disc is the "cancel" zone).
pub const INNER_FRACTION: f32 = 0.34;
/// Transparent margin around the ring at 96 DPI.
pub const MARGIN_96: f32 = 6.0;
/// Gap between neighbouring sectors at 96 DPI.
pub const GAP_96: f32 = 3.0;

/// Screen rectangle in physical pixels (`right`/`bottom` exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Sizes of one ring in physical pixels for a given DPI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub n: usize,
    pub scale: f32,
    pub outer: f32,
    pub inner: f32,
    pub gap: f32,
    /// Side length of the (square) window.
    pub size: i32,
}

impl Layout {
    pub fn new(n: usize, dpi: u32) -> Self {
        let scale = dpi.max(48) as f32 / 96.0;
        let outer = RADIUS_96 * scale;
        let size = ((RADIUS_96 + MARGIN_96) * 2.0 * scale).ceil() as i32;
        Self {
            n: n.max(1),
            scale,
            outer,
            inner: outer * INNER_FRACTION,
            gap: GAP_96 * scale,
            size,
        }
    }

    /// Window-local centre of the ring.
    pub fn centre(&self) -> (f32, f32) {
        (self.size as f32 / 2.0, self.size as f32 / 2.0)
    }

    pub fn step(&self) -> f32 {
        TAU / self.n as f32
    }

    /// Window-local centre of slot `i`'s label (middle of the band).
    pub fn label_pos(&self, i: usize) -> (f32, f32) {
        let a = i as f32 * self.step();
        let r = (self.outer + self.inner) / 2.0;
        let (cx, cy) = self.centre();
        (cx + r * a.sin(), cy - r * a.cos())
    }
}

/// Largest slot count a ring can have.
pub const MAX_SLOTS: usize = 8;
/// Widest a slot's text box ever gets (at 96 DPI).
const BOX_MAX_96: f32 = 88.0;

/// Sizes of a slot's content at 96 DPI; they shrink as the ring gets more slots so that the
/// neighbouring texts never touch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotMetrics {
    /// Width of the text box (a chord of the label band minus the gaps).
    pub box_w: f32,
    pub label_px: f32,
    /// Em size of a glyph icon.
    pub icon_px: f32,
    /// Side of an app picture.
    pub image_px: f32,
}

/// The content sizes for a ring of `n` slots (clamped to `1..=MAX_SLOTS`).
pub fn slot_metrics(n: usize) -> SlotMetrics {
    let n = n.clamp(1, MAX_SLOTS);
    let r_mid = (RADIUS_96 + RADIUS_96 * INNER_FRACTION) / 2.0;
    let chord = if n < 2 {
        f32::MAX
    } else {
        2.0 * r_mid * (std::f32::consts::PI / n as f32).sin()
    };
    let box_w = (chord - GAP_96 - 4.0).min(BOX_MAX_96);
    let (label_px, icon_px) = match n {
        1..=4 => (14.0, 24.0),
        5 | 6 => (13.0, 22.0),
        7 => (12.0, 20.0),
        _ => (11.0, 18.0),
    };
    SlotMetrics {
        box_w,
        label_px,
        icon_px,
        image_px: icon_px + 4.0,
    }
}

impl SlotMetrics {
    /// Height of one text line.
    pub fn line_h(&self) -> f32 {
        self.label_px * 1.45
    }

    /// Vertical offsets (96 DPI px, from the slot's centre) of the picture's and the label's
    /// centres, given the picture's height. Without a picture the label sits on the centre.
    pub fn rows(&self, visual: Option<f32>) -> (f32, f32) {
        let line = self.line_h();
        match visual {
            None => (0.0, 0.0),
            Some(h) => {
                let block = h + 3.0 + line;
                (-block / 2.0 + h / 2.0, block / 2.0 - line / 2.0)
            }
        }
    }
}

/// Width of `c` in half-em units: wide (CJK, full-width) characters are 2, the rest 1.
fn text_units(c: char) -> usize {
    if (c as u32) < 0x2E80 { 1 } else { 2 }
}

/// `text` cut with an ellipsis so that it fits a box of `box_w` px at `label_px`. A cautious
/// estimate (a half-em is about 0.55 of the font size), so GDI's own end-ellipsis stays the
/// backstop rather than the rule.
pub fn fit_label(text: &str, box_w: f32, label_px: f32) -> String {
    let text = text.trim();
    let max_units = ((box_w / (label_px * 0.55)).floor() as usize).max(3);
    let total: usize = text.chars().map(text_units).sum();
    if total <= max_units {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let u = text_units(c);
        // One unit stays free for the ellipsis.
        if used + u + 1 > max_units {
            break;
        }
        out.push(c);
        used += u;
    }
    out.push('\u{2026}');
    out
}

/// Angle of the vector (dx, dy) (screen coordinates, y down), clockwise from up, in `[0, 2π)`.
pub fn angle_of(dx: f32, dy: f32) -> f32 {
    let a = dx.atan2(-dy);
    if a < 0.0 { a + TAU } else { a }
}

/// Which of `n` sectors the vector (dx, dy) points into.
pub fn sector_of(dx: f32, dy: f32, n: usize) -> usize {
    let n = n.max(1);
    let step = TAU / n as f32;
    (((angle_of(dx, dy) + step / 2.0) / step).floor() as usize) % n
}

/// Top-left of a `size` x `size` window centred as close to `anchor` as possible while staying
/// inside `work`. If the work area is smaller than the window, the window is centred in it.
pub fn clamp_origin(anchor: (i32, i32), size: i32, work: Area) -> (i32, i32) {
    let fit = |a: i32, lo: i32, hi: i32| -> i32 {
        if hi - lo <= size {
            lo + (hi - lo - size) / 2
        } else {
            (a - size / 2).clamp(lo, hi - size)
        }
    };
    (
        fit(anchor.0, work.left, work.right),
        fit(anchor.1, work.top, work.bottom),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sectors_eight() {
        // up, up-right, right, ... clockwise.
        assert_eq!(sector_of(0.0, -10.0, 8), 0);
        assert_eq!(sector_of(10.0, -10.0, 8), 1);
        assert_eq!(sector_of(10.0, 0.0, 8), 2);
        assert_eq!(sector_of(10.0, 10.0, 8), 3);
        assert_eq!(sector_of(0.0, 10.0, 8), 4);
        assert_eq!(sector_of(-10.0, 10.0, 8), 5);
        assert_eq!(sector_of(-10.0, 0.0, 8), 6);
        assert_eq!(sector_of(-10.0, -10.0, 8), 7);
    }

    #[test]
    fn sector_boundaries_and_other_counts() {
        // Boundary of slot 0 is at ±22.5°.
        let t = 22.0_f32.to_radians();
        assert_eq!(sector_of(t.sin(), -t.cos(), 8), 0);
        assert_eq!(sector_of(-t.sin(), -t.cos(), 8), 0);
        let t = 23.0_f32.to_radians();
        assert_eq!(sector_of(t.sin(), -t.cos(), 8), 1);
        assert_eq!(sector_of(-t.sin(), -t.cos(), 8), 7);
        assert_eq!(sector_of(5.0, 5.0, 1), 0);
        assert_eq!(sector_of(0.0, 10.0, 4), 2);
        assert_eq!(sector_of(10.0, 0.1, 6), 2);
    }

    #[test]
    fn label_positions() {
        let l = Layout::new(8, 96);
        let (cx, cy) = l.centre();
        let r = (l.outer + l.inner) / 2.0;
        let (x, y) = l.label_pos(0);
        assert!((x - cx).abs() < 0.01 && (y - (cy - r)).abs() < 0.01);
        let (x, y) = l.label_pos(2);
        assert!((x - (cx + r)).abs() < 0.01 && (y - cy).abs() < 0.01);
        let (x, y) = l.label_pos(4);
        assert!((x - cx).abs() < 0.01 && (y - (cy + r)).abs() < 0.01);
        for i in 0..8 {
            let (x, y) = l.label_pos(i);
            assert_eq!(sector_of(x - cx, y - cy, 8), i);
        }
    }

    #[test]
    fn layout_scales_with_dpi() {
        let a = Layout::new(8, 96);
        let b = Layout::new(8, 192);
        assert_eq!(a.size, 292);
        assert_eq!(b.size, 584);
        assert!((b.outer - 2.0 * a.outer).abs() < 0.01);
    }

    #[test]
    fn metrics_shrink_with_the_slot_count() {
        let mut prev = slot_metrics(1);
        for n in 2..=MAX_SLOTS {
            let m = slot_metrics(n);
            assert!(
                m.box_w <= prev.box_w && m.label_px <= prev.label_px,
                "n={n}"
            );
            assert!(m.icon_px <= prev.icon_px, "n={n}");
            prev = m;
        }
        // Neighbouring boxes never overlap: the box fits the chord at the label radius.
        let r_mid = (RADIUS_96 + RADIUS_96 * INNER_FRACTION) / 2.0;
        for n in 2..=MAX_SLOTS {
            let chord = 2.0 * r_mid * (std::f32::consts::PI / n as f32).sin();
            assert!(slot_metrics(n).box_w + GAP_96 <= chord, "n={n}");
        }
        assert_eq!(slot_metrics(0), slot_metrics(1));
        assert_eq!(slot_metrics(20), slot_metrics(MAX_SLOTS));
        assert!(slot_metrics(8).box_w > 50.0, "still readable");
    }

    #[test]
    fn every_count_lays_slots_out_evenly_and_apart() {
        for n in 1..=MAX_SLOTS {
            let l = Layout::new(n, 96);
            let (cx, cy) = l.centre();
            let r = (l.outer + l.inner) / 2.0;
            for i in 0..n {
                let (x, y) = l.label_pos(i);
                // Same radius, own sector, steps of 2π/n.
                assert!((((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs() < 0.01);
                assert_eq!(sector_of(x - cx, y - cy, n), i, "n={n} i={i}");
                if n > 1 {
                    let (x2, y2) = l.label_pos((i + 1) % n);
                    let d = ((x - x2).powi(2) + (y - y2).powi(2)).sqrt();
                    let want = 2.0 * r * (std::f32::consts::PI / n as f32).sin();
                    assert!((d - want).abs() < 0.05, "n={n}");
                }
            }
            // The first slot is straight up.
            let (x, y) = l.label_pos(0);
            assert!((x - cx).abs() < 0.01 && y < cy);
        }
    }

    #[test]
    fn rows_stack_picture_over_label() {
        let m = slot_metrics(6);
        assert_eq!(m.rows(None), (0.0, 0.0));
        let (i, t) = m.rows(Some(m.icon_px));
        assert!(i < 0.0 && t > 0.0);
        // The label starts below the picture's bottom edge.
        assert!(t - m.line_h() / 2.0 >= i + m.icon_px / 2.0);
        // The block is centred on the slot.
        assert!(((i - m.icon_px / 2.0) + (t + m.line_h() / 2.0)).abs() < 0.01);
    }

    #[test]
    fn labels_are_cut_to_fit() {
        let m = slot_metrics(8);
        // Short labels stay as they are (and are trimmed).
        assert_eq!(fit_label("  閉じる ", m.box_w, m.label_px), "閉じる");
        assert_eq!(fit_label("Copy", m.box_w, m.label_px), "Copy");
        // A long Japanese label gets an ellipsis and stays within the budget.
        let long = fit_label("スクリーンショットを撮る", m.box_w, m.label_px);
        assert!(long.ends_with('\u{2026}'), "{long}");
        assert!(long.chars().count() < "スクリーンショットを撮る".chars().count());
        let units: usize = long.chars().map(text_units).sum();
        assert!(units <= (m.box_w / (m.label_px * 0.55)) as usize, "{long}");
        // Latin text is cut later than Japanese of the same length.
        let latin = fit_label("Visual Studio Code Insiders", m.box_w, m.label_px);
        assert!(latin.ends_with('\u{2026}') && latin.chars().count() > long.chars().count());
        // Wider boxes keep more.
        let wide = slot_metrics(2);
        let kept = fit_label("スクリーンショットを撮る", wide.box_w, wide.label_px);
        assert!(kept.chars().count() > long.chars().count());
        // Never empty for real text, never absurdly short.
        assert_eq!(fit_label("", 60.0, 11.0), "");
        assert!(
            fit_label("あいうえおかきくけこ", 10.0, 20.0)
                .chars()
                .count()
                >= 2
        );
    }

    #[test]
    fn clamps_to_work_area() {
        let work = Area {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(clamp_origin((960, 500), 292, work), (960 - 146, 500 - 146));
        assert_eq!(clamp_origin((5, 5), 292, work), (0, 0));
        assert_eq!(
            clamp_origin((1919, 1039), 292, work),
            (1920 - 292, 1040 - 292)
        );
        // A monitor to the left (negative coordinates).
        let left = Area {
            left: -1280,
            top: 100,
            right: 0,
            bottom: 1024,
        };
        assert_eq!(clamp_origin((-2, 110), 292, left), (-292, 100));
        // Work area smaller than the window: centred.
        let tiny = Area {
            left: 0,
            top: 0,
            right: 200,
            bottom: 200,
        };
        assert_eq!(clamp_origin((10, 10), 292, tiny), (-46, -46));
    }
}
