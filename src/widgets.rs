//! Fluent-style widgets: toggle switch, settings cards, slider, combo box, navigation items.
//! Every color, size and stroke comes from `Tokens`.

use std::fmt::Debug;
use std::hash::Hash;

use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Frame, Layout, Margin, Painter, Pos2, Rect, Response,
    RichText, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, text::LayoutJob,
};

use crate::config::{Action, ActionGroup, ActionKind};
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
    combo_menu_with(
        ui,
        t,
        width,
        id,
        selected,
        egui::PopupCloseBehavior::CloseOnClick,
        menu,
    );
}

/// [`combo_menu`] with its own rule for when a click closes the menu.
fn combo_menu_with(
    ui: &mut Ui,
    t: &Tokens,
    width: f32,
    id: impl Hash + Debug,
    selected: &str,
    close: egui::PopupCloseBehavior,
    menu: impl FnOnce(&mut Ui),
) {
    let size = t.fs_small;
    let stroke = t.stroke_thin;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .close_behavior(close)
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

// ---------------------------------------------------------------------------------------------
// Action picker: a grouped, filterable list of action kinds

/// A list of at least this many entries gets a filter box.
const PICKER_FILTER_MIN: usize = 10;
/// The tallest the list of a picker grows before it scrolls.
const PICKER_MAX_H: f32 = 340.0;
/// The widest the picker's popup needs for its longest label.
const PICKER_MIN_W: f32 = 260.0;

/// What a pick of an action picker chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// 「全体と同じ」: only offered where there is something to inherit.
    Inherit,
    Kind(ActionKind),
}

/// One heading of the picker with the kinds under it; the section without a group (「なし」) is
/// listed first and has no heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerSection {
    pub group: Option<ActionGroup>,
    pub kinds: Vec<ActionKind>,
}

