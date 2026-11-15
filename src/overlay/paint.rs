//! Software rasteriser for the ring (anti-aliased, premultiplied BGRA `u32` pixels). Pure: no Win32.
//! Text is drawn by `window.rs` with GDI and composited here through `blend_text`.

use std::f32::consts::{PI, TAU};

use super::geometry::{Layout, angle_of};
use super::level::{LevelInfo, LevelLayout};

/// Straight (non-premultiplied) RGBA, 0..=255 floats.
#[derive(Clone, Copy, Debug)]
pub struct Rgba(pub f32, pub f32, pub f32, pub f32);

impl Rgba {
    const fn hex(rgb: u32, a: f32) -> Self {
        Rgba(
            ((rgb >> 16) & 0xFF) as f32,
            ((rgb >> 8) & 0xFF) as f32,
            (rgb & 0xFF) as f32,
            a,
        )
    }

    fn lerp(self, o: Rgba, t: f32) -> Rgba {
        let t = t.clamp(0.0, 1.0);
        let m = |a: f32, b: f32| a + (b - a) * t;
        Rgba(
            m(self.0, o.0),
            m(self.1, o.1),
            m(self.2, o.2),
            m(self.3, o.3),
        )
    }
}

/// Colors mirror `Tokens::dark()` in `src/theme.rs` (`card`, `accent`, `on_accent`, `text`); the
/// overlay is self-contained so that a stand-alone binary can include it.
pub struct Palette {
    pub fill: Rgba,
    pub border: Rgba,
    pub sel_fill: Rgba,
    pub sel_border: Rgba,
    pub flash_fill: Rgba,
    pub text: Rgba,
    pub text_sel: Rgba,
    pub text_dim: Rgba,
    pub track: Rgba,
    pub bar: Rgba,
    pub bar_muted: Rgba,
}

pub const PALETTE: Palette = Palette {
    // card 0x161616, slightly darker and translucent.
    fill: Rgba::hex(0x161616, 0.90),
    border: Rgba::hex(0xFFFFFF, 0.20),
    // accent 0x60CDFF.
    sel_fill: Rgba::hex(0x60CDFF, 0.96),
    sel_border: Rgba::hex(0xBFEAFF, 0.90),
    flash_fill: Rgba::hex(0xE6F7FF, 1.0),
    text: Rgba::hex(0xFFFFFF, 1.0),
    // on_accent 0x000000.
    text_sel: Rgba::hex(0x000000, 1.0),
    text_dim: Rgba::hex(0xC8C8C8, 1.0),
    track: Rgba::hex(0x484848, 0.95),
    // accent 0x60CDFF; a muted volume is grey.
    bar: Rgba::hex(0x60CDFF, 1.0),
    bar_muted: Rgba::hex(0x8A8A8A, 1.0),
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlight {
    None,
    Selected(usize),
    /// The committed sector, flashed brighter.
    Flash(usize),
}

fn premul(c: Rgba) -> u32 {
    let a = c.3.clamp(0.0, 1.0);
    let ch = |v: f32| (v * a).round().clamp(0.0, 255.0) as u32;
    ((a * 255.0).round() as u32) << 24 | ch(c.0) << 16 | ch(c.1) << 8 | ch(c.2)
}

/// Draw the ring into `px` (`size` x `size`, cleared first).
pub fn render_ring(px: &mut [u32], l: &Layout, hl: Highlight) {
    let size = l.size as usize;
    assert!(px.len() >= size * size);
    px[..size * size].fill(0);
    let (cx, cy) = l.centre();
    let step = l.step();
    let n = l.n;
    let hl_index = match hl {
        Highlight::None => None,
        Highlight::Selected(i) | Highlight::Flash(i) => Some(i),
    };
    let disc_r = l.inner - l.gap * 0.5;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            if r > l.outer + 1.0 {
                continue;
            }
            // Centre disc ("cancel" zone).
            if r < l.inner {
                let d = disc_r - r;
                if d > -1.0 {
                    px[y * size + x] = shade(PALETTE.fill, PALETTE.border, d);
                }
                continue;
            }
            let a = angle_of(dx, dy);
            let idx = (((a + step / 2.0) / step).floor() as usize) % n;
            // Distance to the nearest sector boundary (none for a single slot).
            let bd = if n < 2 {
                f32::MAX
            } else {
                let mut off = a - idx as f32 * step;
                if off > PI {
                    off -= TAU;
                } else if off < -PI {
                    off += TAU;
                }
                let to_edge = (step / 2.0 - off.abs()).max(0.0);
                r * to_edge.sin()
            };
            let d = (l.outer - r).min(r - l.inner).min(bd - l.gap * 0.5);
            if d <= -1.0 {
                continue;
            }
            let (fill, border) = match (hl_index == Some(idx), hl) {
                (true, Highlight::Flash(_)) => (PALETTE.flash_fill, PALETTE.sel_border),
                (true, _) => (PALETTE.sel_fill, PALETTE.sel_border),
                _ => (PALETTE.fill, PALETTE.border),
            };
            px[y * size + x] = shade(fill, border, d);
        }
    }
}

