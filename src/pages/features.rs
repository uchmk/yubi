//! Feature pages (操作の割り当て, リングメニュー, 押し込みと触覚): settings cards bound to the config.
//! The gestures themselves are not implemented yet; these pages only edit the model.

use eframe::egui::{self, Align, Layout, RichText, Ui};

use crate::config::{
    ActionKind, Combo, Config, EdgeKind, EdgeTarget, Finger, HoldCombos, HoldTrigger, Ring,
    RingItem,
};
use crate::howto::{self, Mini, Side};
use crate::icons;
use crate::nav::Page;
use crate::win_gestures;

use super::Cmd;
use crate::theme::Tokens;
use crate::widgets::{
    self, SliderSpec, accent_button, combo_sized, group, icon_button, section_header,
};

const FINGER_COMBO_W: f32 = 120.0;
const TRIGGER_COMBO_W: f32 = 120.0;

/// Page header plus the card with the feature's enable toggle. Returns whether the feature is on.
pub(super) fn feature_top(ui: &mut Ui, t: &Tokens, cfg: &mut Config, page: Page) -> bool {
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

pub fn custom_press(
    ui: &mut Ui,
    t: &Tokens,
    cfg: &mut Config,
    device_key: Option<&str>,
) -> Option<Cmd> {
    let on = feature_top(ui, t, cfg, Page::CustomPress);
    let mut cmd = None;
    let measured = device_key.is_some_and(|k| cfg.calibration.contains_key(k));
    ui.add_enabled_ui(on, |ui| {
        group(ui, t, |g| {
            g.wrapped_row(|ui| {
                ui.label(
                    RichText::new(
                        "押し込んでも、通常のクリックはそのまま行われます。押し込みの強さは 0.01 から 0.20 の値で決めます。複数の指では力が分かれるため、本数が多いほど低めが向いています。",
                    )
                    .size(t.fs_small)
                    .color(t.text_muted),
                );
            });
            g.wrapped_row(|ui| {
                ui.label(
                    RichText::new(if measured {
                        "このトラックパッドは測定済みです (任意)。"
                    } else {
                        "測定しなくても使えます。目安を出したいときは測定できます。"
                    })
                    .size(t.fs_small)
                    .color(t.text_muted),
                );
                if widgets::button(ui, t, "押し込みと触覚を開く").clicked() {
                    cmd = Some(Cmd::Goto(Page::Haptic));
                }
            });
        });
        for n in 1..=5usize {
            section_header(ui, t, &format!("{n} 本指"));
            group(ui, t, |g| {
                let f = &mut cfg.press.fingers[n - 1];
                g.toggle_row(
                    None,
                    &format!("{n} 本指の押し込みを使う"),
                    "オフのあいだ、この本数の押し込みは無視されます。",
                    &mut f.enabled,
                );
                if !f.enabled {
                    return;
                }
                g.action_row("軽い押し込み", "", ("press-light", n - 1), &mut f.light);
                g.action_row("深い押し込み", "", ("press-deep", n - 1), &mut f.deep);
                super::press::count_thresholds(g, t, &mut cfg.press, n);
            });
        }
    });
    // Keep deep above light whichever slider was moved.
    cfg.normalize();
    cmd
}

/// The long hold page: what a 1 to 5 finger long hold does. "リングメニュー" on 3 to 5 fingers is
/// what makes the ring open there; the ring page only shows which counts those are.
pub fn long_hold(ui: &mut Ui, t: &Tokens, cfg: &mut Config) -> Option<Cmd> {
    let on = feature_top(ui, t, cfg, Page::LongHold);
    let mut cmd = None;
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "動作");
        group(ui, t, |g| {
            g.slider_row(
                "長押しの時間",
                "操作が始まるまでに指を置き続ける時間です。すべての指の本数で共通で、「感度」ページの値と同じです。",
                SliderSpec::int(300, 1500, 50, " ms"),
                &mut cfg.sensitivity.hold_ms,
            );
        });
        section_header(ui, t, "指の本数ごとの操作");
        let without_ring: Vec<ActionKind> = ActionKind::ALL
            .into_iter()
            .filter(|k| *k != ActionKind::RingMenu)
            .collect();
        group(ui, t, |g| {
            for n in 1..=5usize {
                let can_ring = n >= 3;
                let desc = if can_ring {
                    "指を置き続けたときの操作です。「リングメニュー」を選ぶと、リングが開きます。"
                } else {
                    "指を置き続けたときの操作です。"
                };
                let kinds: &[ActionKind] = if can_ring {
                    &ActionKind::ALL
                } else {
                    &without_ring
                };
                let a = cfg.long_hold.actions.get_mut(n);
                g.row(None, &format!("{n}本指の長押し"), desc, |ui| {
                    widgets::action_picker_of(ui, t, ("long-hold", n), a, kinds);
                });
            }
        });
        if !cfg.long_hold.ring_counts().is_empty() && !cfg.ring.enabled {
            group(ui, t, |g| {
                g.wrapped_row(|ui| {
                    ui.label(
                        RichText::new(
                            "リングメニューが無効のため、「リングメニュー」を選んだ長押しでは何も起きません。",
                        )
                        .size(t.fs_small)
                        .color(t.accent),
                    );
                    if widgets::button(ui, t, "リングメニューを開く").clicked() {
                        cmd = Some(Cmd::Goto(Page::Ring));
                    }
                });
            });
        }
    });
    cmd
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