/// Lower-cased, with katakana turned into hiragana, so a query matches however it was typed.
fn normalize_query(s: &str) -> String {
    s.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// Whether `label` contains `filter` (an empty filter matches everything).
pub fn label_matches(label: &str, filter: &str) -> bool {
    let q = normalize_query(filter);
    q.is_empty() || normalize_query(label).contains(&q)
}

/// The kinds a picker shows, in its order: the kinds without a group first, then one section per
/// group in [`ActionGroup::ALL`] order (kinds keep their order in `kinds`). Kinds whose label does
/// not match `filter` are left out, and so are the sections that end up empty.
pub fn picker_sections(
    kinds: &[ActionKind],
    label: fn(ActionKind) -> &'static str,
    filter: &str,
) -> Vec<PickerSection> {
    let section = |group: Option<ActionGroup>| PickerSection {
        group,
        kinds: kinds
            .iter()
            .filter(|k| k.group() == group)
            .filter(|k| label_matches(label(**k), filter))
            .copied()
            .collect(),
    };
    std::iter::once(None)
        .chain(ActionGroup::ALL.into_iter().map(Some))
        .map(section)
        .filter(|s| !s.kinds.is_empty())
        .collect()
}

/// What the picker's keyboard moves through, top to bottom.
fn picker_flat(inherit: Option<&str>, sections: &[PickerSection], filter: &str) -> Vec<Pick> {
    let mut v = Vec::new();
    if inherit.is_some_and(|l| label_matches(l, filter)) {
        v.push(Pick::Inherit);
    }
    v.extend(
        sections
            .iter()
            .flat_map(|s| s.kinds.iter().map(|k| Pick::Kind(*k))),
    );
    v
}

/// Where the picker stands while its popup is open.
#[derive(Clone, Default)]
struct PickerState {
    filter: String,
    highlight: usize,
    /// The arrow keys were used (so the highlight shows even without a filter).
    moved: bool,
    /// The first frame has passed (the filter box got the focus).
    seen: bool,
    /// The tallest the list has been since the popup opened: the popup keeps at least this height,
    /// as egui's popup scroll area only grows back from what it held last frame.
    list_h: f32,
}

/// Dev screenshots: the first picker drawn opens with this filter text.
struct Preview {
    filter: Option<String>,
    epoch: u32,
    claimed: Option<egui::Id>,
}

static PREVIEW: std::sync::Mutex<Preview> = std::sync::Mutex::new(Preview {
    filter: None,
    epoch: 0,
    claimed: None,
});

/// Open the first picker drawn with `filter` typed in (`None`: no preview). For dev screenshots.
pub fn set_picker_preview(filter: Option<&str>) {
    let mut p = PREVIEW.lock().unwrap_or_else(|e| e.into_inner());
    if p.filter.as_deref() != filter {
        p.filter = filter.map(str::to_owned);
        p.epoch += 1;
        p.claimed = None;
    }
}

/// [`kind_picker`] for a plain list of kinds. Returns the kind picked this frame.
pub fn kind_combo(
    ui: &mut Ui,
    t: &Tokens,
    id: impl Hash + Debug,
    current: ActionKind,
    kinds: &[ActionKind],
    label: fn(ActionKind) -> &'static str,
) -> Option<ActionKind> {
    match kind_picker(
        ui,
        t,
        t.combo_w,
        id,
        label(current),
        Some(current),
        None,
        kinds,
        label,
    ) {
        Some(Pick::Kind(k)) => Some(k),
        _ => None,
    }
}

/// The grouped, filterable chooser of an action kind. `selected_text` is on the closed box and
/// `current` is marked in the list; `inherit` adds a first entry (「全体と同じ」) with that label.
#[allow(clippy::too_many_arguments)]
pub fn kind_picker(
    ui: &mut Ui,
    t: &Tokens,
    width: f32,
    id: impl Hash + Debug,
    selected_text: &str,
    current: Option<ActionKind>,
    inherit: Option<&'static str>,
    kinds: &[ActionKind],
    label: fn(ActionKind) -> &'static str,
) -> Option<Pick> {
    let button_id = ui.make_persistent_id(egui::IdSalt::new(&id));
    let popup_id = button_id.with("popup");
    let state_id = button_id.with("picker-state");
    let ctx = ui.ctx().clone();
    if !egui::ComboBox::is_open(&ctx, button_id) {
        ctx.data_mut(|d| d.remove::<PickerState>(state_id));
    }
    preview_open(&ctx, button_id, popup_id, state_id);

    let mut picked = None;
    // The popup scrolls the list itself (under the filter box), so its own limit stays out of the way.
    let combo_height = std::mem::replace(&mut ui.spacing_mut().combo_height, f32::INFINITY);
    combo_menu_with(
        ui,
        t,
        width,
        &id,
        selected_text,
        egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            let mut st: PickerState = ui.data(|d| d.get_temp(state_id)).unwrap_or_default();
            picked = picker_menu(ui, t, &mut st, current, inherit, kinds, label);
            if picked.is_some() {
                ui.data_mut(|d| d.remove::<PickerState>(state_id));
                ui.close();
            } else {
                ui.data_mut(|d| d.insert_temp(state_id, st));
            }
        },
    );
    ui.spacing_mut().combo_height = combo_height;
    picked
}

/// Dev screenshots: the claimed picker opens (once per preview) with the preview's filter.
fn preview_open(ctx: &egui::Context, button_id: egui::Id, popup_id: egui::Id, state_id: egui::Id) {
    let mut p = PREVIEW.lock().unwrap_or_else(|e| e.into_inner());
    let Some(filter) = p.filter.clone() else {
        return;
    };
    match p.claimed {
        None => p.claimed = Some(button_id),
        Some(c) if c != button_id => return,
        _ => {}
    }
    let epoch_id = state_id.with("epoch");
    if ctx.data(|d| d.get_temp::<u32>(epoch_id)) != Some(p.epoch) {
        ctx.data_mut(|d| {
            d.clear();
            d.insert_temp(epoch_id, p.epoch);
            d.insert_temp(
                state_id,
                PickerState {
                    filter,
                    ..PickerState::default()
                },
            );
        });
        // Dev shots reuse one popup id: forget its remembered size so each starts as a fresh open.
        ctx.memory_mut(|m| m.reset_areas());
        egui::Popup::open_id(ctx, popup_id);
    }
}

