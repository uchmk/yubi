//! 押し込みと触覚: the live pressure meter, the optional calibration helper (inline wizard) and the
//! haptic tick settings. Thresholds are raw pressure (0.01 to 0.20); the calibration only suggests
//! values and draws marks, nothing depends on it.
//!
//! The per-count threshold controls ([`count_thresholds`]) are shared with the カスタムプレス page.

use std::time::Duration;

use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};

use crate::calibration::{self, MAX_CALIBRATED_FINGERS, Phase, SAMPLES_PER_PASS, Wizard};
use crate::config::{
    Calibration, Config, DEFAULT_MULTI, HapticStrength, MIN_THRESHOLD_GAP, Press, THRESHOLD_RANGE,
    fit_pair, q01,
};
use crate::config_io;
use crate::engine::MonitorState;
use crate::input::now_us;
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{
    self, Group, SliderSpec, accent_button, button, group, section_header, slider_with_value,
};

/// Top of the meter scale: the pad reports about 0.01 to 0.20.
const METER_MAX: f32 = 0.20;
const ROW_H: f32 = 12.0;
const LABEL_W: f32 = 52.0;
const VALUE_W: f32 = 44.0;
/// A frame older than this no longer counts as "now" on the meter.
const FRESH_US: u64 = 300_000;
const THRESHOLD_SPEC: SliderSpec = SliderSpec {
    min: THRESHOLD_RANGE.0 as f64,
    max: THRESHOLD_RANGE.1 as f64,
    step: 0.01,
    decimals: 2,
    suffix: "",
};

/// State of the 押し込みと触覚 page that lives across frames.
#[derive(Default)]
pub struct PressUi {
    wizard: Wizard,
    /// `frames_total` of the last frame handed to the wizard (a frame is fed once).
    last_frames: u64,
    /// The finger count the meter marks follow (the last count that was on the pad).
    shown: usize,
    /// One-line result of the last button ("2 本指のしきい値を ... にしました").
    message: Option<String>,
    /// Development screenshots: the demo state was set up once.
    demo_ready: bool,
    /// The pad (`vvvv:pppp`) the running wizard measures; results are stored under it only.
    wizard_key: Option<String>,
    /// When this page last ran: a long silence means it was left, and a sample in progress is stale.
    last_seen_us: u64,
}

/// A page that was not drawn for this long was left; a wizard sample in progress is dropped.
const LEFT_US: u64 = 200_000;

/// Keep the wizard bound to the pad it started on. `held` is whether the wizard has anything
/// running or finished; `bound` is the pad it started on. Returns true when the wizard must be
/// reset because the pad went away or changed.
fn wizard_pad_changed(held: bool, bound: &mut Option<String>, key: Option<&str>) -> bool {
    if !held {
        *bound = None;
        return false;
    }
    match bound {
        None => {
            *bound = key.map(str::to_owned);
            false
        }
        Some(b) => {
            let changed = key != Some(b.as_str());
            if changed {
                *bound = None;
            }
            changed
        }
    }
}

