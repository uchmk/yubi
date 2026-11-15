//! コーナー操作: four triggers (タップ / スワイプ / プレス / ホールド) on the four pad corners. Each
//! trigger has its own tab: a how-to animation with its steps, the switch that turns the trigger on,
//! then the pad diagram (left) with the detail panel of the selected slot (right; below the diagram
//! in a narrow window).

use std::sync::Mutex;

use eframe::egui::{
    self, Align2, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2, text::LayoutJob,
};

use crate::config::{Action, ActionKind, Config, CornerDir, CornerSlot, CornerTrigger, Corners};
use crate::howto::{self, Mini};
use crate::howto_scenes;
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{self, SliderSpec, action_fields, group, launch_block, section_header};

use super::features::feature_top;

/// Nominal Magic Trackpad size (mm) the diagram is drawn for; only the proportions matter.
const PAD_MM: (f32, f32) = (160.0, 115.0);
/// Content width from which the panel sits right of the diagram.
const SIDE_BY_SIDE_W: f32 = 760.0;
const DIAGRAM_MAX_W: f32 = 520.0;
const PILL_H: f32 = 28.0;

/// The tab and the selected slot, kept between frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct View {
    tab: usize,
    selected: Option<(usize, CornerSlot)>,
}

fn view_id(ui: &Ui) -> Id {
    ui.make_persistent_id("yubi-corner-view")
}

/// Development aid (devshots): pin the tab and the selection.
static FORCE: Mutex<Option<View>> = Mutex::new(None);

pub fn force_view(tab: CornerTrigger, selected: Option<(usize, CornerSlot)>) {
    let tab = CornerTrigger::ALL
        .iter()
        .position(|t| *t == tab)
        .unwrap_or(0);
    if let Ok(mut f) = FORCE.lock() {
        *f = Some(View { tab, selected });
    }
}

fn load_view(ui: &Ui) -> View {
    if let Ok(f) = FORCE.lock()
        && let Some(v) = *f
    {
        return v;
    }
    ui.data(|d| d.get_temp(view_id(ui))).unwrap_or_default()
}

fn store_view(ui: &Ui, v: View) {
    ui.data_mut(|d| d.insert_temp(view_id(ui), v));
    // A pinned view follows the user too, so it can still be clicked around in.
    if let Ok(mut f) = FORCE.lock()
        && f.is_some()
    {
        *f = Some(v);
    }
}