pub fn edge(ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    let on = feature_top(ui, t, cfg, Page::Edge);
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "使い方");
        group(ui, t, |g| {
            g.wrapped_row(|ui| edge_guide(ui, t, cfg));
        });
        section_header(ui, t, "端ごとの対象");
        group(ui, t, |g| {
            let e = &mut cfg.edge;
            for (name, value, id) in [
                ("左端", &mut e.left, "edge-left"),
                ("右端", &mut e.right, "edge-right"),
                ("上端", &mut e.top, "edge-top"),
            ] {
                edge_target_rows(g, t, name, id, value);
            }
        });
        section_header(ui, t, "フィードバック");
        group(ui, t, |g| {
            g.toggle_row(
                None,
                "レベルを画面に表示する",
                "音量や明るさを変えている間、画面の下にバーと数値を出します。ジェスチャーに割り当てた音量・明るさの操作でも出ます。",
                &mut cfg.edge.show_level,
            );
            g.toggle_row(
                None,
                "段階ごとに触覚で知らせる",
                "なぞって音量や明るさが 1 段階変わるたびに、短い触覚を返します。端まで届いて変わらないときは返しません。使えない環境では何も起きません。",
                &mut cfg.edge.haptic_ticks,
            );
        });
    });
}

/// The target's name without a trailing "(...)" detail, to fit in the diagram.
fn short_label(target: &EdgeTarget) -> &'static str {
    let l = target.label();
    l.split(" (").next().unwrap_or(l)
}