/// Signed distance (positive inside) to a rounded rectangle `(x0, y0, x1, y1)` with corner
/// radius `r`, at the point `(px, py)`.
fn rrect_dist(px: f32, py: f32, rect: (f32, f32, f32, f32), r: f32) -> f32 {
    let (x0, y0, x1, y1) = rect;
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hx, hy) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let r = r.min(hx).min(hy).max(0.0);
    let qx = (px - cx).abs() - (hx - r);
    let qy = (py - cy).abs() - (hy - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    -(outside + qx.max(qy).min(0.0) - r)
}

/// Draw the level HUD card and bar into `px` (`stride` pixels per row, cleared first). The
/// text (icon, label, number) is drawn on top by `window.rs`.
pub fn render_level(px: &mut [u32], stride: usize, l: &LevelLayout, info: &LevelInfo) {
    render_card(px, stride, l, Some(info));
}

/// Draw the notice card (the HUD card without a bar); the text is drawn on top by `window.rs`.
pub fn render_notice(px: &mut [u32], stride: usize, l: &LevelLayout) {
    render_card(px, stride, l, None);
}

fn render_card(px: &mut [u32], stride: usize, l: &LevelLayout, info: Option<&LevelInfo>) {
    let (w, h) = (l.width as usize, l.height as usize);
    assert!(stride >= w && px.len() >= stride * h);
    for y in 0..h {
        px[y * stride..y * stride + stride].fill(0);
    }
    let card = (0.0, 0.0, l.width as f32, l.height as f32);
    let (bx0, by0, bx1, by1) = l.bar();
    let track = (bx0, by0, bx1, by1);
    let bar_r = (by1 - by0) / 2.0;
    let fill_w = info.map_or(0.0, |i| (bx1 - bx0) * i.fraction());
    let fill = (bx0, by0, bx0 + fill_w, by1);
    let fill_color = if info.is_some_and(|i| i.muted) {
        PALETTE.bar_muted
    } else {
        PALETTE.bar
    };
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = rrect_dist(fx, fy, card, l.radius);
            if d <= -1.0 {
                continue;
            }
            // Card with its 1 px border, then the track and the fill by coverage.
            let t = (d - 0.5).clamp(0.0, 1.0);
            let mut c = PALETTE.border.lerp(PALETTE.fill, t);
            let cov_track = (rrect_dist(fx, fy, track, bar_r) + 0.5).clamp(0.0, 1.0);
            if info.is_some() {
                c = c.lerp(PALETTE.track, cov_track);
            }
            if fill_w >= 1.0 {
                let cov_fill = (rrect_dist(fx, fy, fill, bar_r) + 0.5).clamp(0.0, 1.0);
                c = c.lerp(fill_color, cov_fill);
            }
            c.3 *= (d + 0.5).clamp(0.0, 1.0);
            px[y * stride + x] = premul(c);
        }
    }
}

/// Edge-aware color: `d` is the signed distance inside the shape in pixels. Within ~1 px of the
/// edge the border color shows; the outermost half pixel fades out for anti-aliasing.
fn shade(fill: Rgba, border: Rgba, d: f32) -> u32 {
    let cov = (d + 0.5).clamp(0.0, 1.0);
    let t = (d - 0.5).clamp(0.0, 1.0);
    let mut c = border.lerp(fill, t);
    c.3 *= cov;
    premul(c)
}