pub fn corner(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    let on = feature_top(ui, t, cfg, Page::Corner);
    let mut view = load_view(ui);
    let before = view;
    ui.add_enabled_ui(on, |ui| {
        ui.add_space(t.space_sm);
        let labels = CornerTrigger::ALL.map(CornerTrigger::label);
        segmented_tabs(ui, t, "corner-tab", &labels, &mut view.tab);
        let tr = CornerTrigger::ALL[view.tab.min(3)];
        // The selection belongs to the tab it was made on.
        if view.selected.is_some_and(|(_, s)| s.trigger() != tr) {
            view.selected = None;
        }
        section_header(ui, t, "使い方");
        group(ui, t, |g| g.wrapped_row(|ui| guide(ui, t, cfg, tr)));
        ui.add_space(t.space_md);
        group(ui, t, |g| {
            g.toggle_row(
                None,
                &format!("四隅の{}を使う", tr.label()),
                "オフのあいだは、どの角でも、この操作に反応しません。",
                cfg.corner.trigger_enabled_mut(tr),
            );
            g.wrapped_row(|ui| {
                ui.vertical(|ui| {
                    for (i, line) in explanation(tr, cfg.sensitivity.tap_ms, cfg.sensitivity.hold_ms)
                        .into_iter()
                        .enumerate()
                    {
                        ui.label(RichText::new(line).size(t.fs_small).color(if i == 0 {
                            t.text
                        } else {
                            t.text_muted
                        }));
                    }
                });
            });
        });
        section_header(ui, t, "割り当て");
        let active = cfg.corner.trigger_enabled(tr);
        ui.add_enabled_ui(active, |ui| {
            layout(ui, t, cfg, tr, &mut view);
        });
        if !active {
            ui.add_space(t.space_xs);
            ui.label(
                RichText::new(format!(
                    "「四隅の{}を使う」がオフのため、上の割り当ては使われません。",
                    tr.label()
                ))
                .size(t.fs_small)
                .color(t.text_muted),
            );
        }
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
    if view != before {
        store_view(ui, view);
    }
}

/// Segmented control over `labels` (`salt` keeps the ids of two controls apart).
pub(super) fn segmented_tabs(
    ui: &mut Ui,
    t: &Tokens,
    salt: &'static str,
    labels: &[&str],
    tab: &mut usize,
) {
    let avail = ui.available_width();
    let seg_w = (avail / labels.len() as f32).clamp(64.0, 112.0);
    let size = Vec2::new(seg_w * labels.len() as f32, t.control_h + 4.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let radius = f32::from(t.radius_md);
    ui.painter().rect_filled(rect, radius, t.control);
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(t.stroke_thin, t.border),
        egui::StrokeKind::Inside,
    );
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(
            Pos2::new(rect.left() + seg_w * i as f32, rect.top()),
            Vec2::new(seg_w, rect.height()),
        )
        .shrink(2.0);
        let resp = ui.interact(r, ui.auto_id_with((salt, i)), Sense::click());
        if resp.clicked() {
            *tab = i;
        }
        let selected = *tab == i;
        let p = ui.painter();
        if selected {
            p.rect_filled(r, radius - 1.0, t.accent_soft);
            p.rect_stroke(
                r,
                radius - 1.0,
                Stroke::new(t.stroke_thin, t.accent),
                egui::StrokeKind::Inside,
            );
        } else if resp.hovered() {
            p.rect_filled(r, radius - 1.0, t.control_hover);
        }
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            label,
            if selected {
                t.font_semibold(t.fs_body)
            } else {
                FontId::proportional(t.fs_body)
            },
            if selected { t.text } else { t.text_muted },
        );
        let name = format!("{label}のタブ");
        let enabled = ui.is_enabled();
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::accesskit::Role::Tab, enabled, selected, name.clone())
        });
    }
}

/// What counts as this trigger, what happens to normal input in a set corner, and the limits.
fn explanation(tr: CornerTrigger, tap_ms: u16, hold_ms: u16) -> Vec<String> {
    match tr {
        CornerTrigger::Tap => vec![
            format!(
                "角の中に指 1 本で触れ、動かさずにすぐ（約 {:.1} 秒以内に）離すとタップです。",
                f32::from(tap_ms) / 1000.0
            ),
            "タップを割り当てた角では、その角のタップによるクリックと、トラックパッドを押し込む通常のクリックは行われません（その角だけ）。割り当てていない角は、ふつうに動きます。".to_owned(),
            "タップによるクリックを止められるかは、トラックパッドのドライバの動きに左右されます。環境によっては、通常のクリックが先に届くことがあります。".to_owned(),
        ],
        CornerTrigger::Swipe => vec![
            "角の中に指 1 本を置き、すぐ（約 0.4 秒以内に）6 mm ほどなぞるとスワイプです。".to_owned(),
            "操作を割り当てていない方向は、ふつうのカーソル操作のままです。割り当てた方向では、なぞり始めるとカーソルが止まります。".to_owned(),
            "「斜め」で「反対側の角でウィンドウサイズを変更」を選ぶと、指を離すまで、前面のウィンドウが指の動きに合わせて大きさを変えます。".to_owned(),
        ],
        CornerTrigger::Press => vec![
            "角の中で、トラックパッドを押し込むとプレスです（物理クリック、または「押し込みと触覚」の強さを超えたとき）。".to_owned(),
            "プレスを割り当てた角では、押し込んでも通常のクリックは行われません（その角だけ）。押し込みの強さによる操作には、クリックを止める働きはありません。".to_owned(),
        ],
        CornerTrigger::Hold => vec![
            format!("角の中に指 1 本を、動かさずに {hold_ms} ms 置き続けるとホールドです。"),
            "ホールドを割り当てた角では、その角の 1 本指の長押しは動きません。時間は「長押し」ページと同じ値です。".to_owned(),
            "2 本以上の指では動きません。".to_owned(),
        ],
    }
}