/// What Enter picks: the highlighted row, but only when one is drawn highlighted (a filter is typed
/// or the arrow keys were used); on a fresh popup Enter does nothing.
fn enter_pick(flat: &[Pick], highlight: usize, filter: &str, moved: bool) -> Option<Pick> {
    if filter.trim().is_empty() && !moved {
        return None;
    }
    flat.get(highlight).copied()
}

/// The contents of the picker popup. Returns the pick, if one was made.
fn picker_menu(
    ui: &mut Ui,
    t: &Tokens,
    st: &mut PickerState,
    current: Option<ActionKind>,
    inherit: Option<&'static str>,
    kinds: &[ActionKind],
    label: fn(ActionKind) -> &'static str,
) -> Option<Pick> {
    // The popup's scroll area is only as tall as what it held last frame; after a short list (no
    // match) the next one must be free to be taller than that.
    let mut room = ui.max_rect();
    room.max.y = room.min.y + PICKER_MAX_H + 4.0 * t.control_h;
    ui.scope_builder(egui::UiBuilder::new().max_rect(room), |ui| {
        picker_body(ui, t, st, current, inherit, kinds, label)
    })
    .inner
}

fn picker_body(
    ui: &mut Ui,
    t: &Tokens,
    st: &mut PickerState,
    current: Option<ActionKind>,
    inherit: Option<&'static str>,
    kinds: &[ActionKind],
    label: fn(ActionKind) -> &'static str,
) -> Option<Pick> {
    ui.set_min_width(PICKER_MIN_W);
    let with_filter = kinds.len() + usize::from(inherit.is_some()) >= PICKER_FILTER_MIN;
    let mut sections = picker_sections(kinds, label, &st.filter);
    let mut flat = picker_flat(inherit, &sections, &st.filter);
    let mut picked = None;

    if with_filter {
        // Keys first, so the text box never sees them.
        let (down, up, enter) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        if down && !flat.is_empty() {
            st.highlight = (st.highlight + 1).min(flat.len() - 1);
            st.moved = true;
        }
        if up {
            st.highlight = st.highlight.saturating_sub(1);
            st.moved = true;
        }
        if enter {
            picked = enter_pick(&flat, st.highlight, &st.filter, st.moved);
        }
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.filter)
                .hint_text("絞り込み")
                .desired_width(f32::INFINITY),
        );
        if !st.seen {
            r.request_focus();
            st.seen = true;
        }
        if r.changed() {
            st.highlight = 0;
            sections = picker_sections(kinds, label, &st.filter);
            flat = picker_flat(inherit, &sections, &st.filter);
        }
        ui.add_space(t.space_xs);
    }
    st.highlight = st.highlight.min(flat.len().saturating_sub(1));
    let show_highlight = !st.filter.trim().is_empty() || st.moved;

    if flat.is_empty() {
        ui.set_min_height(st.list_h);
        ui.add_space(t.space_sm);
        ui.label(
            RichText::new("該当なし")
                .size(t.fs_small)
                .color(t.text_muted),
        );
        ui.add_space(t.space_sm);
        return picked;
    }

    let mut index = 0usize;
    let scroll_to = st.moved;
    let list = egui::ScrollArea::vertical()
        .max_height(PICKER_MAX_H)
        .min_scrolled_height(st.list_h)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.set_min_height(st.list_h);
            ui.spacing_mut().item_spacing.y = 1.0;
            if let Some(text) = inherit
                && flat.first() == Some(&Pick::Inherit)
            {
                let hl = show_highlight && st.highlight == index;
                if picker_item(ui, t, text, current.is_none(), hl, hl && scroll_to) {
                    picked = Some(Pick::Inherit);
                }
                index += 1;
            }
            for (n, s) in sections.iter().enumerate() {
                if let Some(g) = s.group {
                    ui.add_space(if n == 0 { t.space_xs } else { t.space_md });
                    ui.label(
                        RichText::new(g.label())
                            .size(t.fs_small)
                            .color(t.text_muted),
                    );
                    ui.add_space(t.space_xs);
                }
                for &k in &s.kinds {
                    let hl = show_highlight && st.highlight == index;
                    if picker_item(ui, t, label(k), current == Some(k), hl, hl && scroll_to) {
                        picked = Some(Pick::Kind(k));
                    }
                    index += 1;
                }
            }
        });
    st.list_h = st.list_h.max(list.content_size.y.min(PICKER_MAX_H));
    picked
}