/// Composite `coverage` (0..=255, from GDI glyph rendering) of an opaque `color` over `dst`.
pub fn blend_text(dst: &mut u32, coverage: u8, color: Rgba) {
    if coverage == 0 {
        return;
    }
    let c = coverage as f32 / 255.0;
    let d = *dst;
    let ch = |shift: u32, v: f32| {
        let old = ((d >> shift) & 0xFF) as f32;
        (old * (1.0 - c) + v * c).round().clamp(0.0, 255.0) as u32
    };
    *dst = ch(24, 255.0) << 24 | ch(16, color.0) << 16 | ch(8, color.1) << 8 | ch(0, color.2);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(px: &[u32], l: &Layout, dx: f32, dy: f32) -> u32 {
        let (cx, cy) = l.centre();
        px[(cy + dy) as usize * l.size as usize + (cx + dx) as usize]
    }

    #[test]
    fn ring_draws_only_inside_the_ring() {
        let l = Layout::new(8, 96);
        let mut px = vec![0u32; (l.size * l.size) as usize];
        render_ring(&mut px, &l, Highlight::Selected(2));
        let mid = (l.outer + l.inner) / 2.0;
        // Corner of the window and just outside the ring stay transparent.
        assert_eq!(px[0], 0);
        assert_eq!(pixel(&px, &l, l.outer + 3.0, 0.0), 0);
        // Selected sector (right, slot 2) is accent coloured and opaque-ish: blue > red.
        let s = pixel(&px, &l, mid, 0.0);
        assert!(s >> 24 > 230);
        assert!(
            (s & 0xFF) > ((s >> 16) & 0xFF),
            "accent is blue-heavy: {s:08x}"
        );
        // An unselected sector is dark and translucent.
        let u = pixel(&px, &l, -mid, 0.0);
        assert!((u >> 24) > 200 && (u >> 24) < 250);
        assert!((u & 0xFF) < 0x40);
        // The gap between slot 0 and slot 1 is empty (boundary at 22.5°).
        let a = 22.5f32.to_radians();
        assert_eq!(pixel(&px, &l, mid * a.sin(), -mid * a.cos()) >> 24, 0);
        // Centre disc is drawn.
        assert!(pixel(&px, &l, 0.0, 0.0) >> 24 > 200);
    }

    #[test]
    fn flash_is_brighter_than_selection() {
        let l = Layout::new(8, 96);
        let mut a = vec![0u32; (l.size * l.size) as usize];
        let mut b = a.clone();
        render_ring(&mut a, &l, Highlight::Selected(0));
        render_ring(&mut b, &l, Highlight::Flash(0));
        let mid = (l.outer + l.inner) / 2.0;
        let red = |v: u32| (v >> 16) & 0xFF;
        assert!(red(pixel(&b, &l, 0.0, -mid)) > red(pixel(&a, &l, 0.0, -mid)));
    }

    #[test]
    fn level_bar_follows_the_value() {
        use super::super::level::{LevelInfo, LevelKind};
        let l = LevelLayout::new(96);
        let stride = l.width as usize;
        let mut px = vec![0u32; stride * l.height as usize];
        let info = |p| LevelInfo {
            kind: LevelKind::Volume,
            percent: Some(p),
            muted: false,
        };
        render_level(&mut px, stride, &l, &info(50));
        let (bx0, by0, bx1, by1) = l.bar();
        let at = |px: &[u32], fx: f32| px[((by0 + by1) / 2.0) as usize * stride + fx as usize];
        let quarter = bx0 + (bx1 - bx0) * 0.25;
        let three_quarters = bx0 + (bx1 - bx0) * 0.75;
        // Filled part is accent blue-heavy, the rest is the grey track.
        let f = at(&px, quarter);
        assert!((f & 0xFF) > 0xE0 && ((f >> 16) & 0xFF) < 0xA0, "{f:08x}");
        let t = at(&px, three_quarters);
        assert!((t & 0xFF) < 0x60, "{t:08x}");
        // The window corner is transparent, the card inside is drawn.
        assert_eq!(px[0], 0);
        assert!(px[stride * 20 + 100] >> 24 > 200);
        // Muted: the fill turns grey.
        let mut m = info(50);
        m.muted = true;
        render_level(&mut px, stride, &l, &m);
        let g = at(&px, quarter);
        assert!((g & 0xFF).abs_diff((g >> 16) & 0xFF) < 8, "{g:08x}");
        // 0 %: no fill at all.
        render_level(&mut px, stride, &l, &info(0));
        let z = at(&px, quarter);
        assert!((z & 0xFF) < 0x60, "{z:08x}");
    }

    #[test]
    fn text_blend_covers() {
        let mut d = 0xFF10_1010u32;
        blend_text(&mut d, 255, PALETTE.text);
        assert_eq!(d, 0xFFFF_FFFF);
        let mut d = 0xFF10_1010u32;
        blend_text(&mut d, 0, PALETTE.text);
        assert_eq!(d, 0xFF10_1010);
    }
}
