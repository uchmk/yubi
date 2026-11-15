//! Fluent-style widgets: toggle switch, settings cards, slider, combo box, navigation items.
//! Every color, size and stroke comes from `Tokens`.

use std::fmt::Debug;
use std::hash::Hash;

use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Frame, Layout, Margin, Painter, Pos2, Rect, Response,
    RichText, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, text::LayoutJob,
};

use crate::config::{Action, ActionKind};
use crate::fonts;
use crate::icons;
use crate::theme::Tokens;

// ---------------------------------------------------------------------------------------------
// Icons

/// Draw an icon glyph centered on `center`. Draws nothing when no icon font was found.
pub fn paint_icon(painter: &Painter, center: Pos2, glyph: char, size: f32, color: Color32) {
    if fonts::icons_available() {
        painter.text(
            center,
            Align2::CENTER_CENTER,
            glyph.to_string(),
            egui::FontId::new(size, fonts::icons()),
            color,
        );
    }
}

fn dim(ui: &Ui, t: &Tokens, color: Color32) -> Color32 {
    if ui.is_enabled() {
        color
    } else {
        t.disabled(color)
    }
}

// ---------------------------------------------------------------------------------------------
// Page structure

pub fn page_header(ui: &mut Ui, t: &Tokens, title: &str, subtitle: &str) {
    ui.add_space(t.space_md);
    ui.label(RichText::new(title).font(t.font_semibold(t.fs_title)));
    ui.add_space(t.space_xs);
    ui.label(RichText::new(subtitle).color(t.text_muted));
    ui.add_space(t.space_lg);
}

pub fn section_header(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.add_space(t.space_md);
    ui.label(RichText::new(text).font(t.font_semibold(t.fs_section)));
    ui.add_space(t.space_sm);
}

/// A card made of joined rows separated by thin lines.
pub struct Group<'a> {
    ui: &'a mut Ui,
    t: &'a Tokens,
    first: bool,
}

pub fn group<R>(ui: &mut Ui, t: &Tokens, add: impl FnOnce(&mut Group<'_>) -> R) -> R {
    let out = Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(t.stroke_thin, t.border))
        .corner_radius(t.radius_lg)
        .inner_margin(Margin::ZERO)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(&mut Group { ui, t, first: true })
        })
        .inner;
    ui.add_space(t.space_sm);
    out
}