/// One row of the picker: the current one has a bar and a soft fill, the highlighted one (keys, or
/// the first match) a lighter fill. Returns whether it was clicked.
fn picker_item(
    ui: &mut Ui,
    t: &Tokens,
    text: &str,
    current: bool,
    highlighted: bool,
    scroll: bool,
) -> bool {
    let h = t.fs_body + 2.0 * t.space_sm;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    if scroll {
        ui.scroll_to_rect(rect, None);
    }
    let radius = t.radius_sm;
    if current {
        ui.painter().rect_filled(rect, radius, t.accent_soft);
        let bar = Rect::from_center_size(
            Pos2::new(rect.left() + t.stroke_focus, rect.center().y),
            Vec2::new(t.stroke_focus + 1.0, h * 0.5),
        );
        ui.painter().rect_filled(bar, t.stroke_focus, t.accent);
    } else if highlighted || resp.hovered() {
        ui.painter().rect_filled(rect, radius, t.control_hover);
    }
    ui.painter().text(
        Pos2::new(rect.left() + t.space_md, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(t.fs_body),
        t.text,
    );
    resp.widget_info(|| WidgetInfo::selected(Role::MenuItem, true, current, text));
    resp.on_hover_cursor(CursorIcon::PointingHand).clicked()
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
    action_picker_head_with(ui, t, &id, action, kinds, label);
    if let Action::Launch { path, args } = action {
        launch_fields(ui, t, &id, path, args);
    }
}

/// The kind chooser and the short fields of the action; the long fields of a launch action are
/// left to [`launch_fields`], for a row that puts them on a line of their own.
pub fn action_picker_head(
    ui: &mut Ui,
    t: &Tokens,
    id: &(impl Hash + Debug),
    action: &mut Action,
    kinds: &[ActionKind],
) {
    action_picker_head_with(ui, t, id, action, kinds, ActionKind::label);
}

/// [`action_picker_head`] with its own names for the kinds.
pub fn action_picker_head_with(
    ui: &mut Ui,
    t: &Tokens,
    id: &(impl Hash + Debug),
    action: &mut Action,
    kinds: &[ActionKind],
    label: fn(ActionKind) -> &'static str,
) {
    if let Some(kind) = kind_combo(ui, t, id, action.kind(), kinds, label) {
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

/// How long the list of running apps is reused before it is read again.
const RUNNING_TTL_S: f64 = 2.0;

/// The fields of a "launch an app" action on one line: the path (typed, browsed or taken from a
/// running app), and optional arguments.
pub fn launch_fields(
    ui: &mut Ui,
    t: &Tokens,
    id: &(impl Hash + Debug),
    path: &mut String,
    args: &mut String,
) {
    path_field(ui, path, t.text_input_w);
    launch_buttons(ui, t, id, path);
    args_field(ui, args, t.text_input_w * 0.75);
}

/// The same fields as one block for a roomy row: the path (taking the free width), 参照… and the
/// running-apps list on one line, and the arguments below, left-aligned with the path.
pub fn launch_block(
    ui: &mut Ui,
    t: &Tokens,
    id: &(impl Hash + Debug),
    path: &mut String,
    args: &mut String,
) {
    let gap = ui.spacing().item_spacing.x;
    // The browse button is about as wide as its label; the list has the standard combo width.
    let side = 2.0 * gap + t.combo_w + 4.0 * t.fs_body + 2.0 * ui.spacing().button_padding.x;
    let path_w = (ui.available_width() - side).max(t.text_input_w * 0.6);
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            path_field(ui, path, path_w);
            launch_buttons(ui, t, id, path);
        });
        args_field(ui, args, path_w);
    });
}

