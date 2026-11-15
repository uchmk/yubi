//! Feature pages (操作の割り当て, リングメニュー, アプリ設定, 押し込みと触覚): settings cards bound to the config.
//! The gestures themselves are not implemented yet; these pages only edit the model.

use eframe::egui::{self, Align, Layout, RichText, Ui};

use crate::config::{
    Combo, Config, EdgeTarget, Finger, HapticStrength, HoldCombos, HoldTrigger, Profile, RingItem,
};
use crate::icons;
use crate::nav::Page;
use crate::win_gestures;

use super::Cmd;
use crate::theme::Tokens;
use crate::widgets::{
    self, SliderSpec, accent_button, combo_sized, group, icon_button, section_header, toggle,
};

const FINGER_COMBO_W: f32 = 120.0;
const TRIGGER_COMBO_W: f32 = 120.0;
const CORNER_NAMES: [&str; 4] = ["左上", "右上", "左下", "右下"];

/// Page header plus the card with the feature's enable toggle. Returns whether the feature is on.
fn feature_top(ui: &mut Ui, t: &Tokens, cfg: &mut Config, page: Page) -> bool {
    widgets::page_header(ui, t, page.label(), page.subtitle());
    group(ui, t, |g| {
        if let Some(flag) = page.enabled_mut(cfg) {
            g.toggle_row(
                Some(page.icon()),
                &format!("{}を有効にする", page.label()),
                "オフにすると、この機能のジェスチャーは無視されます。",
                flag,
            );
        }
    });
    page.enabled_mut(cfg).is_some_and(|f| *f)
}

/// A warning card with a jump to 全般 when Windows' own `fingers`-finger gestures are still on.
fn windows_conflict(
    ui: &mut Ui,
    t: &Tokens,
    values: &win_gestures::Values,
    fingers: u8,
    on: bool,
) -> Option<Cmd> {
    if !on || !win_gestures::conflicts(values, fingers) {
        return None;
    }
    let mut cmd = None;
    group(ui, t, |g| {
        g.wrapped_row(|ui| {
            ui.label(
                RichText::new(format!(
                    "Windows の {fingers} 本指ジェスチャーが有効なままです。この機能と重なって、意図しない動作になることがあります。"
                ))
                .size(t.fs_small)
                .color(t.accent),
            );
            if widgets::button(ui, t, "全般で無効にする").clicked() {
                cmd = Some(Cmd::Goto(Page::General));
            }
        });
    });
    cmd
}

pub fn drag(ui: &mut Ui, t: &Tokens, cfg: &mut Config, win: &win_gestures::Values) -> Option<Cmd> {
    let on = feature_top(ui, t, cfg, Page::Drag);
    let cmd = windows_conflict(ui, t, win, cfg.drag.fingers, on);
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "動作");
        group(ui, t, |g| {
            let d = &mut cfg.drag;
            g.slider_row(
                "指の本数",
                "ドラッグに使う指の本数です。",
                SliderSpec::int(3, 5, 1, " 本"),
                &mut d.fingers,
            );
            g.slider_row(
                "指を離してから終了するまでの時間",
                "この時間内に指を置き直すと、ドラッグを続けられます。",
                SliderSpec::int(0, 1000, 10, " ms"),
                &mut d.lift_timeout_ms,
            );
            g.toggle_row(
                None,
                "離したあともドラッグを続ける",
                "指を離しても、次にタップするまでドラッグ状態を保ちます。",
                &mut d.keep_drag_on_release,
            );
        });
    });
    cmd
}