/// Development aid (`YUBI_SHOT_PRESS=idle|wizard|calibrated`): a fake pad with fake contacts and a
/// demo config, so the page can be photographed without a trackpad. Returns the fake state.
pub fn demo(mode: &str, cfg: &mut Config, st: &mut PressUi, frames: u64) -> MonitorState {
    use crate::input::{Contact, DeviceInfo, TouchFrame};
    use std::sync::Arc;
    let device = Arc::new(DeviceInfo {
        id: r"\?\HID#VID_05AC&PID_0324#demo".into(),
        name: "Magic Trackpad".into(),
        x_range: (0, 1000),
        y_range: (0, 1000),
        pressure_range: Some((0, 255)),
        finger_slots: 5,
        size_mm: (160.0, 114.9),
    });
    let key = calibration::device_key(&device.id).unwrap_or_default();
    if !st.demo_ready {
        st.demo_ready = true;
        cfg.press.enabled = true;
        for f in cfg.press.fingers.iter_mut().take(3) {
            f.enabled = true;
        }
        cfg.press.fingers[2].light_threshold = None;
        cfg.press.fingers[2].deep_threshold = None;
        match mode {
            "wizard" => st.wizard.start(),
            "calibrated" => {
                cfg.calibration.insert(
                    key,
                    Calibration {
                        baseline: 0.01,
                        f1: 0.18,
                        f2: Some(0.15),
                        date: "2026-11-15".into(),
                        ..Calibration::default()
                    },
                );
                st.message = Some("2 本指のしきい値を 軽 0.07 / 深 0.12 にしました。".into());
            }
            _ => {}
        }
    }
    let levels: &[f32] = match mode {
        "wizard" => &[0.02],
        "calibrated" => &[0.13, 0.10, 0.06],
        _ => &[0.09, 0.07],
    };
    let contacts = levels
        .iter()
        .enumerate()
        .map(|(i, &p)| Contact {
            id: i as u32,
            x: 0.3 + 0.1 * i as f32,
            y: 0.5,
            x_mm: 50.0,
            y_mm: 50.0,
            tip: true,
            confidence: true,
            pressure: Some(p),
        })
        .collect();
    let t_us = now_us();
    MonitorState {
        device: Some(device.clone()),
        frame: Some(TouchFrame {
            contacts,
            button: false,
            scan_time: 0,
            t_us,
            device,
        }),
        frames_total: frames,
        last_frame_us: t_us,
        ..MonitorState::default()
    }
}

// ---------------------------------------------------------------------------------------------
// Pure helpers

/// Set the thresholds of `fingers` fingers from a measured baseline and firm level
/// ([`calibration::suggest`]). One finger writes the shared pair; two to five write that count's
/// own pair (so "1 本指と同じ" turns off). Returns the pair that was set.
pub fn use_measured(press: &mut Press, fingers: usize, baseline: f32, firm: f32) -> (f32, f32) {
    let (l, d) = calibration::suggest(baseline, firm);
    if fingers <= 1 {
        press.light_threshold = l;
        press.deep_threshold = d;
    } else if let Some(f) = press.fingers.get_mut(fingers - 1) {
        f.light_threshold = Some(l);
        f.deep_threshold = Some(d);
    }
    (l, d)
}

/// The shared "same as one finger" state of a count (counts 2..=5).
pub fn shares_one_finger(press: &Press, fingers: usize) -> bool {
    fingers >= 2
        && press
            .fingers
            .get(fingers - 1)
            .is_some_and(|f| f.light_threshold.is_none() && f.deep_threshold.is_none())
}

/// Switch a count between the shared pair and its own pair (starting from the defaults).
pub fn set_shared(press: &mut Press, fingers: usize, shared: bool) {
    let Some(f) = press.fingers.get_mut(fingers.wrapping_sub(1)) else {
        return;
    };
    if fingers < 2 {
        return;
    }
    if shared {
        f.light_threshold = None;
        f.deep_threshold = None;
    } else {
        f.light_threshold = Some(DEFAULT_MULTI.0);
        f.deep_threshold = Some(DEFAULT_MULTI.1);
    }
}

// ---------------------------------------------------------------------------------------------
// Threshold controls (shared with カスタムプレス)

/// Slider, value and a "-" / "+" step button, right to left inside a row's control area.
fn stepper(ui: &mut Ui, t: &Tokens, v: &mut f32) -> bool {
    let mut changed = false;
    if button(ui, t, "+").clicked() {
        *v = q01(*v + 0.01);
        changed = true;
    }
    changed |= slider_with_value(ui, t, v, THRESHOLD_SPEC);
    if button(ui, t, "-").clicked() {
        *v = q01(*v - 0.01);
        changed = true;
    }
    if changed {
        *v = q01(*v);
    }
    changed
}