fn path_field(ui: &mut Ui, path: &mut String, width: f32) {
    ui.add(
        egui::TextEdit::singleline(path)
            .hint_text("アプリ（.exe / .lnk）のパス")
            .desired_width(width),
    );
}

fn args_field(ui: &mut Ui, args: &mut String, width: f32) {
    ui.add(
        egui::TextEdit::singleline(args)
            .hint_text("引数（省略可）")
            .desired_width(width),
    );
}

/// 参照… (a file dialog) and 実行中のアプリから (a list of running apps), both setting `path`.
fn launch_buttons(ui: &mut Ui, t: &Tokens, id: &(impl Hash + Debug), path: &mut String) {
    if button(ui, t, "参照…").clicked()
        && let Ok(Some(p)) = crate::os::file_dialog::open_app()
    {
        *path = p.to_string_lossy().into_owned();
    }
    combo_menu(
        ui,
        t,
        t.combo_w,
        (id, "running"),
        "実行中のアプリから",
        |ui| {
            let now = ui.input(|i| i.time);
            let key = ui.make_persistent_id("yubi-launch-running");
            let cached: Option<(f64, Vec<String>)> = ui.data(|d| d.get_temp(key));
            let list = match cached {
                Some((at, list)) if now - at < RUNNING_TTL_S => list,
                _ => {
                    let list = crate::os::running::running_exe_paths();
                    ui.data_mut(|d| d.insert_temp(key, (now, list.clone())));
                    list
                }
            };
            if list.is_empty() {
                ui.label(RichText::new("ウィンドウのあるアプリがありません").color(t.text_muted));
            }
            for p in list {
                let name = p.rsplit(['\\', '/']).next().unwrap_or(&p).to_owned();
                if ui.selectable_label(false, name).on_hover_text(&p).clicked() {
                    *path = p;
                }
            }
        },
    );
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

#[cfg(test)]
mod picker_tests {
    use super::*;

    #[test]
    fn escape_closes_the_popup_even_with_the_filter_focused() {
        let ctx = egui::Context::default();
        let t = Tokens::dark();
        let mut button = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            button = Some(ui.make_persistent_id(egui::IdSalt::new("k")));
        });
        out.textures_delta.clear();
        let button = button.unwrap();
        egui::Popup::open_id(&ctx, button.with("popup"));
        let screen = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(700.0, 520.0),
        ));
        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: screen,
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                kind_combo(
                    ui,
                    &t,
                    "k",
                    ActionKind::TaskView,
                    &ActionKind::ALL,
                    ActionKind::label,
                );
            });
            out.textures_delta.clear();
        };
        for _ in 0..4 {
            frame(vec![]);
        }
        assert!(egui::ComboBox::is_open(&ctx, button));
        frame(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        frame(vec![]);
        assert!(
            !egui::ComboBox::is_open(&ctx, button),
            "Esc left the popup open"
        );
    }

    #[test]
    fn enter_does_nothing_until_a_row_is_highlighted() {
        let flat = [Pick::Inherit, Pick::Kind(ActionKind::TaskView)];
        assert_eq!(enter_pick(&flat, 0, "", false), None);
        assert_eq!(enter_pick(&flat, 0, "  ", false), None);
        assert_eq!(enter_pick(&flat, 0, "タス", false), Some(Pick::Inherit));
        assert_eq!(enter_pick(&flat, 1, "", true), Some(flat[1]));
    }

    /// Run the picker popup headless and return its height after `frames` frames.
    fn popup_height(ctx: &egui::Context, t: &Tokens, popup_id: egui::Id, frames: usize) -> f32 {
        let mut h = 0.0;
        for _ in 0..frames {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    Vec2::new(700.0, 520.0),
                )),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                kind_combo(
                    ui,
                    t,
                    "k",
                    ActionKind::TaskView,
                    &ActionKind::ALL,
                    ActionKind::label,
                );
            });
            out.textures_delta.clear();
            h = ctx
                .memory(|m| m.area_rect(popup_id))
                .map_or(0.0, |r| r.height());
        }
        h
    }

    #[test]
    fn popup_regrows_after_the_filter_is_cleared() {
        let ctx = egui::Context::default();
        let t = Tokens::dark();
        let mut button = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            button = Some(ui.make_persistent_id(egui::IdSalt::new("k")));
        });
        out.textures_delta.clear();
        let button = button.unwrap();
        let (popup_id, state_id) = (button.with("popup"), button.with("picker-state"));
        egui::Popup::open_id(&ctx, popup_id);
        let full = popup_height(&ctx, &t, popup_id, 6);
        let set = |f: &str| {
            ctx.data_mut(|d| {
                let mut st: PickerState = d.get_temp(state_id).unwrap_or_default();
                st.filter = f.to_owned();
                st.seen = true;
                d.insert_temp(state_id, st);
            })
        };
        set("zzz");
        let none = popup_height(&ctx, &t, popup_id, 6);
        set("");
        let back = popup_height(&ctx, &t, popup_id, 6);
        assert!(none > 40.0, "no-match popup collapsed: {none}");
        assert!(
            (back - full).abs() < 2.0,
            "popup left clipped after clearing the filter: {back} vs {full}"
        );
    }

    #[test]
    fn unfiltered_sections_hold_every_kind_once_in_group_order() {
        let s = picker_sections(&ActionKind::WITH_RESIZE, ActionKind::label, "");
        assert_eq!(s[0].group, None);
        assert_eq!(s[0].kinds, vec![ActionKind::None]);
        let groups: Vec<_> = s.iter().filter_map(|s| s.group).collect();
        assert_eq!(groups, ActionGroup::ALL.to_vec());
        let mut all: Vec<_> = s.iter().flat_map(|s| s.kinds.clone()).collect();
        assert_eq!(all.len(), ActionKind::WITH_RESIZE.len());
        all.sort_by_key(|k| format!("{k:?}"));
        all.dedup();
        assert_eq!(all.len(), ActionKind::WITH_RESIZE.len());
    }

    #[test]
    fn the_kinds_subset_is_kept() {
        let kinds = [ActionKind::None, ActionKind::Find, ActionKind::TaskView];
        let s = picker_sections(&kinds, ActionKind::label, "");
        let flat: Vec<_> = s.iter().flat_map(|s| s.kinds.clone()).collect();
        assert_eq!(
            flat,
            vec![ActionKind::None, ActionKind::TaskView, ActionKind::Find]
        );
    }

    #[test]
    fn filtering_hides_empty_groups_and_ignores_case_and_kana_type() {
        let s = picker_sections(&ActionKind::ALL, ActionKind::label, "デスク");
        assert!(s.iter().all(|s| s.group != Some(ActionGroup::Basic)));
        let flat: Vec<_> = s.iter().flat_map(|s| s.kinds.clone()).collect();
        assert!(flat.contains(&ActionKind::ShowDesktop));
        assert!(flat.contains(&ActionKind::DesktopNext));
        assert!(!flat.contains(&ActionKind::Find));
        // Hiragana matches katakana labels, and ASCII ignores case.
        assert_eq!(
            picker_sections(&ActionKind::ALL, ActionKind::label, "ですく"),
            s
        );
        let alt = picker_sections(&ActionKind::ALL, ActionKind::label, "ALT+tab");
        assert_eq!(alt.len(), 1);
        assert_eq!(alt[0].kinds, vec![ActionKind::AppSwitch]);
    }

    #[test]
    fn no_match_gives_no_sections() {
        assert!(picker_sections(&ActionKind::ALL, ActionKind::label, "zzz").is_empty());
        assert!(label_matches("なし", "  "));
    }

    #[test]
    fn flat_order_puts_the_inherit_entry_first() {
        let s = picker_sections(&ActionKind::ALL, ActionKind::label, "");
        let flat = picker_flat(Some("全体と同じ"), &s, "");
        assert_eq!(flat[0], Pick::Inherit);
        assert_eq!(flat[1], Pick::Kind(ActionKind::None));
        let none = picker_flat(Some("全体と同じ"), &[], "zzz");
        assert!(none.is_empty());
    }
}