/// How to use the edge slider: an animated pad with the configured bands, three steps, the details.
fn edge_guide(ui: &mut Ui, t: &Tokens, cfg: &Config) {
    let strip = cfg.sensitivity.edge_strip_mm;
    let detent = cfg.sensitivity.edge_detent_mm;
    let e = &cfg.edge;
    ui.vertical(|ui| {
        let anim = howto::Anim::new(ui);
        let bands = [
            (Side::Left, &e.left),
            (Side::Right, &e.right),
            (Side::Top, &e.top),
        ]
        .map(|(side, target)| howto::EdgeBand {
            side,
            label: (*target != EdgeTarget::None).then(|| short_label(target)),
            step_mm: target.step_mm(detent),
        });
        let steps: Vec<f32> = bands
            .iter()
            .filter(|b| b.label.is_some())
            .map(|b| b.step_mm)
            .collect();
        let step_text = match steps.first() {
            Some(first) if steps.iter().all(|s| (s - first).abs() < 0.05) => {
                format!("{first:.1} mm なぞるごとに 1 段階進みます。")
            }
            Some(_) => "なぞる距離は端ごとの対象の設定に従い、その距離ごとに 1 段階進みます。".to_owned(),
            None => format!("{detent:.1} mm なぞるごとに 1 段階進みます。"),
        };
        let side = [Side::Left, Side::Right, Side::Top]
            .into_iter()
            .zip([&e.left, &e.right, &e.top])
            .find(|(_, target)| **target != EdgeTarget::None)
            .map_or(Side::Left, |(side, _)| side);
        let step = std::cell::Cell::new(0);
        howto::two_col(
            ui,
            t,
            howto::edge_scene_width(ui.available_width()),
            |ui| step.set(howto::edge_scene(ui, t, anim, &bands, strip)),
            |ui| {
                howto::summary(
                    ui,
                    t,
                    "端に指を置いて、端に沿ってなぞります",
                    &[
                        format!(
                            "ハイライトの帯が反応する範囲です（端から {strip:.1} mm）。帯の中で約 0.15 秒、動かさずに待つと始まります（2 mm 以内）。"
                        ),
                        format!(
                            "3 mm ほど進むと有効になり、カーソルは止まります。{step_text}左右の端は上へ、上の端は右へなぞると「進む」（音量なら上げる）、逆が「戻る」です。"
                        ),
                        "指を離すか、2 本目の指を置くと終わります。".to_owned(),
                    ],
                );
                ui.add_space(t.space_md);
                howto::steps(
                    ui,
                    t,
                    &[
                        Mini::EdgePlace(side),
                        Mini::EdgeWait(side),
                        Mini::EdgeSlide(side),
                    ],
                    &["端に指を 1 本置く", "少し止める", "端に沿ってなぞる"],
                    anim.live().then_some(step.get()),
                );
            },
        );
    });
}

/// A row with the target picker for one edge, plus key and distance rows for a custom target.
fn edge_target_rows(
    g: &mut widgets::Group<'_>,
    t: &Tokens,
    name: &str,
    id: &str,
    value: &mut EdgeTarget,
) {
    let mut kind = value.kind();
    g.row(None, name, "なぞって調整する対象です。", |ui| {
        let w = t.combo_w * 1.6;
        if combo_sized(ui, t, w, id, &mut kind, &EdgeKind::ALL, EdgeKind::label) {
            value.set_kind(kind);
        }
    });
    if let EdgeTarget::Custom(c) = value {
        for (title, desc, text, hint) in [
            (
                "進む方向のキー",
                "指を進めたときに送るキーです。",
                &mut c.forward,
                "例: Ctrl+Tab",
            ),
            (
                "戻る方向のキー",
                "指を戻したときに送るキーです。",
                &mut c.back,
                "例: Ctrl+Shift+Tab",
            ),
        ] {
            g.row(None, title, desc, |ui| {
                ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(hint)
                        .desired_width(t.text_input_w),
                );
            });
        }
        g.slider_row(
            "1ステップの距離",
            "なぞる距離がこの値に達するたびにキーを送ります。",
            SliderSpec {
                min: 1.0,
                max: 6.0,
                step: 0.5,
                decimals: 1,
                suffix: " mm",
            },
            &mut c.step_mm,
        );
    }
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

/// "3 本の指", "3 本または 5 本の指", "3 本、4 本、5 本のいずれかの指" (nothing bound: a hint).
fn ring_fingers_text(counts: &[usize]) -> String {
    match counts {
        [] => "指（本数は「ロングホールド」で選びます）".to_owned(),
        [a] => format!("{a} 本の指"),
        [a, b] => format!("{a} 本または {b} 本の指"),
        many => format!(
            "{}のいずれかの指",
            many.iter()
                .map(|n| format!("{n} 本"))
                .collect::<Vec<_>>()
                .join("、")
        ),
    }
}

