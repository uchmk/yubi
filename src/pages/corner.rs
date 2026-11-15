//! コーナー操作: each pad corner has three swipe slots (horizontal, vertical, diagonal). A wide
//! window shows the pad as a diagram with a clickable pill at the end of each of the 12 arrows;
//! a narrow one falls back to four cards with three rows each.

use eframe::egui::{
    self, FontId, PopupCloseBehavior, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2,
    text::LayoutJob,
};

use crate::config::{Action, ActionKind, Config, CornerDir, Corners};
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{SliderSpec, action_fields, action_picker_with, group, section_header};

use super::features::feature_top;

/// Nominal Magic Trackpad size (mm) the diagram is drawn for; only the proportions matter.
const PAD_MM: (f32, f32) = (160.0, 115.0);
/// Content width under which the card list is used instead of the diagram.
const LIST_BELOW_W: f32 = 640.0;
const DIAGRAM_MAX_W: f32 = 560.0;
const PILL_W: f32 = 130.0;
const PILL_H: f32 = 28.0;

pub fn corner(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    let on = feature_top(ui, t, cfg, Page::Corner);
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "使い方");
        group(ui, t, |g| {
            g.wrapped_row(|ui| {
                ui.vertical(|ui| {
                    ui.label("角から指 1 本で、端に沿って、または中心へ向かってなぞります。");
                    ui.label(
                        RichText::new(format!(
                            "指を角の中（{:.0} mm 四方）に置き、すぐ（約 0.4 秒以内）に 6 mm ほどなぞると有効になり、カーソルは止まります。\
                             ゆっくり動かしたときや、角の外から始めたときは、ふつうのカーソル操作です。",
                            cfg.corner.size_mm
                        ))
                        .size(t.fs_small)
                        .color(t.text_muted),
                    );
                });
            });
        });
        section_header(ui, t, "割り当て");
        if ui.available_width() >= LIST_BELOW_W {
            group(ui, t, |g| g.wrapped_row(|ui| diagram(ui, t, &mut cfg.corner)));
        } else {
            cards(ui, t, &mut cfg.corner);
        }
        ui.label(
            RichText::new(
                "「斜め」では「反対側の角でウィンドウサイズを変更」も選べます。指を離すまで、前面のウィンドウが指の動きに合わせて大きさを変えます。",
            )
            .size(t.fs_small)
            .color(t.text_muted),
        );
        ui.add_space(t.space_sm);
        section_header(ui, t, "反応する範囲");
        group(ui, t, |g| {
            g.slider_row(
                "角の大きさ",
                "この四角の中から始めた操作だけが、コーナー操作になります。操作を割り当てた角では、端のスライダーは始まりません。",
                SliderSpec {
                    min: f64::from(Corners::SIZE.min),
                    max: f64::from(Corners::SIZE.max),
                    step: 1.0,
                    decimals: 0,
                    suffix: " mm",
                },
                &mut cfg.corner.size_mm,
            );
        });
    });
}

/// "オフ" instead of "なし": a slot that does nothing is off.
fn kind_label(k: ActionKind) -> &'static str {
    if k == ActionKind::None {
        "オフ"
    } else {
        k.label()
    }
}

/// The kinds a slot offers: the window resize only makes sense for the diagonal swipe, which
/// keeps going toward the middle of the pad.
fn kinds_for(dir: CornerDir) -> &'static [ActionKind] {
    if dir == CornerDir::Diagonal {
        &ActionKind::WITH_RESIZE
    } else {
        &ActionKind::ALL
    }
}

/// Text on a pill and in the tooltip.
fn pill_text(a: &Action) -> String {
    match a {
        Action::None => "オフ".to_owned(),
        Action::ResizeFromOpposite => "サイズ変更".to_owned(),
        other => other.label(),
    }
}

fn full_text(a: &Action) -> String {
    match a {
        Action::None => "オフ".to_owned(),
        other => other.label(),
    }
}

// ---------------------------------------------------------------------------------------------
// Narrow: four cards of three rows

