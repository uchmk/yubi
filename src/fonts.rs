//! Japanese fallback font loaded from the Windows font folder.

use std::sync::Arc;

use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};

const CANDIDATES: [&str; 3] = [
    r"C:\Windows\Fonts\YuGothM.ttc",
    r"C:\Windows\Fonts\meiryo.ttc",
    r"C:\Windows\Fonts\msgothic.ttc",
];

pub fn install(ctx: &Context) {
    let Some((path, bytes)) = CANDIDATES
        .iter()
        .find_map(|p| std::fs::read(p).ok().map(|b| (*p, b)))
    else {
        eprintln!("yubi: no Japanese system font found; Japanese text may not render");
        return;
    };
    let mut data = FontData::from_owned(bytes);
    data.index = 0;
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("system-ja".to_owned(), Arc::new(data));
    // Proportional: the Japanese font goes first so mixed Latin/Japanese text shares one baseline.
    // Monospace: keep egui's mono font for digits and append the Japanese font as fallback.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "system-ja".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push("system-ja".to_owned());
    ctx.set_fonts(fonts);
    eprintln!("yubi: using font {path}");
}
