//! Pure ring geometry: sector lookup, label positions, work-area clamping. No Win32.
//!
//! Angles are radians measured clockwise from "up" (screen -y). Slot 0 is centred on "up", slot
//! `i` on `i * 2π / n`, so every sector is `2π / n` wide and symmetric around its slot angle.

use std::f32::consts::{PI, TAU};

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

/// Pointer dot radius at 96 DPI.
pub const DOT_R_96: f32 = 3.0;
/// The hand's distance (mm) at which the pointer dot is fully visible (the engine's `ENTER_MM`).
pub const POINTER_FULL_MM: f32 = 5.0;
/// Time constant of the pointer dot's follow, in milliseconds.
pub const POINTER_TAU_MS: f32 = 40.0;

impl Layout {
    pub fn dot_r(&self) -> f32 {
        DOT_R_96 * self.scale
    }

    /// Window-local centre of the pointer dot for the direction `angle` (0 = up, clockwise): it
    /// sits on the centre disc's rim side, just inside the gap.
    pub fn dot_centre(&self, angle: f32) -> (f32, f32) {
        let r = self.inner - self.gap - self.dot_r();
        let (cx, cy) = self.centre();
        (cx + r * angle.sin(), cy - r * angle.cos())
    }
}

/// Shortest signed angle from `from` to `to`, in `(-π, π]`.
pub fn angle_diff(from: f32, to: f32) -> f32 {
    let d = (to - from).rem_euclid(TAU);
    if d > PI { d - TAU } else { d }
}

/// `from` moved `t` (0..=1) of the way to `to` along the shorter arc, in `[0, 2π)`.
pub fn lerp_angle(from: f32, to: f32, t: f32) -> f32 {
    (from + angle_diff(from, to) * t).rem_euclid(TAU)
}

/// How visible the pointer dot is for a hand `dist_mm` from the anchor.
pub fn pointer_alpha(dist_mm: f32) -> f32 {
    (dist_mm / POINTER_FULL_MM).clamp(0.0, 1.0)
}

/// The pointer dot's direction, following the hand's with a short exponential lag.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerFollow {
    pub angle: f32,
    pub target: f32,
}

impl PointerFollow {
    /// Aim at the hand offset (`dx`, `dy`); near the centre the direction is meaningless, so the
    /// old aim stays.
    pub fn aim(&mut self, dx: f32, dy: f32) {
        if dx.hypot(dy) >= 0.3 {
            self.target = angle_of(dx, dy);
        }
    }

    /// Jump to the aim.
    pub fn snap(&mut self) {
        self.angle = self.target;
    }

    pub fn settled(&self) -> bool {
        angle_diff(self.angle, self.target).abs() < 0.002
    }

    /// Advance by `dt_ms` with time constant `tau_ms` (frame-rate independent). True once settled.
    pub fn step(&mut self, dt_ms: f32, tau_ms: f32) -> bool {
        let k = 1.0 - (-dt_ms.max(0.0) / tau_ms.max(1.0)).exp();
        let d = angle_diff(self.angle, self.target);
        if d.abs() < 0.002 {
            self.angle = self.target;
            return true;
        }
        self.angle = (self.angle + d * k).rem_euclid(TAU);
        self.settled()
    }
}

/// Ease-out (cubic) of `t` in 0..=1.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// The selection band's animation: it slides along the shorter arc from slot to slot, and fades
/// in or out in place otherwise. The centre disc's light fades alongside. Time only advances
/// through `step`, so it is deterministic.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BandAnim {
    /// Centre of the band (0 = up, clockwise).
    pub angle: f32,
    /// 0 = invisible .. 1 = fully selected.
    pub strength: f32,
    /// How much the centre disc is lit.
    pub centre: f32,
    strength_to: f32,
    centre_to: f32,
    /// (from, to, elapsed ms) of a slide in flight.
    slide: Option<(f32, f32, f32)>,
}

