//! The 感度 page: how far, how fast and how long a finger must move for each gesture.

use eframe::egui::{self, Align, Layout, RichText, Ui};

use super::Cmd;
use crate::config::{Config, Cursor, Scroll, ScrollDirection, SensRange, Sensitivity};
use crate::nav::Page;
use crate::os::touchpad;
use crate::theme::Tokens;
use crate::widgets::{self, Group, Num, SliderSpec, group, section_header, slider_with_value};

/// A slider spec covering `range` in steps of `step`.
fn spec(range: SensRange, step: f64, decimals: usize, suffix: &'static str) -> SliderSpec {
    SliderSpec {
        min: f64::from(range.min),
        max: f64::from(range.max),
        step,
        decimals,
        suffix,
    }
}

/// One row: slider with value and unit, plus a button that restores the default.
fn row<N: Num + PartialEq>(
    g: &mut Group<'_>,
    t: &Tokens,
    title: &str,
    desc: &str,
    spec: SliderSpec,
    value: &mut N,
    default: N,
) {
    g.row(None, title, desc, |ui| {
        // The first widget added sits rightmost.
        let changed = *value != default;
        let reset = ui.add_enabled_ui(changed, |ui| widgets::button(ui, t, "既定に戻す"));
        if reset.inner.clicked() {
            *value = default;
        }
        slider_with_value(ui, t, value, spec);
    });
}

/// A slider row that is greyed out while its feature is off.
fn scroll_row(
    g: &mut Group<'_>,
    t: &Tokens,
    on: bool,
    title: &str,
    desc: &str,
    spec: SliderSpec,
    slot: (&mut f32, f32),
) {
    let (value, default) = slot;
    g.row(None, title, desc, |ui| {
        ui.add_enabled_ui(on, |ui| {
            let changed = *value != default;
            let reset = ui.add_enabled_ui(changed, |ui| widgets::button(ui, t, "既定に戻す"));
            if reset.inner.clicked() {
                *value = default;
            }
            slider_with_value(ui, t, value, spec);
        });
    });
}

/// 2 本指スクロール: yubi's own two-finger scroll, with the warning while Windows' still runs.
fn scroll_group(ui: &mut Ui, t: &Tokens, cfg: &mut Config, win_pan: Option<bool>) -> Option<Cmd> {
    let mut cmd = None;
    let d = Scroll::default();
    let s = &mut cfg.scroll;
    group(ui, t, |g| {
        g.toggle_row(
            None,
            "2 本指スクロールを使う",
            "2 本指でなぞったときのスクロールを yubi が行い、速さをマウスホイールとは別に決められます。",
            &mut s.enabled,
        );
        // Windows scrolls too unless its own switch is off: the page would move twice.
        if s.enabled && win_pan != Some(false) {
            g.wrapped_row(|ui| {
                ui.vertical(|ui| {
                let lead = if win_pan.is_some() {
                    "Windows の「2 本指でドラッグしてスクロール」がオンのままです。ページが二重にスクロールします。"
                } else {
                    "Windows の「2 本指でドラッグしてスクロール」の状態を確認できません。オンのままだと、ページが二重にスクロールします。"
                };
                ui.label(RichText::new(lead).size(t.fs_small).color(t.accent));
                ui.label(
                    RichText::new(
                        "設定 → Bluetooth とデバイス → タッチパッド → スクロールとズーム → 「2 本指でドラッグしてスクロール」をオフにしてください。",
                    )
                    .size(t.fs_small)
                    .color(t.text_muted),
                );
                if widgets::button(ui, t, "Windows のタッチパッド設定を開く").clicked() {
                    cmd = Some(Cmd::OpenTouchpadSettings);
                }
                });
            });
        }
        let on = s.enabled;
        scroll_row(
            g,
            t,
            on,
            "縦の速さ",
            "縦にスクロールする量の倍率です。",
            spec(Scroll::SPEED, 0.1, 1, " 倍"),
            (&mut s.vertical_speed, d.vertical_speed),
        );
        scroll_row(
            g,
            t,
            on,
            "横の速さ",
            "横にスクロールする量の倍率です。",
            spec(Scroll::SPEED, 0.1, 1, " 倍"),
            (&mut s.horizontal_speed, d.horizontal_speed),
        );
        scroll_row(
            g,
            t,
            on,
            "加速",
            "速くなぞるほど大きくスクロールします。0 で一定です。",
            spec(Scroll::ACCEL, 0.05, 2, ""),
            (&mut s.accel, d.accel),
        );
        g.row(
            None,
            "スクロールの向き",
            "指の動きとスクロールの向きの関係です。",
            |ui| {
                ui.add_enabled_ui(on, |ui| {
                    widgets::combo(
                        ui,
                        t,
                        "scroll-direction",
                        &mut s.direction,
                        &ScrollDirection::ALL,
                        ScrollDirection::label,
                    );
                });
            },
        );
        g.row(
            None,
            "縦横を固定する",
            "ほぼ縦（横）になぞったとき、その向きだけにスクロールします。",
            |ui| {
                ui.add_enabled_ui(on, |ui| widgets::toggle(ui, t, &mut s.axis_lock));
            },
        );
        g.row(
            None,
            "慣性スクロール",
            "指を離したあともしばらくスクロールを続けます。",
            |ui| {
                ui.add_enabled_ui(on, |ui| widgets::toggle(ui, t, &mut s.inertia));
            },
        );
        scroll_row(
            g,
            t,
            on && s.inertia,
            "慣性の強さ",
            "大きいほど長く滑ります。",
            spec(Scroll::INERTIA, 0.05, 2, ""),
            (&mut s.inertia_strength, d.inertia_strength),
        );
    });
    cmd
}