/// The light and deep rows for one pair, with the inline reason when the gap rule moved a value.
fn pair_rows(g: &mut Group<'_>, t: &Tokens, id: egui::Id, light: &mut f32, deep: &mut f32) {
    let ctx = g.ctx();
    let (mut cl, mut cd) = (false, false);
    g.row(
        None,
        "軽い押し込みのしきい値",
        "この値を超えると軽い押し込みとみなします (0.01 から 0.20)。",
        |ui| cl = stepper(ui, t, light),
    );
    g.row(
        None,
        "深い押し込みのしきい値",
        "この値を超えると深い押し込みとみなします。",
        |ui| cd = stepper(ui, t, deep),
    );
    if cl || cd {
        let gap_broken = *deep - *light < MIN_THRESHOLD_GAP - 1e-4;
        (*light, *deep) = fit_pair(*light, *deep);
        ctx.data_mut(|d| d.insert_temp(id, gap_broken));
    }
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        g.wrapped_row(|ui| {
            ui.label(
                RichText::new("深いしきい値は軽いしきい値より 0.02 以上大きくしてください")
                    .size(t.fs_small)
                    .color(t.accent),
            );
        });
    }
}

/// Threshold rows for `fingers` (1..=5) fingers: the shared pair for one finger, and for two or
/// more the "1 本指と同じ" switch plus the count's own pair.
pub fn count_thresholds(g: &mut Group<'_>, t: &Tokens, press: &mut Press, fingers: usize) {
    let id = egui::Id::unique(("press-gap", fingers));
    if fingers <= 1 {
        let (mut l, mut d) = (press.light_threshold, press.deep_threshold);
        pair_rows(g, t, id, &mut l, &mut d);
        (press.light_threshold, press.deep_threshold) = (l, d);
        return;
    }
    let mut shared = shares_one_finger(press, fingers);
    let before = shared;
    g.toggle_row(
        None,
        "1 本指と同じしきい値を使う",
        "オフにすると、この本数だけのしきい値を使います。複数の指では力が分かれるため、低めが向いています。",
        &mut shared,
    );
    if shared != before {
        set_shared(press, fingers, shared);
    }
    if shared {
        return;
    }
    let Some(f) = press.fingers.get_mut(fingers - 1) else {
        return;
    };
    let (mut l, mut d) = (
        f.light_threshold.unwrap_or(DEFAULT_MULTI.0),
        f.deep_threshold.unwrap_or(DEFAULT_MULTI.1),
    );
    pair_rows(g, t, id, &mut l, &mut d);
    f.light_threshold = Some(l);
    f.deep_threshold = Some(d);
}

// ---------------------------------------------------------------------------------------------
// The page

/// What the meter shows: marks for one finger count.
struct Marks {
    light: f32,
    deep: f32,
    baseline: Option<f32>,
    firm: Option<f32>,
}