fn approach(v: f32, to: f32, d: f32) -> f32 {
    if (to - v).abs() <= d {
        to
    } else {
        v + d * (to - v).signum()
    }
}

impl BandAnim {
    /// Aim at the model's selection (`slot`, or the centre disc if `centre`), `n` slots in all.
    /// Without `animate` everything jumps.
    pub fn aim(&mut self, slot: Option<usize>, centre: bool, n: usize, animate: bool) {
        self.centre_to = if slot.is_none() && centre { 1.0 } else { 0.0 };
        match slot.filter(|&i| i < n) {
            Some(i) => {
                let to = i as f32 * TAU / n as f32;
                if animate && self.strength > 0.5 && angle_diff(self.angle, to).abs() > 1e-4 {
                    self.slide = Some((self.angle, to, 0.0));
                } else {
                    self.angle = to;
                    self.slide = None;
                }
                self.strength_to = 1.0;
            }
            None => self.strength_to = 0.0,
        }
        if !animate {
            self.finish();
        }
    }

    /// Jump to the aim.
    pub fn finish(&mut self) {
        if let Some((_, to, _)) = self.slide.take() {
            self.angle = to;
        }
        self.strength = self.strength_to;
        self.centre = self.centre_to;
    }

    pub fn settled(&self) -> bool {
        self.slide.is_none() && self.strength == self.strength_to && self.centre == self.centre_to
    }

    /// Advance by `dt_ms`: the slide takes `slide_ms`, the fades `fade_ms`.
    pub fn step(&mut self, dt_ms: f32, slide_ms: f32, fade_ms: f32) {
        let dt = dt_ms.max(0.0);
        if let Some((from, to, el)) = self.slide {
            let el = el + dt;
            let t = (el / slide_ms.max(1.0)).min(1.0);
            if t >= 1.0 {
                self.angle = to;
                self.slide = None;
            } else {
                self.angle = lerp_angle(from, to, ease_out(t));
                self.slide = Some((from, to, el));
            }
        }
        let d = dt / fade_ms.max(1.0);
        self.strength = approach(self.strength, self.strength_to, d);
        self.centre = approach(self.centre, self.centre_to, d);
    }
}

