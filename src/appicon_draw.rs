// The yubi app icon: a rounded square ("trackpad") with three fingertip dots on a gentle arc.
// Std only and no `use` lines, so `build.rs` can `include!` this file as well.

/// Straight-alpha RGBA, row-major, top-down, `size` x `size`. Blue square (grey when `paused`)
/// with three white dots, anti-aliased and transparent outside the rounded square.
pub fn render_rgba(size: u32, paused: bool) -> Vec<u8> {
    let (fr, fg, fb) = if paused { (128.0, 128.0, 128.0) } else { (0.0, 120.0, 212.0) };
    let s = size as f32;
    // Square inset by half a pixel so the edge stays crisp; corner radius ~ 22%.
    let inset = if size <= 24 { 0.5 } else { s * 0.02 };
    let half = s / 2.0 - inset;
    let corner = s * 0.22;
    let dot_r = s * 0.098;
    let dots = [
        (0.5 - 0.25, 0.46),
        (0.5, 0.385),
        (0.5 + 0.25, 0.46),
    ];
    const SS: u32 = 4;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (mut square, mut dot) = (0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    // Signed distance to the rounded square.
                    let qx = (px - s / 2.0).abs() - (half - corner);
                    let qy = (py - s / 2.0).abs() - (half - corner);
                    let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt()
                        + qx.max(qy).min(0.0)
                        - corner;
                    if d <= 0.0 {
                        square += 1.0;
                        if dots.iter().any(|&(dx, dy)| {
                            let (ex, ey) = (px - dx * s, py - dy * s);
                            ex * ex + ey * ey <= dot_r * dot_r
                        }) {
                            dot += 1.0;
                        }
                    }
                }
            }
            let n = (SS * SS) as f32;
            let alpha = square / n;
            let w = if square > 0.0 { dot / square } else { 0.0 };
            let i = ((y * size + x) * 4) as usize;
            out[i] = (fr * (1.0 - w) + 255.0 * w).round() as u8;
            out[i + 1] = (fg * (1.0 - w) + 255.0 * w).round() as u8;
            out[i + 2] = (fb * (1.0 - w) + 255.0 * w).round() as u8;
            out[i + 3] = (alpha * 255.0).round() as u8;
        }
    }
    out
}
