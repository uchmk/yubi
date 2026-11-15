//! Design tokens: Fluent-style structure (Windows 11 Settings / PowerToys) with a black default.
//! Pages and widgets take every color, size, radius and stroke width from `Tokens`.

use eframe::egui::{self, Color32, Context, CornerRadius, FontFamily, FontId, Stroke, Theme, Vec2};

use crate::config::ThemeChoice;

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    // Spacing.
    pub space_xs: f32,
    pub space_sm: f32,
    pub space_md: f32,
    pub space_lg: f32,
    pub space_xl: f32,
    // Radius.
    pub radius_sm: u8,
    pub radius_md: u8,
    pub radius_lg: u8,
    // Stroke widths.
    pub stroke_thin: f32,
    pub stroke_focus: f32,
    // Font sizes.
    pub fs_small: f32,
    pub fs_body: f32,
    pub fs_section: f32,
    pub fs_title: f32,
    pub fs_icon: f32,
    pub fs_icon_large: f32,
    // Layout.
    pub nav_width: f32,
    pub nav_rail_width: f32,
    pub nav_auto_collapse_w: f32,
    pub nav_item_h: f32,
    pub nav_gap: f32,
    pub nav_pad: f32,
    pub nav_header_h: f32,
    pub nav_pill_w: f32,
    pub nav_pill_h: f32,
    pub topbar_h: f32,
    pub search_w: f32,
    pub search_h: f32,
    pub search_results_max: usize,
    pub row_min_h: f32,
    pub row_min_h_compact: f32,
    pub row_min_h_list: f32,
    pub tile_pad_x: f32,
    pub row_pad_x: f32,
    pub row_pad_y: f32,
    pub row_icon_w: f32,
    pub content_max_w: f32,
    pub two_col_min: f32,
    pub tile_w: f32,
    pub tile_h: f32,
    pub control_h: f32,
    pub combo_w: f32,
    pub text_input_w: f32,
    pub icon_button: f32,
    // Toggle switch.
    pub toggle_w: f32,
    pub toggle_h: f32,
    pub toggle_knob: f32,
    pub toggle_label_w: f32,
    // Slider.
    pub slider_w: f32,
    pub slider_rail_h: f32,
    pub slider_knob: f32,
    pub slider_value_w: f32,
    // Colors.
    pub bg: Color32,
    pub nav: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub separator: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub accent_soft: Color32,
    pub nav_selected: Color32,
    pub nav_hover: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub input_bg: Color32,
    pub rail: Color32,
    pub toggle_outline: Color32,
}

impl Tokens {
    /// Black theme: PowerToys dark structure on a pure black page.
    pub fn dark() -> Self {
        Self {
            bg: rgb(0x000000),
            nav: rgb(0x0C0C0C),
            card: rgb(0x161616),
            card_hover: rgb(0x1E1E1E),
            border: rgb(0x262626),
            separator: rgb(0x222222),
            text: rgb(0xFFFFFF),
            text_muted: rgb(0xA8A8A8),
            accent: rgb(0x60CDFF),
            on_accent: rgb(0x000000),
            accent_soft: rgb(0x0B3547),
            nav_selected: rgb(0x1A1A1A),
            nav_hover: rgb(0x141414),
            control: rgb(0x2A2A2A),
            control_hover: rgb(0x343434),
            input_bg: rgb(0x0F0F0F),
            rail: rgb(0x6B6B6B),
            toggle_outline: rgb(0xA8A8A8),
            ..Self::base()
        }
    }

    pub fn light() -> Self {
        Self {
            bg: rgb(0xF3F3F3),
            nav: rgb(0xEEEEEE),
            card: rgb(0xFBFBFB),
            card_hover: rgb(0xF6F6F6),
            border: rgb(0xE5E5E5),
            separator: rgb(0xE5E5E5),
            text: rgb(0x1A1A1A),
            text_muted: rgb(0x5D5D5D),
            accent: rgb(0x005FB8),
            on_accent: rgb(0xFFFFFF),
            accent_soft: rgb(0xCCE4F7),
            nav_selected: rgb(0xE2E2E2),
            nav_hover: rgb(0xE8E8E8),
            control: rgb(0xFFFFFF),
            control_hover: rgb(0xF5F5F5),
            input_bg: rgb(0xFFFFFF),
            rail: rgb(0x8A8A8A),
            toggle_outline: rgb(0x5D5D5D),
            ..Self::base()
        }
    }