fn cards(ui: &mut Ui, t: &Tokens, corners: &mut Corners) {
    for (i, name) in Corners::NAMES.iter().enumerate() {
        ui.label(
            RichText::new(*name)
                .size(t.fs_body)
                .strong()
                .color(t.text_muted),
        );
        group(ui, t, |g| {
            let c = &mut corners.corners[i];
            for dir in CornerDir::ALL {
                let (title, desc) = match dir {
                    CornerDir::Horizontal => ("横", "角から、上下の辺に沿ってなぞります。"),
                    CornerDir::Vertical => ("縦", "角から、左右の辺に沿ってなぞります。"),
                    CornerDir::Diagonal => ("斜め", "角から、中心へ向かってなぞります。"),
                };
                let slot = c.slot_mut(dir);
                g.row(None, title, desc, |ui| {
                    action_picker_with(
                        ui,
                        t,
                        ("corner-card", i, dir.token()),
                        slot,
                        kinds_for(dir),
                        kind_label,
                    );
                });
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Wide: the pad diagram

fn diagram(ui: &mut Ui, t: &Tokens, corners: &mut Corners) {
    let avail = ui.available_width();
    let w = avail.min(DIAGRAM_MAX_W);
    let scale = w / PAD_MM.0;
    let (outer, _) = ui.allocate_exact_size(Vec2::new(avail, PAD_MM.1 * scale), Sense::hover());
    let rect = Rect::from_min_size(
        Pos2::new(outer.left() + (avail - w) / 2.0, outer.top()),
        Vec2::new(w, PAD_MM.1 * scale),
    );
    let p = ui.painter_at(rect.expand(1.0));
    p.rect_filled(rect, t.radius_lg as f32, t.control);
    p.rect_stroke(
        rect,
        t.radius_lg as f32,
        Stroke::new(t.stroke_thin, t.border),
        egui::StrokeKind::Inside,
    );
    let zone = (corners.size_mm * scale).clamp(8.0, w / 3.0);
    // Long enough to read as an arrow, short enough that the two pills along an edge never touch.
    let arm = (w / 2.0 - zone / 2.0 - PILL_W - 14.0).clamp(30.0, w * 0.2);
    for (i, name) in Corners::NAMES.iter().enumerate() {
        let (sx, sy) = sign_of(i);
        let origin = Pos2::new(
            if sx > 0.0 { rect.left() } else { rect.right() },
            if sy > 0.0 { rect.top() } else { rect.bottom() },
        );
        let zone_rect = Rect::from_two_pos(origin, origin + Vec2::new(sx * zone, sy * zone));
        let used = corners.corners[i].any();
        p.rect_filled(
            zone_rect,
            t.radius_md as f32,
            if used { t.accent_soft } else { t.control_hover },
        );
        p.rect_stroke(
            zone_rect,
            t.radius_md as f32,
            Stroke::new(t.stroke_thin, if used { t.accent } else { t.border }),
            egui::StrokeKind::Inside,
        );
        let start = zone_rect.center();
        for dir in CornerDir::ALL {
            let reach = match dir {
                CornerDir::Diagonal => arm * 0.8,
                _ => arm,
            };
            let (dx, dy) = match dir {
                CornerDir::Horizontal => (sx * reach, 0.0),
                CornerDir::Vertical => (0.0, sy * reach),
                CornerDir::Diagonal => (sx * reach, sy * reach),
            };
            let tip = start + Vec2::new(dx, dy);
            let pill = pill_rect(rect, dir, tip, sx, sy);
            let slot = corners.corners[i].slot_mut(dir);
            let set = *slot != Action::None;
            let color = if set {
                t.accent
            } else {
                t.text_muted.gamma_multiply(0.45)
            };
            arrow(&p, start, tip, Stroke::new(t.stroke_thin * 1.5, color));
            slot_pill(ui, t, i, name, dir, pill, slot);
        }
    }
}

/// A line with a small fixed-size head (the egui arrow scales its head with the length).
fn arrow(p: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    let dir = (to - from).normalized();
    let side = Vec2::new(-dir.y, dir.x);
    let head = 7.0;
    p.line_segment([from, to], stroke);
    p.line_segment([to, to - dir * head + side * head * 0.6], stroke);
    p.line_segment([to, to - dir * head - side * head * 0.6], stroke);
}

fn sign_of(corner: usize) -> (f32, f32) {
    (
        if corner.is_multiple_of(2) { 1.0 } else { -1.0 },
        if corner < 2 { 1.0 } else { -1.0 },
    )
}

/// Where the pill of the arrow ending at `tip` sits: beyond the tip for the arrows that run
/// along the top/bottom edge or diagonally, below/above it (against the pad's side) for the
/// one that runs along the side edge.
fn pill_rect(pad: Rect, dir: CornerDir, tip: Pos2, sx: f32, sy: f32) -> Rect {
    let size = Vec2::new(PILL_W, PILL_H);
    let gap = 6.0;
    let min = match dir {
        CornerDir::Horizontal | CornerDir::Diagonal => {
            let x = if sx > 0.0 {
                tip.x + gap
            } else {
                tip.x - gap - size.x
            };
            Pos2::new(x, tip.y - size.y / 2.0)
        }
        CornerDir::Vertical => {
            let x = if sx > 0.0 {
                pad.left() + 8.0
            } else {
                pad.right() - 8.0 - size.x
            };
            let y = if sy > 0.0 {
                tip.y + gap
            } else {
                tip.y - gap - size.y
            };
            Pos2::new(x, y)
        }
    };
    Rect::from_min_size(min, size)
}

/// One clickable pill: filled with the accent when the slot has an action, dashed and dim when
/// it is off. A click opens a menu with the same choices as the combo box of the cards.
fn slot_pill(
    ui: &mut Ui,
    t: &Tokens,
    corner: usize,
    corner_name: &str,
    dir: CornerDir,
    rect: Rect,
    slot: &mut Action,
) {
    let id = ui.auto_id_with(("corner-pill", corner, dir.token()));
    let resp = ui.interact(rect, id, Sense::click());
    let set = *slot != Action::None;
    let hovered = resp.hovered() || resp.has_focus();
    let radius = t.radius_md as f32;
    let p = ui.painter();
    let text_color = if set { t.on_accent } else { t.text_muted };
    if set {
        p.rect_filled(rect, radius, t.accent);
        if hovered {
            p.rect_stroke(
                rect,
                radius,
                Stroke::new(t.stroke_focus, t.text),
                egui::StrokeKind::Outside,
            );
        }
    } else {
        p.rect_filled(rect, radius, if hovered { t.control_hover } else { t.card });
        let stroke = Stroke::new(
            t.stroke_thin,
            if hovered {
                t.accent
            } else {
                t.text_muted.gamma_multiply(0.5)
            },
        );
        let r = rect.shrink(t.stroke_thin / 2.0);
        let path = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        p.extend(egui::Shape::dashed_line(&path, stroke, 4.0, 3.0));
    }
    let mut job = LayoutJob::simple_singleline(
        pill_text(slot),
        FontId::proportional(t.fs_small),
        text_color,
    );
    job.wrap.max_width = rect.width() - 2.0 * t.space_sm;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = p.layout_job(job);
    let pos = rect.center() - galley.size() / 2.0;
    p.galley(pos, galley, text_color);
    let name = format!("{corner_name}・{}", dir.label());
    let tip = format!("{name}：{}", full_text(slot));
    let resp = resp.on_hover_text(tip.clone());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::accesskit::Role::Button, ui.is_enabled(), tip.clone())
    });
    egui::Popup::menu(&resp)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(t.combo_w);
            ui.label(RichText::new(name).size(t.fs_small).color(t.text_muted));
            let current = slot.kind();
            for &kind in kinds_for(dir) {
                if ui
                    .selectable_label(current == kind, kind_label(kind))
                    .clicked()
                {
                    slot.set_kind(kind);
                    // A kind with a field (keys, step) stays open so the field can be filled in.
                    if !matches!(
                        slot,
                        Action::Keys(_)
                            | Action::VolumeStep(_)
                            | Action::BrightnessStep(_)
                            | Action::Launch { .. }
                    ) {
                        ui.close();
                    }
                }
            }
            if let Action::Launch { path, args } = slot {
                crate::widgets::launch_fields(ui, t, &resp.id, path, args);
            } else {
                action_fields(ui, t, slot);
            }
        });
}