pub fn sensitivity(
    ui: &mut Ui,
    t: &Tokens,
    cfg: &mut Config,
    win_pan: Option<bool>,
) -> Option<Cmd> {
    widgets::page_header(
        ui,
        t,
        Page::Sensitivity.label(),
        Page::Sensitivity.subtitle(),
    );
    section_header(ui, t, "カーソル");
    group(ui, t, |g| cursor_group(g, t, &mut cfg.cursor));

    section_header(ui, t, "2 本指スクロール");
    let cmd = scroll_group(ui, t, cfg, win_pan);

    let d = Sensitivity::default();
    let s = &mut cfg.sensitivity;

    section_header(ui, t, "基本");
    group(ui, t, |g| {
        let r = Sensitivity::DRAG_GAIN;
        row(
            g,
            t,
            "ドラッグの速さ",
            "指を動かしたときにカーソルが進む速さです。大きいほど速く動きます。",
            spec(r, 1.0, 0, " px/mm"),
            &mut s.drag_gain,
            d.drag_gain,
        );
        row(
            g,
            t,
            "ウィンドウ移動の速さ",
            "3本指でウィンドウを動かすときの速さです。",
            spec(r, 1.0, 0, " px/mm"),
            &mut s.window_gain,
            d.window_gain,
        );
        row(
            g,
            t,
            "スワイプの距離",
            "スワイプと判定するために指を動かす距離です。小さいほど軽いなぞりで反応します。",
            spec(Sensitivity::SWIPE, 1.0, 0, " mm"),
            &mut s.swipe_mm,
            d.swipe_mm,
        );
        row(
            g,
            t,
            "長押しの時間",
            "長押しと判定するまで指を置き続ける時間です。",
            spec(Sensitivity::HOLD_MS, 50.0, 0, " ms"),
            &mut s.hold_ms,
            d.hold_ms,
        );
    });

    section_header(ui, t, "詳細");
    // A shot can open the section to check its layout.
    let open = std::env::var_os("YUBI_SENS_OPEN").is_some();
    egui::CollapsingHeader::new("詳細な判定値を調整")
        .id_salt("sensitivity-detail")
        .default_open(open)
        .show(ui, |ui| {
            detail(ui, t, s, &d);
        });
    cmd
}

