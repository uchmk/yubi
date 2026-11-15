//! System fonts loaded at runtime from the Windows font folder (nothing is bundled).
//!
//! - Text: "Yu Gothic UI" (face 1 of YuGothM.ttc = Regular; face 1 of YuGothR.ttc = Semilight),
//!   then Meiryo and MS Gothic as fallbacks.
//! - Semibold headings: "Yu Gothic UI Semibold" (face 2 of YuGothB.ttc).
//! - Icons: "Segoe Fluent Icons" (SegoeIcons.ttf), else "Segoe MDL2 Assets" (segmdl2.ttf).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};

/// (path, face index) in order of preference.
const TEXT: [(&str, u32); 5] = [
    (r"C:\Windows\Fonts\YuGothM.ttc", 1),
    (r"C:\Windows\Fonts\YuGothR.ttc", 1),
    (r"C:\Windows\Fonts\YuGothR.ttc", 0),
    (r"C:\Windows\Fonts\meiryo.ttc", 0),
    (r"C:\Windows\Fonts\msgothic.ttc", 0),
];
const SEMIBOLD: [(&str, u32); 1] = [(r"C:\Windows\Fonts\YuGothB.ttc", 2)];
const ICONS: [(&str, u32); 2] = [
    (r"C:\Windows\Fonts\SegoeIcons.ttf", 0),
    (r"C:\Windows\Fonts\segmdl2.ttf", 0),
];

static ICONS_LOADED: AtomicBool = AtomicBool::new(false);

/// Family for headings; falls back to the normal text font when no semibold face exists.
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Family for icon glyphs (see `icons.rs`).
pub fn icons() -> FontFamily {
    FontFamily::Name("icons".into())
}

/// False when no icon font was found; callers then draw no icons.
pub fn icons_available() -> bool {
    ICONS_LOADED.load(Ordering::Relaxed)
}

fn load(candidates: &[(&'static str, u32)]) -> Option<(&'static str, FontData)> {
    candidates.iter().find_map(|&(path, index)| {
        let bytes = std::fs::read(path).ok()?;
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        Some((path, data))
    })
}

pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::default();

    // Proportional: the Japanese font goes first so mixed Latin/Japanese text shares one baseline.
    match load(&TEXT) {
        Some((path, data)) => {
            eprintln!("yubi: text font {path} (face {})", data.index);
            fonts
                .font_data
                .insert("system-ja".to_owned(), Arc::new(data));
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "system-ja".to_owned());
            // Monospace keeps egui's mono font for digits and falls back to the Japanese font.
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push("system-ja".to_owned());
        }
        None => eprintln!("yubi: no Japanese system font found; Japanese text may not render"),
    }

    let mut semibold_chain = vec![];
    if let Some((path, data)) = load(&SEMIBOLD) {
        eprintln!("yubi: semibold font {path} (face {})", data.index);
        fonts
            .font_data
            .insert("system-semibold".to_owned(), Arc::new(data));
        semibold_chain.push("system-semibold".to_owned());
    }
    semibold_chain.extend(
        fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts.families.insert(semibold(), semibold_chain);

    // The family must exist even without the font, or selecting it would panic.
    let icon_chain = match load(&ICONS) {
        Some((path, data)) => {
            eprintln!("yubi: icon font {path}");
            fonts
                .font_data
                .insert("system-icons".to_owned(), Arc::new(data));
            ICONS_LOADED.store(true, Ordering::Relaxed);
            vec!["system-icons".to_owned()]
        }
        None => {
            eprintln!("yubi: no icon font found; icons are not drawn");
            fonts
                .families
                .get(&FontFamily::Proportional)
                .cloned()
                .unwrap_or_default()
        }
    };
    fonts.families.insert(icons(), icon_chain);

    ctx.set_fonts(fonts);
}