/// The 4-finger page: tap, four swipe directions and pinch, plus the notes about what overlaps.
pub fn four(ui: &mut Ui, t: &Tokens, cfg: &mut Config, win: &win_gestures::Values) -> Option<Cmd> {
    let on = feature_top(ui, t, cfg, Page::Four);
    let mut cmd = windows_conflict(ui, t, win, 4, on);
    if on && cfg.drag.enabled && cfg.drag.fingers == 4 {
        group(ui, t, |g| {
            g.wrapped_row(|ui| {
                ui.label(
                    RichText::new("ドラッグに 4 本指を使っているため、スワイプは使えません")
                        .size(t.fs_small)
                        .color(t.accent),
                );
                if widgets::button(ui, t, "ドラッグを開く").clicked() {
                    cmd = Some(Cmd::Goto(Page::Drag));
                }
            });
        });
    }
    ui.add_enabled_ui(on, |ui| {
        let f = &mut cfg.four;
        section_header(ui, t, "タップ");
        group(ui, t, |g| {
            g.action_row("タップ", "4本指で軽く触れたとき", "four-tap", &mut f.tap);
        });
        section_header(ui, t, "スワイプ");
        group(ui, t, |g| {
            g.action_row(
                "左にスワイプ",
                "4本指で左へなぞったとき",
                "four-left",
                &mut f.swipe_left,
            );
            g.action_row(
                "右にスワイプ",
                "4本指で右へなぞったとき",
                "four-right",
                &mut f.swipe_right,
            );
            g.action_row(
                "上にスワイプ",
                "4本指で上へなぞったとき",
                "four-up",
                &mut f.swipe_up,
            );
            g.action_row(
                "下にスワイプ",
                "4本指で下へなぞったとき",
                "four-down",
                &mut f.swipe_down,
            );
        });
        section_header(ui, t, "ピンチ");
        group(ui, t, |g| {
            g.action_row(
                "ピンチイン",
                "4本指で指を寄せたとき",
                "four-in",
                &mut f.pinch_in,
            );
            g.action_row(
                "ピンチアウト",
                "4本指で指を広げたとき",
                "four-out",
                &mut f.pinch_out,
            );
        });
    });
    cmd
}

pub fn five(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    let on = feature_top(ui, t, cfg, Page::Five);
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "ピンチ");
        group(ui, t, |g| {
            let f = &mut cfg.five;
            g.action_row(
                "ピンチイン",
                "5本指で指を寄せたとき",
                "five-in",
                &mut f.pinch_in,
            );
            g.action_row(
                "ピンチアウト",
                "5本指で指を広げたとき",
                "five-out",
                &mut f.pinch_out,
            );
        });
        section_header(ui, t, "スワイプ");
        group(ui, t, |g| {
            let f = &mut cfg.five;
            g.action_row(
                "上にスワイプ",
                "5本指で上へなぞったとき",
                "five-up",
                &mut f.swipe_up,
            );
            g.action_row(
                "下にスワイプ",
                "5本指で下へなぞったとき",
                "five-down",
                &mut f.swipe_down,
            );
        });
    });
}

fn threshold_rows(g: &mut widgets::Group<'_>, cfg: &mut Config) {
    let p = &mut cfg.press;
    g.slider_row(
        "軽い押し込みのしきい値",
        "この深さを超えると軽い押し込みとみなします。",
        SliderSpec::fraction(),
        &mut p.light_threshold,
    );
    g.slider_row(
        "深い押し込みのしきい値",
        "この深さを超えると深い押し込みとみなします。軽い押し込みより大きくなります。",
        SliderSpec::fraction(),
        &mut p.deep_threshold,
    );
}

pub fn custom_press(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    let on = feature_top(ui, t, cfg, Page::CustomPress);
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "しきい値");
        group(ui, t, |g| threshold_rows(g, cfg));
        for i in 0..5 {
            section_header(ui, t, &format!("{}本指", i + 1));
            group(ui, t, |g| {
                let f = &mut cfg.press.fingers[i];
                g.action_row("軽い押し込み", "", ("press-light", i), &mut f.light);
                g.action_row("深い押し込み", "", ("press-deep", i), &mut f.deep);
            });
        }
    });
    // Keep deep above light whichever slider was moved.
    cfg.normalize();
}

