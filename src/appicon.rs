//! The app icon, drawn in code (`appicon_draw.rs`, shared with `build.rs` for the exe resource).

include!("appicon_draw.rs");

/// The window / taskbar icon.
pub fn window_icon() -> eframe::egui::IconData {
    const N: u32 = 64;
    eframe::egui::IconData {
        rgba: render_rgba(N, false),
        width: N,
        height: N,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(rgba: &[u8], n: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * n + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn the_icon_is_a_rounded_square_with_white_dots() {
        let n = 64;
        let img = render_rgba(n, false);
        assert_eq!(img.len(), (n * n * 4) as usize);
        assert_eq!(px(&img, n, 0, 0)[3], 0, "corner is transparent");
        assert_eq!(px(&img, n, 32, 56), [0, 120, 212, 255], "body is blue");
        assert_eq!(
            px(&img, n, 32, (0.385 * n as f32) as u32),
            [255, 255, 255, 255]
        );
        let grey = render_rgba(n, true);
        assert_eq!(px(&grey, n, 32, 56), [128, 128, 128, 255]);
    }
}