pub fn ring(ui: &mut Ui, t: &Tokens, cfg: &mut Config) -> Option<Cmd> {
    let on = feature_top(ui, t, cfg, Page::Ring);
    let mut cmd = None;
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "使い方");
        group(ui, t, |g| {
            g.wrapped_row(|ui| ring_guide(ui, t, cfg));
        });
        for n in cfg.ring_counts() {
            if (cfg.drag.enabled && usize::from(cfg.drag.fingers) == n)
                || (cfg.four.enabled && n == 4)
                || (cfg.five.enabled && n == 5)
            {
                ui.label(
                    RichText::new(format!(
                        "{n} 本指には別のジェスチャーも割り当てられています。指を {} ms 動かさずに置くと、そちらより先にリングが開きます。",
                        cfg.sensitivity.hold_ms
                    ))
                    .size(t.fs_small)
                    .color(t.accent),
                );
                ui.add_space(t.space_sm);
            }
        }
        if !cfg.ring.has_usable_items() {
            group(ui, t, |g| {
                g.wrapped_row(|ui| {
                    let msg = if cfg.ring.items.is_empty() {
                        "項目が空のため、既定の項目（コピー・貼り付け・元に戻す・スクリーンショット・タスクビュー・デスクトップ表示）を使っています。"
                    } else {
                        "動作が設定された項目がないため、既定の項目を使っています。"
                    };
                    ui.label(RichText::new(msg).size(t.fs_small).color(t.accent));
                    if widgets::button(ui, t, "既定の項目を一覧に追加して編集する").clicked() {
                        cfg.ring.items = Ring::default_items();
                    }
                });
            });
        }
        section_header(ui, t, "呼び出し");
        group(ui, t, |g| {
            let hold_ms = &mut cfg.sensitivity.hold_ms;
            let bound = cfg.long_hold.ring_counts();
            let (summary, attention) = if bound.is_empty() {
                (
                    "注意: どの指の本数にも割り当てられていません".to_owned(),
                    true,
                )
            } else {
                let list = bound
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join("・");
                if cfg.long_hold.enabled {
                    (format!("ロングホールドの {list} 本指で開きます"), false)
                } else {
                    (
                        format!(
                            "注意: ロングホールドが無効のため、{list} 本指でも開きません"
                        ),
                        true,
                    )
                }
            };
            g.row(
                None,
                "指の本数",
                "リングメニューを開く指の本数は、「ロングホールド」で「リングメニュー」を選んだ本数です（3〜5 本）。",
                |ui| {
                    if widgets::button(ui, t, "ロングホールドで変更").clicked() {
                        cmd = Some(Cmd::Goto(Page::LongHold));
                    }
                    let color = if attention { t.accent } else { t.text_muted };
                    ui.label(RichText::new(summary).size(t.fs_small).color(color));
                },
            );
            g.slider_row(
                "長押しの時間",
                "メニューが現れるまでに指を置き続ける時間です。長押しと共通で、「感度」ページの値と同じです。",
                SliderSpec::int(300, 1500, 50, " ms"),
                hold_ms,
            );
        });
        section_header(ui, t, "メニュー項目");
        let items = &mut cfg.ring.items;
        let mut remove = None;
        ui.label(
            RichText::new(format!(
                "最大 {} 個です。上から順に、リングの真上から時計回りに並びます。",
                Ring::MAX_ITEMS
            ))
            .size(t.fs_small)
            .color(t.text_muted),
        );
        ui.add_space(t.space_sm);
        group(ui, t, |g| {
            for (i, item) in items.iter_mut().enumerate() {
                g.raw_row(|ui| {
                    ui.vertical(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, t, icons::DELETE, "削除").clicked() {
                                remove = Some(i);
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                let beyond = i >= Ring::MAX_ITEMS;
                                let color = if beyond { t.accent } else { t.text_muted };
                                ui.label(
                                    RichText::new(format!("{}", i + 1))
                                        .size(t.fs_small)
                                        .color(color),
                                )
                                .on_hover_text(if beyond {
                                    "最大を超えているため、リングには出ません"
                                } else {
                                    "リングでの順番（真上が 1、時計回り）"
                                });
                                ui.add(
                                    egui::TextEdit::singleline(&mut item.label)
                                        .hint_text("表示名")
                                        .desired_width(t.text_input_w / 2.0),
                                );
                                widgets::action_picker_head(
                                    ui,
                                    t,
                                    &("ring-item", i),
                                    &mut item.action,
                                    &ActionKind::ALL,
                                );
                            });
                        });
                        if let crate::config::Action::Launch { path, args } = &mut item.action {
                            // The long fields get a line of their own, so a narrow window wraps
                            // them instead of pushing the delete button out of view.
                            ui.horizontal(|ui| {
                                ui.add_space(t.space_lg);
                                widgets::launch_block(ui, t, &("ring-item", i), path, args);
                            });
                        }
                    });
                });
            }
            if items.is_empty() {
                g.raw_row(|ui| {
                    ui.label(
                        RichText::new("項目はまだありません（上の既定の項目が使われます）。")
                            .color(t.text_muted),
                    );
                });
            }
        });
        if let Some(i) = remove {
            items.remove(i);
        }
        let full = items.len() >= Ring::MAX_ITEMS;
        ui.add_enabled_ui(!full, |ui| {
            if accent_button(ui, t, "追加").clicked() {
                items.push(RingItem::default());
            }
        });
        if full {
            ui.label(
                RichText::new(format!("項目は最大 {} 個までです。", Ring::MAX_ITEMS))
                    .size(t.fs_small)
                    .color(t.text_muted),
            );
        }
        section_header(ui, t, "フィードバック");
        group(ui, t, |g| {
            g.toggle_row(
                None,
                "触覚で知らせる",
                "リングが開いたとき、選ぶ項目が変わるたび、項目を実行したときに、短い触覚を返します。取り消したときは返しません。使えない環境では何も起きません。",
                &mut cfg.ring.haptic_ticks,
            );
        });
    });
    cmd
}

