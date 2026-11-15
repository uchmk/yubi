//! The 感度 page: how far, how fast and how long a finger must move for each gesture.

use eframe::egui::{self, Ui};

use crate::config::{Config, SensRange, Sensitivity};
use crate::nav::Page;
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

pub fn sensitivity(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    widgets::page_header(
        ui,
        t,
        Page::Sensitivity.label(),
        Page::Sensitivity.subtitle(),
    );
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