/// The how-to for one trigger: the corner animation and its three steps.
fn guide(ui: &mut Ui, t: &Tokens, cfg: &Config, tr: CornerTrigger) {
    let size = cfg.corner.size_mm;
    let (summary, detail, second) = match tr {
        CornerTrigger::Tap => (
            "角にちょんと触れて、すぐ離します",
            "動かさずに、すばやく離すのがコツです。",
            "すぐ離す",
        ),
        CornerTrigger::Swipe => (
            "角から外へ、すばやくなぞります",
            "横・縦・斜めの 3 方向に、それぞれ操作を割り当てられます。",
            "外へなぞる",
        ),
        CornerTrigger::Press => (
            "角で、トラックパッドを押し込みます",
            "押し込むと、通常のクリックの代わりに操作が実行されます。",
            "押し込む",
        ),
        CornerTrigger::Hold => (
            "角に指を置いたまま、動かさずに待ちます",
            "時間がたつと、指を離さなくても操作が実行されます。",
            "動かさずに待つ",
        ),
    };
    ui.vertical(|ui| {
        let anim = howto::Anim::new(ui);
        let step = std::cell::Cell::new(0);
        howto::two_col(
            ui,
            t,
            howto_scenes::corner_scene_width(ui.available_width()),
            |ui| step.set(howto_scenes::corner_scene(ui, t, anim, Some(tr), size)),
            |ui| {
                howto::summary(ui, t, summary, &[detail.to_owned()]);
                ui.add_space(t.space_md);
                howto::steps(
                    ui,
                    t,
                    &[Mini::CornerZone, Mini::Corner(tr), Mini::Run(1)],
                    &["指を角に置く", second, "実行される"],
                    anim.live().then_some(step.get()),
                );
            },
        );
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

/// The kinds a swipe slot offers: the window resize only makes sense for the diagonal swipe, which
/// keeps going toward the middle of the pad.
fn kinds_for(dir: CornerDir) -> &'static [ActionKind] {
    if dir == CornerDir::Diagonal {
        &ActionKind::WITH_RESIZE
    } else {
        &ActionKind::ALL
    }
}

/// Text on a pill.
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
// Diagram and panel

/// The diagram (left) and the detail panel of the selected slot (right, or below).
fn layout(ui: &mut Ui, t: &Tokens, cfg: &mut Config, tr: CornerTrigger, view: &mut View) {
    let avail = ui.available_width();
    if avail >= SIDE_BY_SIDE_W {
        let left_w = (avail * 0.56).clamp(360.0, DIAGRAM_MAX_W + 40.0);
        let gap = t.space_lg;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(left_w);
                group(ui, t, |g| {
                    g.wrapped_row(|ui| diagram(ui, t, &cfg.corner, tr, view));
                });
            });
            ui.add_space(gap - ui.spacing().item_spacing.x);
            ui.vertical(|ui| {
                ui.set_width((avail - left_w - gap).max(260.0));
                panel(ui, t, cfg, tr, view);
            });
        });
    } else {
        group(ui, t, |g| {
            g.wrapped_row(|ui| diagram(ui, t, &cfg.corner, tr, view));
        });
        ui.add_space(t.space_md);
        panel(ui, t, cfg, tr, view);
    }
}

/// The slots of `tr` for one corner, in diagram order.
fn slots_of(tr: CornerTrigger) -> &'static [CornerSlot] {
    match tr {
        CornerTrigger::Swipe => &[
            CornerSlot::Swipe(CornerDir::Horizontal),
            CornerSlot::Swipe(CornerDir::Vertical),
            CornerSlot::Swipe(CornerDir::Diagonal),
        ],
        CornerTrigger::Tap => &[CornerSlot::Tap],
        CornerTrigger::Press => &[CornerSlot::Press],
        CornerTrigger::Hold => &[CornerSlot::Hold],
    }
}

/// The kinds a slot offers.
fn kinds_of(slot: CornerSlot) -> &'static [ActionKind] {
    match slot {
        CornerSlot::Swipe(d) => kinds_for(d),
        _ => &ActionKind::ALL,
    }
}

fn slot_title(corner: usize, slot: CornerSlot) -> String {
    format!("{}・{}", Corners::NAMES[corner], slot.label())
}