/// The touchpad cursor speed: Windows' own setting, kept by yubi while it runs.
fn cursor_group(g: &mut Group<'_>, t: &Tokens, cur: &mut Cursor) {
    let support = touchpad::support();
    let usable = support.is_ok();
    let desc = match &support {
        Ok(()) => {
            "オンのあいだ、yubi がタッチパッドのカーソルの速度を保ちます。終了すると元の速さに戻します。"
        }
        Err(e) => e.reason(),
    };
    let mut on = cur.speed.is_some() && usable;
    g.row(
        None,
        "yubi でカーソルの速度を管理する",
        desc,
        |ui| {
            ui.add_enabled_ui(usable, |ui| {
                widgets::toggle(ui, t, &mut on);
            });
        },
    );
    if usable && on != cur.speed.is_some() {
        cur.speed = on.then(|| {
            Cursor::clamp_speed(touchpad::cursor_speed().unwrap_or(touchpad::SPEED_DEFAULT))
        });
    }
    let (lo, hi) = Cursor::RANGE;
    let spec = SliderSpec::int(u16::from(lo), u16::from(hi), 1, "");
    let mut speed = cur.speed.unwrap_or(touchpad::SPEED_DEFAULT);
    g.row(
        None,
        "カーソルの速度",
        "タッチパッドでカーソルを動かす速さです。マウスの速度とは別の設定です。",
        |ui| {
            ui.add_enabled_ui(usable && cur.speed.is_some(), |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(speed.to_string()).size(t.fs_body));
                    ui.add_space(t.space_sm);
                    ui.label(RichText::new("速い").size(t.fs_small).color(t.text_muted));
                    let mut v = f64::from(speed);
                    if widgets::slider(ui, t, &mut v, spec) {
                        speed = u8::from_f64(v);
                    }
                    ui.label(RichText::new("遅い").size(t.fs_small).color(t.text_muted));
                });
            });
        },
    );
    if cur.speed.is_some() && cur.speed != Some(speed) {
        cur.speed = Some(speed);
    }
    g.wrapped_row(|ui| {
        ui.label(
            RichText::new(format!(
                "アプリごとの速さは「{}」のページで設定します。",
                Page::Profiles.label()
            ))
            .size(t.fs_small)
            .color(t.text_muted),
        );
    });
}

fn detail(ui: &mut Ui, t: &Tokens, s: &mut Sensitivity, d: &Sensitivity) {
    group(ui, t, |g| {
        row(
            g,
            t,
            "ドラッグ開始の距離",
            "ドラッグが始まるまでに指を動かす距離です。",
            spec(Sensitivity::DRAG_START, 0.1, 1, " mm"),
            &mut s.drag_start_mm,
            d.drag_start_mm,
        );
        row(
            g,
            t,
            "ウィンドウ移動の開始距離",
            "ウィンドウが動き始めるまでに指を動かす距離です。",
            spec(Sensitivity::WINDOW_START, 0.5, 1, " mm"),
            &mut s.window_start_mm,
            d.window_start_mm,
        );
        row(
            g,
            t,
            "ウィンドウ移動までの間隔",
            "タップしてからスワイプを始めるまでに許される時間です。",
            spec(Sensitivity::WINDOW_GAP, 10.0, 0, " ms"),
            &mut s.window_gap_ms,
            d.window_gap_ms,
        );
        row(
            g,
            t,
            "タップの長さ",
            "タップと判定する、触れてから離すまでの最長時間です。",
            spec(Sensitivity::TAP_MS, 10.0, 0, " ms"),
            &mut s.tap_ms,
            d.tap_ms,
        );
        row(
            g,
            t,
            "タップの許容移動",
            "タップの間に指が動いてもよい距離です。",
            spec(Sensitivity::TAP_TRAVEL, 0.1, 1, " mm"),
            &mut s.tap_travel_mm,
            d.tap_travel_mm,
        );
        row(
            g,
            t,
            "長押しの許容移動",
            "長押しの間に指が動いてもよい距離です。",
            spec(Sensitivity::HOLD_TRAVEL, 0.1, 1, " mm"),
            &mut s.hold_travel_mm,
            d.hold_travel_mm,
        );
        row(
            g,
            t,
            "ピンチインの比率",
            "指の間隔がこの比率まで縮むとピンチインです。小さいほど大きく寄せる必要があります。",
            spec(Sensitivity::PINCH_IN, 0.01, 2, ""),
            &mut s.pinch_in,
            d.pinch_in,
        );
        row(
            g,
            t,
            "ピンチアウトの比率",
            "指の間隔がこの比率まで広がるとピンチアウトです。大きいほど大きく広げる必要があります。",
            spec(Sensitivity::PINCH_OUT, 0.01, 2, ""),
            &mut s.pinch_out,
            d.pinch_out,
        );
        row(
            g,
            t,
            "フリックの速さ",
            "指を離す直前の速さがこの値を超えるとフリックです。",
            spec(Sensitivity::FLICK, 10.0, 0, " mm/s"),
            &mut s.flick_mm_s,
            d.flick_mm_s,
        );
        row(
            g,
            t,
            "端の幅",
            "エッジスライダーが反応する、トラックパッドの端からの幅です。",
            spec(Sensitivity::EDGE_STRIP, 0.5, 1, " mm"),
            &mut s.edge_strip_mm,
            d.edge_strip_mm,
        );
        row(
            g,
            t,
            "端の刻み",
            "エッジスライダーの1ステップになる、なぞる距離です。",
            spec(Sensitivity::EDGE_DETENT, 0.5, 1, " mm"),
            &mut s.edge_detent_mm,
            d.edge_detent_mm,
        );
    });
}