impl Group<'_> {
    /// The egui context, for the small bits of per-widget state a row keeps.
    pub fn ctx(&self) -> egui::Context {
        self.ui.ctx().clone()
    }

    /// Free-form row: `add` fills a left-to-right line with the standard row padding.
    pub fn raw_row(&mut self, add: impl FnOnce(&mut Ui)) {
        self.row_sized(self.t.row_min_h, self.t.row_pad_y, add);
    }

    /// Row whose children wrap onto further lines when the card is narrow.
    pub fn wrapped_row(&mut self, add: impl FnOnce(&mut Ui)) {
        let t = self.t;
        let rect = Frame::new()
            .inner_margin(Margin::symmetric(t.row_pad_x as i8, t.row_pad_y as i8))
            .show(self.ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(add);
            })
            .response
            .rect;
        if !self.first {
            self.ui.painter().hline(
                rect.x_range(),
                rect.top(),
                Stroke::new(t.stroke_thin, t.separator),
            );
        }
        self.first = false;
    }

    /// Compact row for read-only values: muted label on the left, monospace value on the right.
    pub fn info_row(&mut self, label: &str, value: &str) {
        let t = self.t;
        self.row_sized(t.row_min_h_compact, t.space_sm, |ui| {
            let label_w = ui
                .painter()
                .layout_no_wrap(
                    label.to_owned(),
                    egui::TextStyle::Body.resolve(ui.style()),
                    t.text_muted,
                )
                .size()
                .x;
            // The value gets what the label leaves and is cut with an ellipsis (full text on hover).
            let value_w = (ui.available_width() - label_w - t.space_lg).max(t.tile_w);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.scope(|ui| {
                    ui.set_max_width(value_w);
                    ui.add(
                        egui::Label::new(RichText::new(value).monospace())
                            .truncate()
                            .sense(Sense::hover()),
                    )
                    .on_hover_text(value);
                });
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new(label).color(t.text_muted));
                });
            });
        });
    }

    /// Compact list row: title on the left, muted value on the right, no description.
    pub fn list_row(&mut self, title: &str, value: &str) {
        let t = self.t;
        self.row_sized(t.row_min_h_list, t.row_pad_y, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(value).color(t.text_muted));
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new(title).size(t.fs_body));
                });
            });
        });
    }

    fn row_sized(&mut self, min_h: f32, pad_y: f32, add: impl FnOnce(&mut Ui)) {
        let t = self.t;
        let rect = Frame::new()
            .inner_margin(Margin::symmetric(t.row_pad_x as i8, pad_y as i8))
            .show(self.ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().interact_size.y = min_h - 2.0 * pad_y;
                ui.horizontal(add);
            })
            .response
            .rect;
        if !self.first {
            self.ui.painter().hline(
                rect.x_range(),
                rect.top(),
                Stroke::new(t.stroke_thin, t.separator),
            );
        }
        self.first = false;
    }

    /// Standard row: optional icon, title and description on the left, `control` on the right.
    pub fn row(
        &mut self,
        icon: Option<char>,
        title: &str,
        desc: &str,
        control: impl FnOnce(&mut Ui),
    ) {
        let t = self.t;
        self.raw_row(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                control(ui);
                // A clear gap between the control and the (wrapping) description.
                ui.add_space(t.space_lg);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if let Some(glyph) = icon.filter(|_| fonts::icons_available()) {
                        let (rect, _) = ui.allocate_exact_size(
                            Vec2::new(t.row_icon_w, t.fs_icon_large),
                            Sense::hover(),
                        );
                        paint_icon(
                            ui.painter(),
                            Pos2::new(rect.left() + t.fs_icon_large / 2.0, rect.center().y),
                            glyph,
                            t.fs_icon,
                            dim(ui, t, t.text),
                        );
                    }
                    if desc.is_empty() {
                        // No description: a plain label stays vertically centred in the row.
                        ui.label(RichText::new(title).size(t.fs_body));
                    } else {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(RichText::new(title).size(t.fs_body));
                            ui.label(RichText::new(desc).size(t.fs_small).color(t.text_muted));
                        });
                    }
                });
            });
        });
    }

    pub fn toggle_row(&mut self, icon: Option<char>, title: &str, desc: &str, on: &mut bool) {
        let t = self.t;
        self.row(icon, title, desc, |ui| {
            toggle(ui, t, on);
        });
    }

    pub fn slider_row<N: Num>(
        &mut self,
        title: &str,
        desc: &str,
        spec: SliderSpec,
        value: &mut N,
    ) -> bool {
        let t = self.t;
        let mut changed = false;
        self.row(None, title, desc, |ui| {
            changed = slider_with_value(ui, t, value, spec);
        });
        changed
    }

    pub fn action_row(
        &mut self,
        title: &str,
        desc: &str,
        id: impl Hash + Debug,
        action: &mut Action,
    ) {
        let t = self.t;
        self.row(None, title, desc, |ui| action_picker(ui, t, id, action));
    }

    pub fn combo_row<T: Copy + PartialEq>(
        &mut self,
        title: &str,
        desc: &str,
        id: impl Hash + Debug,
        value: &mut T,
        options: &[T],
        label: impl Fn(T) -> &'static str,
    ) -> bool {
        let t = self.t;
        let mut changed = false;
        self.row(None, title, desc, |ui| {
            changed = combo(ui, t, id, value, options, label);
        });
        changed
    }
}

// ---------------------------------------------------------------------------------------------
// Controls

