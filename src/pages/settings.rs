//! Placeholder settings pages: real widgets bound to the config, no behavior yet.

use eframe::egui::{self, Frame, Margin, RichText, Stroke, Ui};

use crate::config::{Config, HapticStrength, Profile, RingItem};
use crate::theme::Tokens;

fn header(ui: &mut Ui, t: &Tokens, title: &str, hint: &str) {
    ui.heading(title);
    ui.label(RichText::new(hint).color(t.text_muted));
    ui.add_space(t.space_md);
}

fn card(ui: &mut Ui, t: &Tokens, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(t.surface_raised)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(t.radius_lg)
        .inner_margin(Margin::same(t.space_md as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

pub fn gestures(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    header(
        ui,
        t,
        "ジェスチャー",
        "ドラッグなど、指の動きに割り当てる操作です。",
    );
    card(ui, t, |ui| {
        let g = &mut cfg.gestures;
        ui.checkbox(&mut g.three_finger_drag, "複数本指ドラッグを使う");
        ui.add_enabled_ui(g.three_finger_drag, |ui| {
            ui.horizontal(|ui| {
                ui.label("ドラッグの指の本数");
                ui.add(egui::Slider::new(&mut g.drag_fingers, 3..=5).suffix(" 本"));
            });
        });
    });
}

pub fn press(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    header(
        ui,
        t,
        "押し込みと触覚",
        "押し込みの深さのしきい値と、触覚フィードバックの強さです。",
    );
    card(ui, t, |ui| {
        let p = &mut cfg.press;
        egui::Grid::new("press-grid")
            .num_columns(2)
            .spacing([t.space_lg, t.space_sm])
            .show(ui, |ui| {
                ui.label("軽い押し込み");
                ui.add(egui::Slider::new(&mut p.light_threshold, 0.0..=1.0).fixed_decimals(2));
                ui.end_row();
                ui.label("深い押し込み");
                ui.add(egui::Slider::new(&mut p.deep_threshold, 0.0..=1.0).fixed_decimals(2));
                ui.end_row();
                ui.label("触覚の強さ");
                egui::ComboBox::from_id_salt("haptic")
                    .selected_text(p.haptic_strength.label())
                    .show_ui(ui, |ui| {
                        for h in HapticStrength::ALL {
                            ui.selectable_value(&mut p.haptic_strength, h, h.label());
                        }
                    });
                ui.end_row();
            });
    });
    // Keep deep above light whichever slider was moved.
    cfg.normalize();
}

pub fn profiles(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    header(
        ui,
        t,
        "アプリ別プロファイル",
        "アプリごとにジェスチャーの有効・無効を切り替えます。",
    );
    card(ui, t, |ui| {
        let mut remove = None;
        for (i, p) in cfg.profiles.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.enabled, "");
                let w = (ui.available_width() - 72.0 - t.space_sm).max(80.0);
                ui.add(
                    egui::TextEdit::singleline(&mut p.exe_name)
                        .hint_text("例: code.exe")
                        .desired_width(w),
                );
                if ui.button("削除").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            cfg.profiles.remove(i);
        }
        if cfg.profiles.is_empty() {
            ui.label(RichText::new("プロファイルはまだありません。").color(t.text_muted));
        }
        if ui.button("追加").clicked() {
            cfg.profiles.push(Profile {
                exe_name: String::new(),
                enabled: true,
            });
        }
    });
}

pub fn ring(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    header(
        ui,
        t,
        "リングメニュー",
        "リング状に並べる項目と、実行するコマンドです。",
    );
    card(ui, t, |ui| {
        let mut remove = None;
        for (i, item) in cfg.ring.items.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let w = ((ui.available_width() - 72.0 - 2.0 * t.space_sm) / 2.0).max(80.0);
                ui.add(
                    egui::TextEdit::singleline(&mut item.label)
                        .hint_text("表示名")
                        .desired_width(w),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut item.command)
                        .hint_text("コマンド")
                        .desired_width(w),
                );
                if ui.button("削除").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            cfg.ring.items.remove(i);
        }
        if cfg.ring.items.is_empty() {
            ui.label(RichText::new("項目はまだありません。").color(t.text_muted));
        }
        if ui.button("追加").clicked() {
            cfg.ring.items.push(RingItem::default());
        }
    });
}