/// How to use the ring menu: an animated pad and ring mock, four steps, the details.
fn ring_guide(ui: &mut Ui, t: &Tokens, cfg: &Config) {
    let counts = cfg.long_hold.ring_counts();
    // The animation shows the first bound count (5 when nothing is bound yet).
    let n = counts.first().copied().unwrap_or(5).clamp(3, 5);
    let who = ring_fingers_text(&counts);
    let hold_ms = cfg.sensitivity.hold_ms;
    let labels: Vec<String> = cfg
        .ring
        .effective_items()
        .into_iter()
        .map(|i| i.label)
        .collect();
    ui.vertical(|ui| {
        let anim = howto::Anim::new(ui);
        let step = std::cell::Cell::new(0);
        howto::two_col(
            ui,
            t,
            howto::ring_scene_width(ui.available_width()),
            |ui| step.set(howto::ring_scene(ui, t, anim, n, hold_ms, &labels)),
            |ui| {
                howto::summary(
                    ui,
                    t,
                    &format!("{who}を置いたままにして、リングから選びます"),
                    &[
                        format!(
                            "{hold_ms} ms ほど動かさずに待つと、カーソルの位置にリングが現れます。真上が先頭の項目で、時計回りに並びます。"
                        ),
                        "選びたい項目の方向へ、指を置いたまま手ごと 8 mm 以上動かし、すべての指を一緒に離すと実行されます。".to_owned(),
                        "取り消し: 中央に戻して離す、Esc キー、手を大きく動かす（約 4.5 cm 以上）、何もせず 5 秒たつ、指の本数が変わる。".to_owned(),
                    ],
                );
                ui.add_space(t.space_md);
                howto::steps(
                    ui,
                    t,
                    &[
                        Mini::RingPlace(n),
                        Mini::RingWait(n),
                        Mini::RingMove(n),
                        Mini::Run(n),
                    ],
                    &[
                        &format!("{n}本の指を置く"),
                        "動かさずに待つ",
                        "方向へ動かす",
                        "離して実行",
                    ],
                    anim.live().then_some(step.get()),
                );
            },
        );
    });
}