/// Fluent toggle switch with an "オン"/"オフ" label on its left.
pub fn toggle(ui: &mut Ui, t: &Tokens, on: &mut bool) -> Response {
    let size = Vec2::new(
        t.toggle_label_w + t.space_md + t.toggle_w,
        t.control_h.max(t.toggle_h),
    );
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(Role::Switch, enabled, *on, ""));
    if enabled {
        resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    }

    let k = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let painter = ui.painter();
    let pill = Rect::from_min_size(
        Pos2::new(
            rect.right() - t.toggle_w,
            rect.center().y - t.toggle_h / 2.0,
        ),
        Vec2::new(t.toggle_w, t.toggle_h),
    );
    let radius = t.toggle_h / 2.0;
    let color = |c: Color32| dim(ui, t, c);
    painter.rect_filled(pill, radius, color(t.accent).gamma_multiply(k));
    painter.rect_stroke(
        pill,
        radius,
        Stroke::new(
            t.stroke_thin,
            color(t.toggle_outline).gamma_multiply(1.0 - k),
        ),
        StrokeKind::Inside,
    );
    let knob_r = t.toggle_knob / 2.0 + if resp.hovered() && enabled { 1.0 } else { 0.0 };
    let x_off = pill.left() + radius;
    let x_on = pill.right() - radius;
    let knob_color = color(t.text_muted).lerp_to_gamma(color(t.on_accent), k);
    painter.circle_filled(
        Pos2::new(x_off + (x_on - x_off) * k, pill.center().y),
        knob_r,
        knob_color,
    );
    if resp.has_focus() {
        painter.rect_stroke(
            pill.expand(t.stroke_focus + 1.0),
            radius + t.stroke_focus + 1.0,
            Stroke::new(t.stroke_focus, t.accent),
            StrokeKind::Outside,
        );
    }
    painter.text(
        Pos2::new(pill.left() - t.space_md, rect.center().y),
        Align2::RIGHT_CENTER,
        if *on { "オン" } else { "オフ" },
        t.font(t.fs_body),
        color(t.text),
    );
    resp
}

/// Numeric types a slider can edit.
pub trait Num: Copy {
    fn to_f64(self) -> f64;
    fn from_f64(v: f64) -> Self;
}

impl Num for u8 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    fn from_f64(v: f64) -> Self {
        v.round().clamp(0.0, 255.0) as u8
    }
}

impl Num for u16 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    fn from_f64(v: f64) -> Self {
        v.round().clamp(0.0, 65535.0) as u16
    }
}

impl Num for f32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    fn from_f64(v: f64) -> Self {
        v as f32
    }
}

#[derive(Clone, Copy)]
pub struct SliderSpec {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub decimals: usize,
    pub suffix: &'static str,
}

impl SliderSpec {
    pub const fn int(min: u16, max: u16, step: u16, suffix: &'static str) -> Self {
        Self {
            min: min as f64,
            max: max as f64,
            step: step as f64,
            decimals: 0,
            suffix,
        }
    }
}

/// Slider with a visible rail, an accent trailing fill, and the value text on its left.
pub fn slider_with_value<N: Num>(ui: &mut Ui, t: &Tokens, value: &mut N, spec: SliderSpec) -> bool {
    let mut v = value.to_f64();
    let changed = slider(ui, t, &mut v, spec);
    if changed {
        *value = N::from_f64(v);
    }
    let shown = format!("{:.*}{}", spec.decimals, value.to_f64(), spec.suffix);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(t.slider_value_w, t.control_h), Sense::hover());
    ui.painter().text(
        Pos2::new(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        shown,
        t.font(t.fs_body),
        dim(ui, t, t.text),
    );
    changed
}