pub fn page(ui: &mut Ui, t: &Tokens, cfg: &mut Config, mon: &MonitorState, st: &mut PressUi) {
    widgets::page_header(ui, t, Page::Haptic.label(), Page::Haptic.subtitle());

    let device = mon.device.clone();
    let key = device.as_ref().and_then(|d| calibration::device_key(&d.id));
    let has_pressure = device.as_ref().is_some_and(|d| d.pressure_range.is_some());

    let now = now_us();
    if st.last_seen_us != 0 && now.saturating_sub(st.last_seen_us) > LEFT_US {
        st.wizard.interrupt();
    }
    st.last_seen_us = now;
    let held = st.wizard.phase() != Phase::Idle;
    if wizard_pad_changed(held, &mut st.wizard_key, key.as_deref()) {
        st.wizard = Wizard::default();
        st.message =
            Some("トラックパッドが変わったため、測定をやめました。もう一度始めてください".into());
    }

    // Hand the newest frame to the wizard once.
    let fresh = now_us().saturating_sub(mon.last_frame_us) <= FRESH_US;
    let pressures: Vec<Option<f32>> = match (&mon.frame, fresh) {
        (Some(f), true) => f
            .contacts
            .iter()
            .filter(|c| c.tip)
            .map(|c| c.pressure)
            .collect(),
        _ => Vec::new(),
    };
    if mon.frames_total != st.last_frames {
        st.last_frames = mon.frames_total;
        if let Some(f) = &mon.frame {
            st.wizard.feed(f.t_us, &pressures);
        }
    } else if !fresh && st.wizard.active() {
        // The pad stopped sending frames (all fingers lifted): that is an empty frame.
        st.wizard.feed(now_us(), &[]);
    }
    if !pressures.is_empty() {
        st.shown = pressures.len().min(MAX_CALIBRATED_FINGERS);
    }
    store_result(cfg, st);

    if device.is_some() {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }

    let n = st.shown.max(1);
    let cal = key.as_ref().and_then(|k| cfg.calibration.get(k)).cloned();
    let (light, deep) = cfg.press.thresholds(n);
    let marks = Marks {
        light,
        deep,
        baseline: cal.as_ref().map(|c| c.baseline),
        firm: cal.as_ref().and_then(|c| c.firm(n)),
    };

    section_header(ui, t, "いまの押し込み");
    group(ui, t, |g| {
        if device.is_none() {
            g.wrapped_row(|ui| {
                ui.label(
                    RichText::new("トラックパッドが見つかりません。接続すると、ここに押し込みの強さが表示されます。")
                        .color(t.text_muted),
                );
            });
        } else if !has_pressure {
            g.wrapped_row(|ui| {
                ui.label(
                    RichText::new("このトラックパッドは押し込みの強さ (圧力) を報告しません。")
                        .color(t.text_muted),
                );
            });
        } else {
            g.wrapped_row(|ui| meter(ui, t, &pressures, &marks));
            g.info_row(
                &format!("{n} 本指のしきい値"),
                &format!("軽 {:.2} / 深 {:.2}", marks.light, marks.deep),
            );
            if let Some(b) = marks.baseline {
                let firm = marks
                    .firm
                    .map_or("未測定".to_owned(), |f| format!("{f:.2}"));
                g.info_row(
                    "測定値 (軸の ○)",
                    &format!("軽く触れる {b:.2} / しっかり押す {firm}"),
                );
            }
        }
    });

    section_header(ui, t, "キャリブレーション (任意)");
    calibration_card(
        ui,
        t,
        cfg,
        st,
        key.as_deref(),
        device.is_some() && has_pressure,
    );

    section_header(ui, t, "触覚");
    group(ui, t, |g| {
        g.toggle_row(
            None,
            "押し込みを触覚で伝える",
            "軽い押し込みと深い押し込みを認識したときに、短い触覚を返します。使えない環境では何も起きません。",
            &mut cfg.press.haptic_ticks,
        );
        g.combo_row(
            "触覚の強さ",
            "押し込んだときのフィードバックの強さです。",
            "haptic",
            &mut cfg.press.haptic_strength,
            &HapticStrength::ALL,
            HapticStrength::label,
        );
    });
    cfg.normalize();
}

/// Keep the wizard's finished measurements in the config (device bound, not exported).
fn store_result(cfg: &mut Config, st: &PressUi) {
    // Only under the pad the wizard started on.
    let Some(key) = st.wizard_key.as_deref() else {
        return;
    };
    if !matches!(st.wizard.phase(), Phase::Offer(_) | Phase::Done) {
        return;
    }
    let date = config_io::date_utc(config_io::unix_secs());
    if let Some(cal) = st.wizard.result(&date) {
        let same = cfg.calibration.get(key).is_some_and(|old| {
            Calibration {
                date: date.clone(),
                ..old.clone()
            } == cal
        });
        if !same {
            cfg.calibration.insert(key.to_owned(), cal);
        }
    }
}