fn diagram(ui: &mut Ui, t: &Tokens, corners: &Corners, tr: CornerTrigger, view: &mut View) {
    let avail = ui.available_width();
    let w = avail.min(DIAGRAM_MAX_W);
    let scale = w / PAD_MM.0;
    let pill_w = (w * 0.27).clamp(80.0, 130.0);
    let (outer, _) = ui.allocate_exact_size(Vec2::new(avail, PAD_MM.1 * scale), Sense::hover());
    let rect = Rect::from_min_size(
        Pos2::new(outer.left() + (avail - w) / 2.0, outer.top()),
        Vec2::new(w, PAD_MM.1 * scale),
    );
    let p = ui.painter_at(rect.expand(1.0));
    p.rect_filled(rect, f32::from(t.radius_lg), t.control);
    p.rect_stroke(
        rect,
        f32::from(t.radius_lg),
        Stroke::new(t.stroke_thin, t.border),
        egui::StrokeKind::Inside,
    );
    let zone = (corners.size_mm * scale).clamp(8.0, w / 3.0);
    // Long enough to read as an arrow, short enough that the two pills along an edge never touch.
    let arm = (w / 2.0 - zone / 2.0 - pill_w - 14.0).clamp(30.0, w * 0.2);
    for i in 0..4 {
        let (sx, sy) = sign_of(i);
        let origin = Pos2::new(
            if sx > 0.0 { rect.left() } else { rect.right() },
            if sy > 0.0 { rect.top() } else { rect.bottom() },
        );
        let zone_rect = Rect::from_two_pos(origin, origin + Vec2::new(sx * zone, sy * zone));
        let used = corners.corners[i].any_of(tr);
        p.rect_filled(
            zone_rect,
            f32::from(t.radius_md),
            if used { t.accent_soft } else { t.control_hover },
        );
        p.rect_stroke(
            zone_rect,
            f32::from(t.radius_md),
            Stroke::new(t.stroke_thin, if used { t.accent } else { t.border }),
            egui::StrokeKind::Inside,
        );
        let start = zone_rect.center();
        for &slot in slots_of(tr) {
            let (pill, tip) = match slot {
                CornerSlot::Swipe(dir) => {
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
                    (pill_rect(rect, dir, tip, sx, sy, pill_w), Some(tip))
                }
                _ => {
                    // Beside the zone, towards the middle of the pad.
                    let x = if sx > 0.0 {
                        zone_rect.right() + 8.0
                    } else {
                        zone_rect.left() - 8.0 - pill_w
                    };
                    (
                        Rect::from_min_size(
                            Pos2::new(x, start.y - PILL_H / 2.0),
                            Vec2::new(pill_w, PILL_H),
                        ),
                        None,
                    )
                }
            };
            let action = corners.corners[i].get(slot);
            if let Some(tip) = tip {
                let color = if *action != Action::None {
                    t.accent
                } else {
                    t.text_muted.gamma_multiply(0.45)
                };
                arrow(&p, start, tip, Stroke::new(t.stroke_thin * 1.5, color));
            }
            let selected = view.selected == Some((i, slot));
            if slot_pill(ui, t, i, slot, pill, action, selected).clicked() {
                view.selected = if selected { None } else { Some((i, slot)) };
            }
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
fn pill_rect(pad: Rect, dir: CornerDir, tip: Pos2, sx: f32, sy: f32, pill_w: f32) -> Rect {
    let size = Vec2::new(pill_w, PILL_H);
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
/// it is off. A click selects the slot; the panel shows and edits it.
fn slot_pill(
    ui: &mut Ui,
    t: &Tokens,
    corner: usize,
    slot: CornerSlot,
    rect: Rect,
    action: &Action,
    selected: bool,
) -> egui::Response {
    let id = ui.auto_id_with(("corner-pill", corner, slot.token()));
    let resp = ui.interact(rect, id, Sense::click());
    let set = *action != Action::None;
    let hovered = resp.hovered() || resp.has_focus();
    let radius = f32::from(t.radius_md);
    let p = ui.painter();
    let text_color = if set { t.on_accent } else { t.text_muted };
    if set {
        p.rect_filled(rect, radius, t.accent);
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
    if selected || (set && hovered) {
        p.rect_stroke(
            rect,
            radius,
            Stroke::new(t.stroke_focus, if selected { t.text } else { t.border }),
            egui::StrokeKind::Outside,
        );
    }
    let mut job = LayoutJob::simple_singleline(
        pill_text(action),
        FontId::proportional(t.fs_small),
        text_color,
    );
    job.wrap.max_width = rect.width() - 2.0 * t.space_sm;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = p.layout_job(job);
    let pos = rect.center() - galley.size() / 2.0;
    p.galley(pos, galley, text_color);
    let tip = format!("{}：{}", slot_title(corner, slot), full_text(action));
    let enabled = ui.is_enabled();
    let resp = resp.on_hover_text(tip.clone());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::accesskit::Role::Button,
            enabled,
            selected,
            tip.clone(),
        )
    });
    resp
}

/// The detail panel: the selected slot's switch and action, or a list of what is set.
fn panel(ui: &mut Ui, t: &Tokens, cfg: &mut Config, tr: CornerTrigger, view: &mut View) {
    let Some((corner, slot)) = view.selected else {
        panel_list(ui, t, cfg, tr, view);
        return;
    };
    let master = cfg.corner.enabled;
    let trigger_on = cfg.corner.trigger_enabled(tr);
    let last_id = ui.make_persistent_id(("yubi-corner-last", corner, slot.token()));
    let last: Option<Action> = ui.data(|d| d.get_temp(last_id));
    let action = cfg.corner.corners[corner].get_mut(slot);
    ui.horizontal(|ui| {
        ui.label(RichText::new(slot_title(corner, slot)).font(t.font_semibold(t.fs_section)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::button(ui, t, "一覧に戻る").clicked() {
                view.selected = None;
            }
        });
    });
    ui.add_space(t.space_sm);
    let ctx = ui.ctx().clone();
    group(ui, t, |g| {
        // Remembered action: turning the slot off keeps it, turning it on brings it back.
        let mut on = *action != Action::None;
        let saved = last.clone().filter(|a| *a != Action::None);
        let desc = match (&saved, on) {
            (Some(a), false) => format!("保存した操作: {}", a.label()),
            _ => "オフのあいだ、この項目は何もしません。".to_owned(),
        };
        let was_on = on;
        g.toggle_row(
            None,
            &format!("{}を使う", slot_title(corner, slot)),
            &desc,
            &mut on,
        );
        if on != was_on {
            if on {
                *action = saved.unwrap_or_else(|| default_action(slot));
            } else {
                let keep = action.clone();
                ctx.data_mut(|d| d.insert_temp(last_id, keep));
                *action = Action::None;
            }
        }
        if *action != Action::None {
            g.row(None, "割り当てる操作", "", |ui| {
                let mut kind = action.kind();
                if widgets::combo(
                    ui,
                    t,
                    ("corner-kind", corner, slot.token()),
                    &mut kind,
                    kinds_of(slot),
                    kind_label,
                ) {
                    if kind == ActionKind::None {
                        let keep = action.clone();
                        ui.data_mut(|d| d.insert_temp(last_id, keep));
                    }
                    action.set_kind(kind);
                }
                action_fields(ui, t, action);
            });
            if let Action::Launch { path, args } = action {
                g.raw_row(|ui| {
                    launch_block(ui, t, &("corner-launch", corner, slot.token()), path, args);
                });
            }
        }
        g.info_row("動作状態", &status(master, trigger_on, tr, action));
    });
    if let Some(note) = slot_note(slot) {
        ui.add_space(t.space_xs);
        ui.label(RichText::new(note).size(t.fs_small).color(t.text_muted));
    }
}

/// Nothing selected: a hint and the slots of this trigger that are set.
fn panel_list(ui: &mut Ui, t: &Tokens, cfg: &Config, tr: CornerTrigger, view: &mut View) {
    ui.label(RichText::new("設定する項目").font(t.font_semibold(t.fs_section)));
    ui.add_space(t.space_sm);
    group(ui, t, |g| {
        g.wrapped_row(|ui| {
            ui.label(
                RichText::new("図の項目を選ぶと、ここで操作を割り当てられます。")
                    .size(t.fs_small)
                    .color(t.text_muted),
            );
        });
        let mut any = false;
        for i in 0..4 {
            for &slot in slots_of(tr) {
                let a = cfg.corner.corners[i].get(slot);
                if *a == Action::None {
                    continue;
                }
                any = true;
                let title = slot_title(i, slot);
                g.row(None, &title, "", |ui| {
                    if ui
                        .link(RichText::new(a.label()).size(t.fs_body))
                        .on_hover_text("この項目を開く")
                        .clicked()
                    {
                        view.selected = Some((i, slot));
                    }
                });
            }
        }
        if !any {
            g.wrapped_row(|ui| {
                ui.label(
                    RichText::new("まだ操作を割り当てた項目はありません。")
                        .size(t.fs_small)
                        .color(t.text_muted),
                );
            });
        }
    });
}

/// The first real action, for a slot switched on without a remembered one.
fn default_action(slot: CornerSlot) -> Action {
    let kind = kinds_of(slot)
        .iter()
        .copied()
        .find(|k| *k != ActionKind::None)
        .unwrap_or(ActionKind::TaskView);
    let mut a = Action::None;
    a.set_kind(kind);
    a
}

/// The "動作状態" line of the panel.
fn status(master: bool, trigger_on: bool, tr: CornerTrigger, action: &Action) -> String {
    if !master {
        "コーナー操作がオフです".to_owned()
    } else if !trigger_on {
        format!("四隅の{}がオフです", tr.label())
    } else if *action == Action::None {
        "未割り当て".to_owned()
    } else {
        format!("有効: {}", action.label())
    }
}

/// A caveat shown under the panel for the slots whose input is shadowed.
fn slot_note(slot: CornerSlot) -> Option<&'static str> {
    match slot {
        CornerSlot::Tap => Some(
            "割り当てると、この角のタップによるクリックと物理クリックは行われません。タップによるクリックを止める働きは、ドライバの動きによっては効かないことがあります。",
        ),
        CornerSlot::Press => Some("割り当てると、この角の物理クリックは行われません。"),
        CornerSlot::Hold => Some("割り当てると、この角では 1 本指の長押しが動きません。"),
        CornerSlot::Swipe(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_trigger_has_its_slots_and_they_belong_to_it() {
        for tr in CornerTrigger::ALL {
            assert!(!slots_of(tr).is_empty());
            for s in slots_of(tr) {
                assert_eq!(s.trigger(), tr);
            }
        }
        assert_eq!(slots_of(CornerTrigger::Swipe).len(), 3);
    }

    #[test]
    fn only_the_diagonal_swipe_offers_the_resize() {
        assert!(
            kinds_of(CornerSlot::Swipe(CornerDir::Diagonal))
                .contains(&ActionKind::ResizeFromOpposite)
        );
        for s in [
            CornerSlot::Swipe(CornerDir::Horizontal),
            CornerSlot::Tap,
            CornerSlot::Press,
            CornerSlot::Hold,
        ] {
            assert!(
                !kinds_of(s).contains(&ActionKind::ResizeFromOpposite),
                "{s:?}"
            );
        }
    }

    #[test]
    fn a_slot_switched_on_gets_a_real_action() {
        for s in CornerSlot::ALL {
            assert_ne!(default_action(s), Action::None, "{s:?}");
        }
    }

    #[test]
    fn the_status_line_names_what_is_off() {
        let a = Action::TaskView;
        assert!(status(false, true, CornerTrigger::Tap, &a).contains("コーナー操作"));
        assert!(status(true, false, CornerTrigger::Hold, &a).contains("ホールド"));
        assert_eq!(
            status(true, true, CornerTrigger::Tap, &Action::None),
            "未割り当て"
        );
        assert!(status(true, true, CornerTrigger::Tap, &a).starts_with("有効"));
    }

    #[test]
    fn every_trigger_explains_itself() {
        for tr in CornerTrigger::ALL {
            assert!(explanation(tr, 200, 500).len() >= 2);
        }
    }

    #[test]
    fn corner_signs_cover_the_four_corners() {
        assert_eq!((sign_of(0), sign_of(1)), ((1.0, 1.0), (-1.0, 1.0)));
        assert_eq!((sign_of(2), sign_of(3)), ((1.0, -1.0), (-1.0, -1.0)));
    }
}