pub fn slider(ui: &mut Ui, t: &Tokens, value: &mut f64, spec: SliderSpec) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(t.slider_w, t.control_h), Sense::click_and_drag());
    let knob_r = t.slider_knob / 2.0;
    let (x0, x1) = (rect.left() + knob_r, rect.right() - knob_r);
    let snap = |v: f64| {
        let steps = ((v - spec.min) / spec.step).round();
        (spec.min + steps * spec.step).clamp(spec.min, spec.max)
    };
    let before = *value;
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on())
    {
        let frac = f64::from(((p.x - x0) / (x1 - x0)).clamp(0.0, 1.0));
        *value = snap(spec.min + frac * (spec.max - spec.min));
    }
    if resp.has_focus() {
        let dir = ui.input(|i| {
            i64::from(i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp))
                - i64::from(
                    i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown),
                )
        });
        if dir != 0 {
            *value = snap(*value + dir as f64 * spec.step);
        }
    }
    let changed = (*value - before).abs() > f64::EPSILON;

    let frac = ((*value - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0) as f32;
    let knob_x = x0 + (x1 - x0) * frac;
    let cy = rect.center().y;
    let painter = ui.painter();
    let rail = Rect::from_min_max(
        Pos2::new(x0, cy - t.slider_rail_h / 2.0),
        Pos2::new(x1, cy + t.slider_rail_h / 2.0),
    );
    let r = t.slider_rail_h / 2.0;
    painter.rect_filled(rail, r, dim(ui, t, t.rail));
    painter.rect_filled(
        Rect::from_min_max(rail.min, Pos2::new(knob_x, rail.max.y)),
        r,
        dim(ui, t, t.accent),
    );
    let center = Pos2::new(knob_x, cy);
    painter.circle_filled(center, knob_r, dim(ui, t, t.control));
    painter.circle_stroke(
        center,
        knob_r,
        Stroke::new(t.stroke_thin, dim(ui, t, t.rail)),
    );
    let inner = if resp.hovered() || resp.dragged() {
        knob_r * 0.7
    } else {
        knob_r * 0.5
    };
    painter.circle_filled(center, inner, dim(ui, t, t.accent));
    if resp.has_focus() {
        painter.circle_stroke(
            center,
            knob_r + t.stroke_focus,
            Stroke::new(t.stroke_focus, t.accent),
        );
    }
    changed
}

/// Fluent-style combo box (rounded, chevron-down glyph). Returns whether the value changed.
pub fn combo<T: Copy + PartialEq>(
    ui: &mut Ui,
    t: &Tokens,
    id: impl Hash + Debug,
    value: &mut T,
    options: &[T],
    label: impl Fn(T) -> &'static str,
) -> bool {
    combo_sized(ui, t, t.combo_w, id, value, options, label)
}

pub fn combo_sized<T: Copy + PartialEq>(
    ui: &mut Ui,
    t: &Tokens,
    width: f32,
    id: impl Hash + Debug,
    value: &mut T,
    options: &[T],
    label: impl Fn(T) -> &'static str,
) -> bool {
    let mut changed = false;
    combo_menu(ui, t, width, id, label(*value), |ui| {
        for &o in options {
            if ui.selectable_label(*value == o, label(o)).clicked() {
                *value = o;
                changed = true;
            }
        }
    });
    changed
}

/// The same combo box with free-form menu contents: `selected` is the text on the closed box.
pub fn combo_menu(
    ui: &mut Ui,
    t: &Tokens,
    width: f32,
    id: impl Hash + Debug,
    selected: &str,
    menu: impl FnOnce(&mut Ui),
) {
    let size = t.fs_small;
    let stroke = t.stroke_thin;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(selected)
        .icon(move |ui, rect, visuals, _open| {
            let c = rect.center();
            let color = visuals.fg_stroke.color;
            if fonts::icons_available() {
                paint_icon(ui.painter(), c, icons::CHEVRON_DOWN, size, color);
            } else {
                let d = size / 3.0;
                let s = Stroke::new(stroke, color);
                let p = ui.painter();
                p.line_segment(
                    [c + Vec2::new(-d, -d / 2.0), c + Vec2::new(0.0, d / 2.0)],
                    s,
                );
                p.line_segment([c + Vec2::new(0.0, d / 2.0), c + Vec2::new(d, -d / 2.0)], s);
            }
        })
        .show_ui(ui, menu);
}

/// Action combo box plus a key-string field when "キー入力" is chosen.
pub fn action_picker(ui: &mut Ui, t: &Tokens, id: impl Hash + Debug, action: &mut Action) {
    action_picker_with(ui, t, id, action, &ActionKind::ALL, ActionKind::label);
}

/// [`action_picker`] limited to the action kinds in `kinds`.
pub fn action_picker_of(
    ui: &mut Ui,
    t: &Tokens,
    id: impl Hash + Debug,
    action: &mut Action,
    kinds: &[ActionKind],
) {
    action_picker_with(ui, t, id, action, kinds, ActionKind::label);
}

