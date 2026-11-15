//! Design tokens (Quiet Precision): one accent, restrained neutrals.
//! Pages take every color, spacing and radius from `Tokens`.

use eframe::egui::{self, Color32, Context, CornerRadius, Stroke, Theme, Vec2};

#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub space_xs: f32,
    pub space_sm: f32,
    pub space_md: f32,
    pub space_lg: f32,
    pub radius_sm: u8,
    pub radius_md: u8,
    pub radius_lg: u8,
    pub sidebar_width: f32,
    pub bg: Color32,
    pub surface: Color32,
    pub surface_raised: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub accent_soft: Color32,
}

impl Tokens {
    pub fn light() -> Self {
        Self {
            bg: Color32::from_rgb(0xFA, 0xFA, 0xFB),
            surface: Color32::from_rgb(0xF0, 0xF1, 0xF3),
            surface_raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            border: Color32::from_rgb(0xD9, 0xDB, 0xE0),
            text: Color32::from_rgb(0x1C, 0x1E, 0x23),
            text_muted: Color32::from_rgb(0x6A, 0x6F, 0x7A),
            accent: Color32::from_rgb(0x2F, 0x6F, 0xEB),
            on_accent: Color32::WHITE,
            accent_soft: Color32::from_rgb(0xDF, 0xE9, 0xFC),
            ..Self::base()
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: Color32::from_rgb(0x16, 0x17, 0x1B),
            surface: Color32::from_rgb(0x1D, 0x1F, 0x24),
            surface_raised: Color32::from_rgb(0x25, 0x27, 0x2D),
            border: Color32::from_rgb(0x3A, 0x3D, 0x45),
            text: Color32::from_rgb(0xEC, 0xED, 0xEF),
            text_muted: Color32::from_rgb(0x9B, 0xA0, 0xAB),
            accent: Color32::from_rgb(0x6A, 0xA1, 0xFF),
            on_accent: Color32::from_rgb(0x0B, 0x16, 0x2B),
            accent_soft: Color32::from_rgb(0x24, 0x34, 0x55),
            ..Self::base()
        }
    }

    fn base() -> Self {
        Self {
            space_xs: 4.0,
            space_sm: 8.0,
            space_md: 14.0,
            space_lg: 24.0,
            radius_sm: 4,
            radius_md: 8,
            radius_lg: 14,
            sidebar_width: 212.0,
            bg: Color32::BLACK,
            surface: Color32::BLACK,
            surface_raised: Color32::BLACK,
            border: Color32::BLACK,
            text: Color32::BLACK,
            text_muted: Color32::BLACK,
            accent: Color32::BLACK,
            on_accent: Color32::BLACK,
            accent_soft: Color32::BLACK,
        }
    }

    /// Tokens for the theme currently in effect.
    pub fn of(ctx: &Context) -> Self {
        match ctx.theme() {
            Theme::Dark => Self::dark(),
            Theme::Light => Self::light(),
        }
    }
}

/// Install light and dark styles built from the tokens.
pub fn apply(ctx: &Context) {
    ctx.set_theme(egui::ThemePreference::System);
    ctx.style_mut_of(Theme::Light, |s| style(s, &Tokens::light(), false));
    ctx.style_mut_of(Theme::Dark, |s| style(s, &Tokens::dark(), true));
}

fn style(s: &mut egui::Style, t: &Tokens, dark: bool) {
    s.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let v = &mut s.visuals;
    v.panel_fill = t.bg;
    v.window_fill = t.surface_raised;
    v.extreme_bg_color = t.surface_raised;
    v.faint_bg_color = t.surface;
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.window_stroke = Stroke::new(1.0, t.border);
    v.slider_trailing_fill = true;

    let radius = CornerRadius::same(t.radius_md);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = radius;
        state.fg_stroke = Stroke::new(1.0, t.text);
    }
    w.noninteractive.corner_radius = radius;
    w.inactive.weak_bg_fill = t.surface_raised;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border);
    // bg_fill paints slider rails and checkbox boxes; it must differ from the card (surface_raised).
    w.inactive.bg_fill = t.border;
    w.hovered.bg_fill = t.border;
    w.hovered.weak_bg_fill = t.surface;
    w.hovered.bg_stroke = Stroke::new(1.0, t.text_muted);
    w.active.bg_fill = t.accent_soft;
    w.active.weak_bg_fill = t.accent_soft;
    w.active.bg_stroke = Stroke::new(1.0, t.accent);
    w.open.bg_fill = t.surface;
    w.open.weak_bg_fill = t.surface;

    s.spacing.item_spacing = Vec2::splat(t.space_sm);
    s.spacing.button_padding = Vec2::new(t.space_md, t.space_xs + 2.0);
    s.spacing.interact_size.y = 28.0;
}