fn calibration_card(
    ui: &mut Ui,
    t: &Tokens,
    cfg: &mut Config,
    st: &mut PressUi,
    key: Option<&str>,
    usable: bool,
) {
    let cal = key.and_then(|k| cfg.calibration.get(k)).cloned();
    group(ui, t, |g| {
        if !usable {
            g.wrapped_row(|ui| {
                ui.label(
                    RichText::new("圧力を報告するトラックパッドを接続すると使えます。しきい値は、ここを使わずに直接決められます。")
                        .color(t.text_muted),
                );
            });
            return;
        }
        if st.wizard.active() {
            wizard_rows(g, t, cfg, st, key);
            return;
        }
        g.wrapped_row(|ui| {
            ui.label(
                RichText::new(
                    "軽く触れたときと、しっかり押したときの圧力を測ると、しきい値の目安を出せます。測らなくても使えます。",
                )
                .size(t.fs_small)
                .color(t.text_muted),
            );
        });
        if let Some(m) = st.message.clone() {
            g.wrapped_row(|ui| {
                ui.label(RichText::new(m).size(t.fs_small).color(t.accent));
            });
        }
        let Some(cal) = cal else {
            g.wrapped_row(|ui| {
                if accent_button(ui, t, "測定を始める").clicked() {
                    st.message = None;
                    st.wizard.start();
                }
            });
            return;
        };
        g.info_row("軽く触れたとき", &format!("{:.2}", cal.baseline));
        for n in 1..=MAX_CALIBRATED_FINGERS {
            let firm = cal.firm(n);
            let title = format!("{n} 本指でしっかり押す");
            let desc = match firm {
                Some(f) => format!("測定値 {f:.2}"),
                None => "未測定".to_owned(),
            };
            g.row(None, &title, &desc, |ui| {
                if button(
                    ui,
                    t,
                    if firm.is_some() {
                        "測り直す"
                    } else {
                        "測る"
                    },
                )
                .clicked()
                {
                    st.message = None;
                    st.wizard.start_count(n, &cal);
                }
                if let Some(f) = firm
                    && button(ui, t, "この値をしきい値に使う").clicked()
                {
                    let (l, d) = use_measured(&mut cfg.press, n, cal.baseline, f);
                    st.message = Some(applied_message(n, l, d));
                }
            });
        }
        g.wrapped_row(|ui| {
            if button(ui, t, "最初からやり直す").clicked() {
                st.message = None;
                st.wizard.start();
            }
            if let Some(k) = key
                && button(ui, t, "測定値を消す").clicked()
            {
                cfg.calibration.remove(k);
                st.message = None;
            }
        });
    });
}

fn applied_message(n: usize, l: f32, d: f32) -> String {
    format!("{n} 本指のしきい値を 軽 {l:.2} / 深 {d:.2} にしました。")
}

fn wizard_rows(
    g: &mut Group<'_>,
    t: &Tokens,
    cfg: &mut Config,
    st: &mut PressUi,
    key: Option<&str>,
) {
    let phase = st.wizard.phase();
    let step = st.wizard.step();
    match phase {
        Phase::Offer(next) => {
            let done_n = next - 1;
            let cal = st.wizard.result("");
            g.wrapped_row(|ui| {
                ui.label(
                    RichText::new(format!("{done_n} 本指の測定が終わりました。")).size(t.fs_body),
                );
            });
            if let Some(m) = st.message.clone() {
                g.wrapped_row(|ui| {
                    ui.label(RichText::new(m).size(t.fs_small).color(t.accent));
                });
            }
            g.wrapped_row(|ui| {
                if let Some(c) = &cal
                    && let Some(f) = c.firm(done_n)
                    && accent_button(ui, t, "この値をしきい値に使う").clicked()
                {
                    let (l, d) = use_measured(&mut cfg.press, done_n, c.baseline, f);
                    st.message = Some(applied_message(done_n, l, d));
                }
                if button(ui, t, &format!("{next} 本指でも測る")).clicked() {
                    st.message = None;
                    st.wizard.accept_offer();
                }
                if button(ui, t, "ここで終える").clicked() {
                    st.wizard.skip_offer();
                }
            });
        }
        Phase::Failed(r) => {
            g.wrapped_row(|ui| {
                ui.label(RichText::new(r.message()).color(t.accent));
            });
            g.wrapped_row(|ui| {
                if accent_button(ui, t, "やり直す").clicked() {
                    st.wizard.retry();
                }
                if button(ui, t, "やめる").clicked() {
                    st.wizard = Wizard::default();
                }
            });
        }
        _ => {
            let total = format!(
                "{} / {SAMPLES_PER_PASS} 回",
                st.wizard.samples_done().min(SAMPLES_PER_PASS)
            );
            g.wrapped_row(|ui| {
                ui.label(RichText::new(step.instruction()).size(t.fs_body));
            });
            g.wrapped_row(|ui| {
                let hint = match phase {
                    Phase::Sampling { .. } => "そのまま 1 秒ほど保ってください",
                    Phase::WaitLift => "いったん指を離してください",
                    _ => "指を置いてください",
                };
                ui.label(RichText::new(hint).size(t.fs_small).color(t.text_muted));
                if let Some(n) = st.wizard.note() {
                    ui.label(RichText::new(n).size(t.fs_small).color(t.accent));
                }
            });
            g.wrapped_row(|ui| {
                progress_bar(ui, t, st.wizard.progress());
                ui.label(RichText::new(total).size(t.fs_small).color(t.text_muted));
            });
            g.wrapped_row(|ui| {
                if button(ui, t, "やり直す").clicked() {
                    st.wizard.start();
                }
                if button(ui, t, "やめる").clicked() {
                    st.wizard = Wizard::default();
                }
            });
        }
    }
    let _ = key;
}