/// [`action_picker`] with its own list of kinds and names for them (for example one that also
/// offers the window resize, or calls "なし" "オフ").
pub fn action_picker_with(
    ui: &mut Ui,
    t: &Tokens,
    id: impl Hash + Debug,
    action: &mut Action,
    kinds: &[ActionKind],
    label: fn(ActionKind) -> &'static str,
) {
    let mut kind = action.kind();
    if combo(ui, t, id, &mut kind, kinds, label) {
        action.set_kind(kind);
    }
    action_fields(ui, t, action);
}

/// The extra field an action needs (keys, or a step), if any.
pub fn action_fields(ui: &mut Ui, t: &Tokens, action: &mut Action) {
    match action {
        Action::Keys(keys) => {
            ui.add(
                egui::TextEdit::singleline(keys)
                    .hint_text("例: Ctrl+Alt+T")
                    .desired_width(t.text_input_w),
            );
        }
        Action::VolumeStep(n) | Action::BrightnessStep(n) => step_field(ui, n),
        _ => {}
    }
}

/// The signed step of a volume or brightness action, in percentage points.
pub fn step_field(ui: &mut Ui, n: &mut i8) {
    ui.add(
        egui::DragValue::new(n)
            .range(-crate::config::MAX_STEP..=crate::config::MAX_STEP)
            .speed(0.2)
            .prefix("1回に ")
            .suffix(" %")
            .custom_formatter(|v, _| format!("{v:+}")),
    );
    if *n == 0 {
        *n = 1;
    }
}

pub fn button(ui: &mut Ui, t: &Tokens, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(t.fs_body))
            .min_size(Vec2::new(0.0, t.control_h)),
    )
}

pub fn accent_button(ui: &mut Ui, t: &Tokens, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(t.fs_body).color(t.on_accent))
            .fill(t.accent)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(0.0, t.control_h)),
    )
}

/// Square icon-only button with a hover fill and a tooltip.
pub fn icon_button(ui: &mut Ui, t: &Tokens, glyph: char, tooltip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(t.icon_button), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, tooltip));
    if resp.hovered() && enabled {
        ui.painter().rect_filled(rect, t.radius_md, t.control_hover);
    }
    paint_icon(
        ui.painter(),
        rect.center(),
        glyph,
        t.fs_icon,
        dim(ui, t, t.text),
    );
    resp.on_hover_text(tooltip)
}

/// Width of a quick-access tile that fits the widest of `labels` (never below `tile_w`).
pub fn tile_width(ui: &Ui, t: &Tokens, labels: &[&str]) -> f32 {
    let widest = labels
        .iter()
        .map(|l| {
            ui.painter()
                .layout_no_wrap((*l).to_owned(), t.font(t.fs_small), t.text)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    t.tile_w.max(widest + 2.0 * t.tile_pad_x)
}

/// Icon-over-label tile for the quick-access grid; `width` comes from `tile_width`.
pub fn tile(
    ui: &mut Ui,
    t: &Tokens,
    width: f32,
    glyph: char,
    label: &str,
    enabled: bool,
) -> Response {
    let enabled = enabled && ui.is_enabled();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, t.tile_h), sense);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    let painter = ui.painter();
    if resp.hovered() && enabled {
        painter.rect_filled(rect, t.radius_md, t.card_hover);
    }
    painter.rect_stroke(
        rect,
        t.radius_md,
        Stroke::new(t.stroke_thin, t.separator),
        StrokeKind::Inside,
    );
    paint_icon(
        painter,
        Pos2::new(rect.center().x, rect.top() + t.tile_h * 0.34),
        glyph,
        t.fs_icon_large,
        if enabled {
            t.accent
        } else {
            t.disabled(t.accent)
        },
    );
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        t.font(t.fs_small),
        if enabled { t.text } else { t.disabled(t.text) },
    );
    let pos = Pos2::new(
        rect.center().x - galley.size().x / 2.0,
        rect.bottom() - t.space_md - galley.size().y / 2.0,
    );
    ui.painter().galley(pos, galley, t.text);
    resp
}

// ---------------------------------------------------------------------------------------------
// Navigation