pub fn long_hold(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    let on = feature_top(ui, t, cfg, Page::LongHold);
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "動作");
        group(ui, t, |g| {
            let l = &mut cfg.long_hold;
            g.slider_row(
                "指の本数",
                "長押しに使う指の本数です。",
                SliderSpec::int(1, 5, 1, " 本"),
                &mut l.fingers,
            );
            g.slider_row(
                "長押しの時間",
                "操作が始まるまでに指を置き続ける時間です。",
                SliderSpec::int(100, 2000, 50, " ms"),
                &mut l.hold_ms,
            );
            g.action_row(
                "実行する操作",
                "長押ししたときの操作です。",
                "long-hold",
                &mut l.action,
            );
        });
    });
}

pub fn hold_combos(ui: &mut Ui, t: &Tokens, cfg: &mut Config, page: Page) {
    let on = feature_top(ui, t, cfg, page);
    let thumb = page == Page::ThumbHold;
    let list: &mut HoldCombos = if thumb {
        &mut cfg.thumb_hold
    } else {
        &mut cfg.finger_hold
    };
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "キーの組み合わせ");
        let mut remove = None;
        let id = if thumb { "thumb-hold" } else { "finger-hold" };
        group(ui, t, |g| {
            for (i, c) in list.combos.iter_mut().enumerate() {
                g.raw_row(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, t, icons::DELETE, "削除").clicked() {
                            remove = Some(i);
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            combo_sized(
                                ui,
                                t,
                                FINGER_COMBO_W,
                                (id, "finger", i),
                                &mut c.hold,
                                &Finger::ALL,
                                Finger::label,
                            );
                            combo_sized(
                                ui,
                                t,
                                TRIGGER_COMBO_W,
                                (id, "trigger", i),
                                &mut c.trigger,
                                &HoldTrigger::ALL,
                                HoldTrigger::label,
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut c.keys)
                                    .hint_text("例: Alt+Tab")
                                    .desired_width(ui.available_width()),
                            );
                        });
                    });
                });
            }
            if list.combos.is_empty() {
                g.raw_row(|ui| {
                    ui.label(RichText::new("組み合わせはまだありません。").color(t.text_muted));
                });
            }
        });
        if let Some(i) = remove {
            list.combos.remove(i);
        }
        if accent_button(ui, t, "追加").clicked() {
            list.combos.push(Combo {
                hold: if thumb { Finger::Thumb } else { Finger::Index },
                ..Combo::default()
            });
        }
    });
}

pub fn corner(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    let on = feature_top(ui, t, cfg, Page::Corner);
    ui.add_enabled_ui(on, |ui| {
        for (i, name) in CORNER_NAMES.iter().enumerate() {
            section_header(ui, t, name);
            group(ui, t, |g| {
                let c = &mut cfg.corner.corners[i];
                g.action_row(
                    "押す",
                    "角を押し込んだときの操作です。",
                    ("corner-press", i),
                    &mut c.press,
                );
                g.action_row(
                    "スワイプ",
                    "反対側の角へなぞったときの操作です。",
                    ("corner-swipe", i),
                    &mut c.swipe,
                );
            });
        }
    });
}

pub fn edge(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    let on = feature_top(ui, t, cfg, Page::Edge);
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "端ごとの対象");
        group(ui, t, |g| {
            let e = &mut cfg.edge;
            for (name, value, id) in [
                ("左端", &mut e.left, "edge-left"),
                ("右端", &mut e.right, "edge-right"),
                ("上端", &mut e.top, "edge-top"),
            ] {
                g.combo_row(
                    name,
                    "なぞって調整する対象です。",
                    id,
                    value,
                    &EdgeTarget::ALL,
                    EdgeTarget::label,
                );
            }
        });
    });
}