fn progress_bar(ui: &mut Ui, t: &Tokens, frac: f32) {
    let w = (ui.available_width() - 80.0).clamp(80.0, 320.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 8.0), Sense::hover());
    ui.painter().rect_filled(rect, t.radius_sm, t.rail);
    let mut fill = rect;
    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
    ui.painter().rect_filled(fill, t.radius_sm, t.accent);
}

// ---------------------------------------------------------------------------------------------
// Meter

/// Bars for the strongest contact and each contact, on the raw 0.00 to 0.20 scale, with the light
/// and deep marks and (when measured) the baseline / firm level.
fn meter(ui: &mut Ui, t: &Tokens, contacts: &[Option<f32>], marks: &Marks) {
    let rows = 1 + MAX_CALIBRATED_FINGERS;
    let head = 18.0;
    let axis = 18.0;
    let h = head + rows as f32 * (ROW_H + t.space_xs) + axis;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let painter = ui.painter_at(rect);
    let (x0, x1) = (rect.left() + LABEL_W, rect.right() - VALUE_W);
    let x_of = |v: f32| x0 + (x1 - x0) * (v / METER_MAX).clamp(0.0, 1.0);
    let top = rect.top() + head;
    let small = t.font(t.fs_small);

    let values: Vec<f32> = contacts.iter().map(|c| c.unwrap_or(0.0)).collect();
    let max = values.iter().copied().fold(0.0_f32, f32::max);
    for r in 0..rows {
        let y = top + r as f32 * (ROW_H + t.space_xs);
        let track = Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + ROW_H));
        painter.rect_filled(track, t.radius_sm, t.rail);
        let (label, value, color) = if r == 0 {
            (
                "最大".to_owned(),
                (!values.is_empty()).then_some(max),
                t.accent,
            )
        } else {
            (format!("指 {r}"), values.get(r - 1).copied(), t.text_muted)
        };
        painter.text(
            Pos2::new(rect.left(), y + ROW_H / 2.0),
            Align2::LEFT_CENTER,
            label,
            small.clone(),
            if r == 0 { t.text } else { t.text_muted },
        );
        if let Some(v) = value {
            let bar = Rect::from_min_max(
                Pos2::new(x0, y),
                Pos2::new(x_of(v).max(x0 + 1.0), y + ROW_H),
            );
            painter.rect_filled(bar, t.radius_sm, color);
            painter.text(
                Pos2::new(rect.right(), y + ROW_H / 2.0),
                Align2::RIGHT_CENTER,
                format!("{v:.2}"),
                small.clone(),
                t.text,
            );
        }
    }
    let bottom = top + rows as f32 * (ROW_H + t.space_xs) - t.space_xs;

    // Threshold marks: lines through every bar, labelled above (light to the left, deep to the right).
    let mark = |v: f32, text: String, color, anchor: Align2, dx: f32| {
        let x = x_of(v);
        painter.line_segment(
            [Pos2::new(x, top - 3.0), Pos2::new(x, bottom)],
            Stroke::new(2.0, color),
        );
        painter.text(
            Pos2::new(x + dx, rect.top() + 1.0),
            anchor,
            text,
            small.clone(),
            color,
        );
    };
    mark(
        marks.light,
        format!("軽 {:.2}", marks.light),
        t.text,
        Align2::RIGHT_TOP,
        -3.0,
    );
    mark(
        marks.deep,
        format!("深 {:.2}", marks.deep),
        t.accent,
        Align2::LEFT_TOP,
        3.0,
    );

    // Axis with 2-decimal ticks, and the measured levels as small marks on it.
    let ay = bottom + t.space_xs;
    painter.line_segment(
        [Pos2::new(x0, ay), Pos2::new(x1, ay)],
        Stroke::new(1.0, t.border),
    );
    for k in 0..=4 {
        let v = METER_MAX * k as f32 / 4.0;
        painter.text(
            Pos2::new(x_of(v), ay + 2.0),
            match k {
                0 => Align2::LEFT_TOP,
                4 => Align2::RIGHT_TOP,
                _ => Align2::CENTER_TOP,
            },
            format!("{v:.2}"),
            small.clone(),
            t.text_muted,
        );
    }
    for (v, text) in [(marks.baseline, "測"), (marks.firm, "測")] {
        if let Some(v) = v {
            painter.circle_stroke(Pos2::new(x_of(v), ay), 3.5, Stroke::new(1.5, t.text_muted));
            let _ = text;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wizard_stays_on_the_pad_it_started_on() {
        let mut bound = None;
        // Nothing running: nothing is bound.
        assert!(!wizard_pad_changed(false, &mut bound, Some("05ac:0324")));
        assert_eq!(bound, None);
        // It starts on one pad and keeps it.
        assert!(!wizard_pad_changed(true, &mut bound, Some("05ac:0324")));
        assert!(!wizard_pad_changed(true, &mut bound, Some("05ac:0324")));
        assert_eq!(bound.as_deref(), Some("05ac:0324"));
        // Another pad, or none at all: reset.
        assert!(wizard_pad_changed(true, &mut bound, Some("05ac:0265")));
        assert_eq!(bound, None);
        assert!(!wizard_pad_changed(true, &mut bound, Some("05ac:0324")));
        assert!(wizard_pad_changed(true, &mut bound, None));
    }

    #[test]
    fn measured_values_become_the_one_finger_thresholds() {
        let mut p = Press::default();
        let set = use_measured(&mut p, 1, 0.01, 0.16);
        assert_eq!(set, (0.07, 0.13));
        assert_eq!((p.light_threshold, p.deep_threshold), (0.07, 0.13));
    }

    #[test]
    fn measured_values_for_more_fingers_set_only_that_count() {
        let mut p = Press::default();
        p.fingers[2].light_threshold = None;
        p.fingers[2].deep_threshold = None;
        assert!(shares_one_finger(&p, 3));
        let set = use_measured(&mut p, 3, 0.01, 0.12);
        assert_eq!(set, (0.05, 0.10));
        assert!(!shares_one_finger(&p, 3));
        assert_eq!(p.thresholds(3), (0.05, 0.10));
        assert_eq!(p.thresholds(1), (0.08, 0.14));
        assert_eq!(p.thresholds(2), DEFAULT_MULTI);
    }

    #[test]
    fn a_measurement_is_clamped_into_the_range_with_the_gap() {
        let mut p = Press::default();
        let (l, d) = use_measured(&mut p, 1, 0.19, 0.20);
        assert!(
            l >= 0.01 && d <= 0.20 && d - l >= MIN_THRESHOLD_GAP - 1e-6,
            "{l} {d}"
        );
        let (l, d) = use_measured(&mut p, 2, 0.01, 0.03);
        assert!(d - l >= MIN_THRESHOLD_GAP - 1e-6);
        assert_eq!(fit_pair(l, d), (l, d));
    }

    #[test]
    fn sharing_the_one_finger_pair_toggles() {
        let mut p = Press::default();
        assert!(!shares_one_finger(&p, 2));
        set_shared(&mut p, 2, true);
        assert!(shares_one_finger(&p, 2));
        assert_eq!(p.thresholds(2), p.thresholds(1));
        set_shared(&mut p, 2, false);
        assert_eq!(p.thresholds(2), DEFAULT_MULTI);
        // One finger has no own pair.
        set_shared(&mut p, 1, true);
        assert!(!shares_one_finger(&p, 1));
        assert_eq!(
            crate::config::DEFAULT_ONE,
            (p.light_threshold, p.deep_threshold)
        );
    }
}