fn single_line(text: &str, font: egui::FontId, color: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple(text.to_owned(), font, color, width.max(1.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job
}

/// One navigation entry. Selected: subtle fill plus a short accent pill on the left.
pub fn nav_item(
    ui: &mut Ui,
    t: &Tokens,
    collapsed: bool,
    selected: bool,
    glyph: char,
    label: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), t.nav_item_h),
        Sense::click(),
    );
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, label));
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, t.radius_md, t.nav_selected);
        let pill = Rect::from_center_size(
            Pos2::new(rect.left() + t.nav_pill_w, rect.center().y),
            Vec2::new(t.nav_pill_w, t.nav_pill_h),
        );
        painter.rect_filled(pill, t.nav_pill_w / 2.0, t.accent);
    } else if resp.hovered() {
        painter.rect_filled(rect, t.radius_md, t.nav_hover);
    }
    let icon_x = if collapsed {
        rect.center().x
    } else {
        rect.left() + t.space_lg
    };
    paint_icon(
        painter,
        Pos2::new(icon_x, rect.center().y),
        glyph,
        t.fs_icon,
        if selected { t.accent } else { t.text },
    );
    if collapsed {
        return resp.on_hover_text(label);
    }
    let x = rect.left() + t.space_lg + t.fs_icon / 2.0 + t.space_md;
    let galley = painter.layout_job(single_line(
        label,
        t.font(t.fs_body),
        t.text,
        rect.right() - x - t.space_sm,
    ));
    let pos = Pos2::new(x, rect.center().y - galley.size().y / 2.0);
    painter.galley(pos, galley, t.text);
    resp
}

/// Collapsible group header: label on the left, chevron on the right.
pub fn nav_group_header(ui: &mut Ui, t: &Tokens, label: &str, open: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), t.nav_item_h),
        Sense::click(),
    );
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, open, label));
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, t.radius_md, t.nav_hover);
    }
    let x = rect.left() + t.space_lg - t.fs_icon / 2.0;
    let galley = painter.layout_job(single_line(
        label,
        t.font_semibold(t.fs_body),
        t.text_muted,
        rect.right() - x - t.space_xl,
    ));
    painter.galley(
        Pos2::new(x, rect.center().y - galley.size().y / 2.0),
        galley,
        t.text_muted,
    );
    paint_icon(
        painter,
        Pos2::new(rect.right() - t.space_lg, rect.center().y),
        if open {
            icons::CHEVRON_UP
        } else {
            icons::CHEVRON_DOWN
        },
        t.fs_small,
        t.text_muted,
    );
    resp
}

/// Device status header of the navigation pane.
pub fn device_status(ui: &mut Ui, t: &Tokens, collapsed: bool, connected: bool) {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), t.nav_header_h),
        Sense::hover(),
    );
    let state = if connected {
        "接続済み"
    } else {
        "未接続"
    };
    let painter = ui.painter();
    let icon_x = if collapsed {
        rect.center().x
    } else {
        rect.left() + t.space_lg
    };
    paint_icon(
        painter,
        Pos2::new(icon_x, rect.center().y),
        icons::TOUCHPAD,
        t.fs_icon_large,
        if connected { t.text } else { t.text_muted },
    );
    if collapsed {
        resp.on_hover_text(format!("Magic Trackpad · {state}"));
        return;
    }
    let x = rect.left() + t.space_lg + t.fs_icon_large / 2.0 + t.space_md;
    painter.text(
        Pos2::new(x, rect.center().y - t.space_sm),
        Align2::LEFT_CENTER,
        "Magic Trackpad",
        t.font_semibold(t.fs_body),
        t.text,
    );
    // The dot is redundant with the text; it only reinforces the state.
    let dot = Pos2::new(x + t.space_xs, rect.center().y + t.space_sm + 1.0);
    if connected {
        painter.circle_filled(dot, t.space_xs, t.accent);
    } else {
        painter.circle_stroke(
            dot,
            t.space_xs - 0.5,
            Stroke::new(t.stroke_thin, t.text_muted),
        );
    }
    painter.text(
        Pos2::new(dot.x + t.space_md, dot.y),
        Align2::LEFT_CENTER,
        state,
        t.font(t.fs_small),
        t.text_muted,
    );
}