pub fn window(
    ui: &mut Ui,
    t: &Tokens,
    cfg: &mut Config,
    win: &win_gestures::Values,
) -> Option<Cmd> {
    let on = feature_top(ui, t, cfg, Page::Window);
    // Window move is a three-finger gesture.
    let cmd = windows_conflict(ui, t, win, 3, on && cfg.window.move_with_three_finger);
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "動作");
        group(ui, t, |g| {
            let w = &mut cfg.window;
            g.toggle_row(
                None,
                "3本指でウィンドウを移動",
                "3本指でタップしてからスワイプすると、ウィンドウを動かします。",
                &mut w.move_with_three_finger,
            );
            g.toggle_row(
                None,
                "フリックでスナップ",
                "ウィンドウをはじくと、画面の端や角に寄せます。",
                &mut w.flick_snap,
            );
            g.toggle_row(
                None,
                "コーナースワイプでサイズ変更",
                "角から反対側の角へなぞって、ウィンドウサイズを変更します。",
                &mut w.corner_resize,
            );
        });
    });
    cmd
}

pub fn ring(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    let on = feature_top(ui, t, cfg, Page::Ring);
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "呼び出し");
        group(ui, t, |g| {
            let r = &mut cfg.ring;
            g.slider_row(
                "指の本数",
                "リングメニューを開く指の本数です。",
                SliderSpec::int(1, 5, 1, " 本"),
                &mut r.fingers,
            );
            g.slider_row(
                "長押しの時間",
                "メニューが現れるまでに指を置き続ける時間です。",
                SliderSpec::int(100, 2000, 50, " ms"),
                &mut r.hold_ms,
            );
        });
        section_header(ui, t, "メニュー項目");
        let items = &mut cfg.ring.items;
        let mut remove = None;
        group(ui, t, |g| {
            for (i, item) in items.iter_mut().enumerate() {
                g.raw_row(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, t, icons::DELETE, "削除").clicked() {
                            remove = Some(i);
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let w = ((ui.available_width() - t.space_sm) / 2.0).max(t.space_xl);
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
                        });
                    });
                });
            }
            if items.is_empty() {
                g.raw_row(|ui| {
                    ui.label(RichText::new("項目はまだありません。").color(t.text_muted));
                });
            }
        });
        if let Some(i) = remove {
            items.remove(i);
        }
        if accent_button(ui, t, "追加").clicked() {
            items.push(RingItem::default());
        }
    });
}

pub fn profiles(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    widgets::page_header(ui, t, Page::Profiles.label(), Page::Profiles.subtitle());
    let mut remove = None;
    group(ui, t, |g| {
        for (i, p) in cfg.profiles.iter_mut().enumerate() {
            g.raw_row(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, t, icons::DELETE, "削除").clicked() {
                        remove = Some(i);
                    }
                    toggle(ui, t, &mut p.enabled);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut p.exe_name)
                                .hint_text("実行ファイル名（例: code.exe）")
                                .desired_width(ui.available_width()),
                        );
                    });
                });
            });
        }
        if cfg.profiles.is_empty() {
            g.raw_row(|ui| {
                ui.label(RichText::new("プロファイルはまだありません。").color(t.text_muted));
            });
        }
    });
    if let Some(i) = remove {
        cfg.profiles.remove(i);
    }
    if accent_button(ui, t, "追加").clicked() {
        cfg.profiles.push(Profile {
            exe_name: String::new(),
            enabled: true,
        });
    }
}

pub fn haptic(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    widgets::page_header(ui, t, Page::Haptic.label(), Page::Haptic.subtitle());
    section_header(ui, t, "しきい値");
    group(ui, t, |g| threshold_rows(g, cfg));
    section_header(ui, t, "触覚");
    group(ui, t, |g| {
        g.combo_row(
            "触覚の強さ",
            "押し込んだときのフィードバックの強さです。",
            "haptic",
            &mut cfg.press.haptic_strength,
            &HapticStrength::ALL,
            HapticStrength::label,
        );
    });
    // Keep deep above light whichever slider was moved.
    cfg.normalize();
}