    /// Sizes shared by both themes; colors are overwritten by `dark` / `light`.
    fn base() -> Self {
        Self {
            space_xs: 4.0,
            space_sm: 8.0,
            space_md: 12.0,
            space_lg: 24.0,
            space_xl: 36.0,
            radius_sm: 4,
            radius_md: 6,
            radius_lg: 8,
            stroke_thin: 1.0,
            stroke_focus: 2.0,
            fs_small: 12.0,
            fs_body: 14.0,
            fs_section: 16.0,
            fs_title: 28.0,
            fs_icon: 16.0,
            fs_icon_large: 26.0,
            nav_width: 280.0,
            nav_rail_width: 52.0,
            nav_auto_collapse_w: 820.0,
            nav_item_h: 34.0,
            nav_gap: 2.0,
            nav_pad: 8.0,
            nav_header_h: 56.0,
            nav_pill_w: 3.0,
            nav_pill_h: 16.0,
            topbar_h: 48.0,
            search_w: 360.0,
            search_h: 32.0,
            search_results_max: 8,
            row_min_h: 64.0,
            row_min_h_compact: 40.0,
            row_min_h_list: 48.0,
            tile_pad_x: 12.0,
            row_pad_x: 16.0,
            row_pad_y: 12.0,
            row_icon_w: 40.0,
            content_max_w: 1180.0,
            two_col_min: 900.0,
            tile_w: 112.0,
            tile_h: 84.0,
            control_h: 32.0,
            combo_w: 200.0,
            text_input_w: 220.0,
            icon_button: 32.0,
            toggle_w: 40.0,
            toggle_h: 20.0,
            toggle_knob: 12.0,
            toggle_label_w: 30.0,
            slider_w: 200.0,
            slider_rail_h: 4.0,
            slider_knob: 18.0,
            slider_value_w: 56.0,
            bg: Color32::BLACK,
            nav: Color32::BLACK,
            card: Color32::BLACK,
            card_hover: Color32::BLACK,
            border: Color32::BLACK,
            separator: Color32::BLACK,
            text: Color32::BLACK,
            text_muted: Color32::BLACK,
            accent: Color32::BLACK,
            on_accent: Color32::BLACK,
            accent_soft: Color32::BLACK,
            nav_selected: Color32::BLACK,
            nav_hover: Color32::BLACK,
            control: Color32::BLACK,
            control_hover: Color32::BLACK,
            input_bg: Color32::BLACK,
            rail: Color32::BLACK,
            toggle_outline: Color32::BLACK,
        }
    }

    /// Tokens for the theme currently in effect.
    pub fn of(ctx: &Context) -> Self {
        match ctx.theme() {
            Theme::Dark => Self::dark(),
            Theme::Light => Self::light(),
        }
    }

    pub fn font(&self, size: f32) -> FontId {
        FontId::proportional(size)
    }

    pub fn font_semibold(&self, size: f32) -> FontId {
        FontId::new(size, crate::fonts::semibold())
    }

    pub fn disabled(&self, color: Color32) -> Color32 {
        color.gamma_multiply(0.4)
    }
}

/// Install the styles and select the theme for `choice`.
/// Black and System both use the black tokens in dark mode; Light uses the light tokens.
pub fn apply(ctx: &Context, choice: ThemeChoice) {
    ctx.set_theme(match choice {
        ThemeChoice::Black => egui::ThemePreference::Dark,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::System => egui::ThemePreference::System,
    });
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
    v.window_fill = t.card;
    v.extreme_bg_color = t.input_bg;
    v.faint_bg_color = t.card_hover;
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(t.stroke_thin, t.accent);
    v.window_stroke = Stroke::new(t.stroke_thin, t.border);
    v.window_corner_radius = CornerRadius::same(t.radius_lg);
    v.menu_corner_radius = CornerRadius::same(t.radius_lg);
    v.popup_shadow = egui::Shadow::NONE;
    v.window_shadow = egui::Shadow::NONE;

    let radius = CornerRadius::same(t.radius_md);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(t.stroke_thin, t.border);
    w.noninteractive.fg_stroke = Stroke::new(t.stroke_thin, t.text);
    w.noninteractive.corner_radius = radius;
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = radius;
        state.fg_stroke = Stroke::new(t.stroke_thin, t.text);
    }
    w.inactive.weak_bg_fill = t.control;
    w.inactive.bg_fill = t.control;
    w.inactive.bg_stroke = Stroke::new(t.stroke_thin, t.border);
    w.hovered.weak_bg_fill = t.control_hover;
    w.hovered.bg_fill = t.control_hover;
    w.hovered.bg_stroke = Stroke::new(t.stroke_thin, t.text_muted);
    w.active.weak_bg_fill = t.accent_soft;
    w.active.bg_fill = t.accent_soft;
    w.active.bg_stroke = Stroke::new(t.stroke_thin, t.accent);
    w.open.weak_bg_fill = t.control_hover;
    w.open.bg_fill = t.control_hover;

    s.spacing.item_spacing = Vec2::splat(t.space_sm);
    s.spacing.button_padding = Vec2::new(t.space_md, t.space_xs + 1.0);
    s.spacing.interact_size.y = t.control_h;
    s.spacing.menu_margin = egui::Margin::same(t.space_xs as i8);

    s.text_styles = [
        (egui::TextStyle::Small, FontId::proportional(t.fs_small)),
        (egui::TextStyle::Body, FontId::proportional(t.fs_body)),
        (egui::TextStyle::Button, FontId::proportional(t.fs_body)),
        (
            egui::TextStyle::Heading,
            FontId::new(t.fs_title, crate::fonts::semibold()),
        ),
        (
            egui::TextStyle::Monospace,
            FontId::new(t.fs_body - 1.0, FontFamily::Monospace),
        ),
    ]
    .into();
}