/// The slot nearest to the band centre while the band is mostly on (its label turns to the
/// selected colour).
pub fn lit_slot(angle: f32, strength: f32, n: usize) -> Option<usize> {
    if n == 0 || strength < 0.5 {
        return None;
    }
    let step = TAU / n as f32;
    Some((((angle + step / 2.0) / step).floor() as usize) % n)
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
    fn slide_takes_the_shorter_arc() {
        // Slot 0 to slot 2 of 8: the midpoint lies at slot 1's angle.
        let step8 = TAU / 8.0;
        let mid = lerp_angle(0.0, 2.0 * step8, 0.5);
        assert!((mid - step8).abs() < 1e-4, "{mid}");
        // Slot 0 to slot 7 of 8 goes backwards through 360 to 315 degrees.
        let to = 7.0 * step8;
        let q = lerp_angle(0.0, to, 0.5);
        assert!((q - 337.5f32.to_radians()).abs() < 1e-4, "{q}");
        assert!((lerp_angle(0.0, to, 1.0) - 315f32.to_radians()).abs() < 1e-4);
        assert!(angle_diff(0.0, to) < 0.0);
    }

    #[test]
    fn pointer_dot_sits_on_the_rim_side() {
        let l = Layout::new(8, 96);
        let (cx, cy) = l.centre();
        let r = l.inner - l.gap - l.dot_r();
        for a in [0.0f32, 1.0, 2.5, 4.0, 5.9] {
            let (x, y) = l.dot_centre(a);
            assert!(((x - cx).hypot(y - cy) - r).abs() < 1e-3);
            assert!((angle_of(x - cx, y - cy) - a).abs() < 1e-3, "angle {a}");
        }
        // 0 = up, 90 degrees = right.
        let (x, y) = l.dot_centre(0.0);
        assert!(y < cy && (x - cx).abs() < 1e-3);
        let (x, y) = l.dot_centre(PI / 2.0);
        assert!(x > cx && (y - cy).abs() < 1e-3);
    }

    #[test]
    fn pointer_follow_is_frame_rate_independent() {
        let mut a = PointerFollow::default();
        let mut b = a;
        a.aim(10.0, 0.0);
        b.aim(10.0, 0.0);
        a.step(16.0, 40.0);
        a.step(16.0, 40.0);
        b.step(32.0, 40.0);
        assert!((a.angle - b.angle).abs() < 1e-4);
        // It goes the short way round (from 350 to 10 degrees through 0).
        let mut p = PointerFollow {
            angle: 350f32.to_radians(),
            target: 10f32.to_radians(),
        };
        p.step(10.0, 40.0);
        assert!(p.angle > 350f32.to_radians() || p.angle < 10f32.to_radians());
        for _ in 0..100 {
            p.step(16.0, 40.0);
        }
        assert!(p.settled());
        // Invisible at the centre, full at 5 mm.
        assert_eq!(pointer_alpha(0.0), 0.0);
        assert_eq!(pointer_alpha(9.0), 1.0);
    }

    #[test]
    fn band_slides_the_short_way_and_fades_in_place() {
        let step = TAU / 8.0;
        // Nothing -> slot 0: it appears in place.
        let mut b = BandAnim::default();
        b.aim(Some(0), false, 8, true);
        assert_eq!((b.angle, b.strength), (0.0, 0.0));
        for _ in 0..10 {
            b.step(16.0, 140.0, 100.0);
        }
        assert_eq!(b.strength, 1.0);
        // Slot 0 -> 2: half way in time is past slot 1 (ease-out), and it ends on slot 2.
        b.aim(Some(2), false, 8, true);
        b.step(70.0, 140.0, 100.0);
        assert!(b.angle > step && b.angle < 2.0 * step);
        assert_eq!(b.strength, 1.0);
        b.step(100.0, 140.0, 100.0);
        assert!((b.angle - 2.0 * step).abs() < 1e-5 && b.settled());
        // Slot 2 -> 7 of 8 wraps through 360: the angle stays close to 0 / 315 on the way.
        b.aim(Some(0), false, 8, false);
        b.aim(Some(7), false, 8, true);
        b.step(40.0, 140.0, 100.0);
        assert!(b.angle > 7.0 * step || b.angle < 0.2, "{}", b.angle);
        b.step(200.0, 140.0, 100.0);
        assert!((b.angle - 7.0 * step).abs() < 1e-5);
        // Slot -> centre: the band fades out where it is, the disc lights up.
        let at = b.angle;
        b.aim(None, true, 8, true);
        b.step(50.0, 140.0, 100.0);
        assert_eq!(b.angle, at);
        assert!((b.strength - 0.5).abs() < 1e-5 && (b.centre - 0.5).abs() < 1e-5);
        b.step(100.0, 140.0, 100.0);
        assert!(b.settled() && b.strength == 0.0 && b.centre == 1.0);
        // Animations off: jumps.
        b.aim(Some(3), false, 8, false);
        assert!(b.settled() && b.strength == 1.0 && b.centre == 0.0);
        assert!((b.angle - 3.0 * step).abs() < 1e-5);
        // Labels light at the nearest slot once the band is mostly on.
        assert_eq!(lit_slot(1.4 * step, 1.0, 8), Some(1));
        assert_eq!(lit_slot(1.6 * step, 1.0, 8), Some(2));
        assert_eq!(lit_slot(1.6 * step, 0.4, 8), None);
        assert_eq!(lit_slot(7.4 * step, 1.0, 8), Some(7));
        assert_eq!(lit_slot(7.6 * step, 1.0, 8), Some(0));
    }

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
