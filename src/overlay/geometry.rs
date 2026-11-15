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
