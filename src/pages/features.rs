//! Feature pages (操作の割り当て, リングメニュー, 押し込みと触覚): settings cards bound to the config.
//! The gestures themselves are not implemented yet; these pages only edit the model.

use eframe::egui::{self, Align, Layout, RichText, Ui};

use crate::config::{
    Action, ActionKind, Combo, Config, DragAction, DragRow, DragStart, EdgeKind, EdgeTarget,
    Finger, HoldCombos, HoldTrigger, Ring, RingItem,
};
use crate::howto::{self, Mini, Side};
use crate::howto_scenes;
use crate::icons;
use crate::nav::Page;
use crate::win_gestures;

use super::{Cmd, overview};
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
    page.is_on(cfg)
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

/// Muted notes when a drag row and a long hold use the same finger count.
fn overlap_note(ui: &mut Ui, t: &Tokens, cfg: &Config) {
    for o in overview::drag_hold_overlaps(cfg) {
        group(ui, t, |g| {
            g.wrapped_row(|ui| {
                ui.label(RichText::new(o.note()).size(t.fs_small).color(t.text_muted));
            });
        });
    }
}

/// The tab of the ドラッグ page (0: 開始と動作, 1: 保持と一時停止).
fn drag_tab_id(ui: &Ui) -> egui::Id {
    ui.make_persistent_id("yubi-drag-tab")
}

/// Development aid (devshots): pin the ドラッグ tab (`usize::MAX`: not pinned).
static FORCE_DRAG_TAB: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);

pub fn force_drag_tab(tab: usize) {
    FORCE_DRAG_TAB.store(tab.min(1), std::sync::atomic::Ordering::Relaxed);
}

fn start_label(s: DragStart) -> &'static str {
    match s {
        DragStart::TouchMove => "触れて動かす",
        DragStart::TapThenMove => "タップ後に動かす",
    }
}

fn action_label(a: DragAction) -> &'static str {
    match a {
        DragAction::Drag => "通常のドラッグ",
        DragAction::WindowMove => "ウィンドウ移動",
        DragAction::WindowResize => "ウィンドウのサイズ変更",
    }
}

/// The row the how-to scene follows: the first one that is on, else the three-finger row.
fn demo_row(cfg: &Config) -> (usize, DragRow) {
    cfg.drag.active().next().unwrap_or((3, DragRow::default()))
}

/// The three-step strip, the animated scene and the one-line summary.
fn drag_guide(ui: &mut Ui, t: &Tokens, cfg: &Config) {
    let (n, row) = demo_row(cfg);
    let tap_first = row.start == DragStart::TapThenMove;
    let what = match row.action {
        DragAction::Drag => "ドラッグ",
        DragAction::WindowMove => "ウィンドウを移動",
        DragAction::WindowResize => "ウィンドウのサイズを変更",
    };
    let captions: [String; 3] = if tap_first {
        [
            format!("{n}本指でタップ"),
            format!("すぐ {n}本指を置いて動かして{what}"),
            "指を離すと終了".to_owned(),
        ]
    } else {
        [
            format!("{n}本指を置く"),
            format!("そのまま動かして{what}"),
            "指を離すと終了".to_owned(),
        ]
    };
    let title = if tap_first {
        format!("{n}本指でタップしてから、すぐ置いて動かします")
    } else {
        format!("{n}本指を置いて、そのまま動かします")
    };
    ui.vertical(|ui| {
        let anim = howto::Anim::new(ui);
        let step = std::cell::Cell::new(0);
        howto::two_col(
            ui,
            t,
            howto_scenes::drag_scene_width(ui.available_width()),
            |ui| {
                step.set(howto_scenes::drag_scene(
                    ui, t, anim, n, tap_first, row.action,
                ))
            },
            |ui| {
                howto::summary(
                    ui,
                    t,
                    &title,
                    &[
                        "指の動きが少し進むと始まります。指を離すと、「保持と一時停止」で決めた時間のあとに終了します（離したあとも続ける設定なら、次のタップまで続きます）。".to_owned(),
                        "タップ後に動かす行は、タップのあと 0.3 秒以内に置き直した指だけが対象です。"
                            .to_owned(),
                    ],
                );
                ui.add_space(t.space_md);
                howto::steps(
                    ui,
                    t,
                    &[
                        Mini::Icon(icons::TOUCHPAD),
                        Mini::Icon(icons::DRAG),
                        Mini::Icon(icons::FOUR_FINGER),
                    ],
                    &[&captions[0], &captions[1], &captions[2]],
                    anim.live().then(|| step.get()),
                );
            },
        );
    });
}

pub fn drag(ui: &mut Ui, t: &Tokens, cfg: &mut Config, win: &win_gestures::Values) -> Option<Cmd> {
    widgets::page_header(ui, t, Page::Drag.label(), Page::Drag.subtitle());
    let pinned = FORCE_DRAG_TAB.load(std::sync::atomic::Ordering::Relaxed);
    let mut tab: usize = if pinned == usize::MAX {
        ui.data(|d| d.get_temp(drag_tab_id(ui))).unwrap_or(0)
    } else {
        pinned
    }
    .min(1);
    ui.add_space(t.space_sm);
    let before = tab;
    super::corner::segmented_tabs(
        ui,
        t,
        "drag-tab",
        &["開始と動作", "保持と一時停止"],
        &mut tab,
    );
    if tab != before {
        ui.data_mut(|d| d.insert_temp(drag_tab_id(ui), tab));
        if pinned != usize::MAX {
            FORCE_DRAG_TAB.store(tab, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let mut cmd = None;
    if tab == 1 {
        section_header(ui, t, "保持と一時停止");
        group(ui, t, |g| {
            let d = &mut cfg.drag;
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
        ui.add_space(t.space_sm);
        ui.label(
            RichText::new("この設定は、有効なすべての本数のドラッグに共通です。")
                .size(t.fs_small)
                .color(t.text_muted),
        );
        return cmd;
    }
    ui.add_space(t.space_sm);
    drag_guide(ui, t, cfg);
    section_header(ui, t, "本数ごとの設定");
    widgets::sections2(ui, t, 2, 4, |ui, i| {
        if i == 3 {
            let active: Vec<usize> = cfg.drag.active().map(|(n, _)| n).collect();
            for n in active {
                cmd = cmd.or(windows_conflict(ui, t, win, n as u8, true));
            }
            overlap_note(ui, t, cfg);
            ui.add_space(t.space_sm);
            ui.label(
                RichText::new(
                    "テキストの選択、ファイルの移動、タイトルバーの操作に使えます。指を離すと、「保持と一時停止」で決めた時間のあとに終了します。Windows の同じ本数のスワイプ・タップと重なる場合は、Windows の設定で「何もしない」にしてください。",
                )
                .size(t.fs_small)
                .color(t.text_muted),
            );
            return;
        }
        let (n, icon) = [
            (3usize, icons::DRAG),
            (4, icons::FOUR_FINGER),
            (5, icons::FIVE_FINGER),
        ][i];
        let flick = &mut cfg.window.flick_snap;
        let Some(row) = cfg.drag.row_mut(n) else {
            return;
        };
        group(ui, t, |g| {
            g.toggle_row(
                Some(icon),
                &format!("{n}本指ドラッグ"),
                &format!("{n}本指を使う操作です。"),
                &mut row.enabled,
            );
            if row.enabled {
                g.combo_row(
                    "開始方法",
                    "指を置いてすぐ動かすか、先にタップするかを選びます。",
                    ("drag-start", n),
                    &mut row.start,
                    &[DragStart::TouchMove, DragStart::TapThenMove],
                    start_label,
                );
                g.combo_row(
                    "実行する動作",
                    "動かしている間に行うことです。",
                    ("drag-action", n),
                    &mut row.action,
                    &[
                        DragAction::Drag,
                        DragAction::WindowMove,
                        DragAction::WindowResize,
                    ],
                    action_label,
                );
                if row.action == DragAction::WindowMove {
                    g.toggle_row(
                        None,
                        "フリックでスナップ",
                        "ウィンドウをはじいて離すと、画面の端や角に寄せます。",
                        flick,
                    );
                }
                if row.action == DragAction::WindowResize {
                    g.wrapped_row(|ui| {
                        ui.label(
                            RichText::new(
                                "触れた位置で決まります。ウィンドウを 3x3 に分けて、角のマスはその角を、辺のマスはその辺を、中央は移動です。",
                            )
                            .size(t.fs_small)
                            .color(t.text_muted),
                        );
                    });
                }
            }
        });
    });
    cmd
}

/// The tab of the ジェスチャー page (0: 3 fingers, 1: 4 fingers, 2: 5 fingers), kept between frames.
fn gesture_tab_id(ui: &Ui) -> egui::Id {
    ui.make_persistent_id("yubi-gesture-tab")
}

/// Development aid (devshots): pin the ジェスチャー tab.
static FORCE_TAB: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);

/// A tab picked by a jump from the search or the overview; the page takes it on its next frame.
static PENDING_TAB: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);

/// Open the ジェスチャー page on tab `tab` (0: 3, 1: 4, 2: 5 fingers) the next time it shows.
pub fn request_gesture_tab(tab: usize) {
    PENDING_TAB.store(tab.min(2), std::sync::atomic::Ordering::Relaxed);
}

/// The tab to open for a gesture id such as `swipe.5.up` (`None` for ids of other pages).
pub fn tab_of_key(key: &str) -> Option<usize> {
    match key.split('.').collect::<Vec<_>>().as_slice() {
        ["tap" | "swipe" | "pinch", "3", ..] => Some(0),
        ["tap" | "swipe" | "pinch", "5", ..] => Some(2),
        ["tap" | "swipe" | "pinch", ..] => Some(1),
        _ => None,
    }
}

pub fn force_gesture_tab(tab: usize) {
    if let Ok(mut f) = FORCE_TAB.lock() {
        *f = Some(tab.min(2));
    }
}

/// The ジェスチャー page: tabs 3本指 / 4本指 / 5本指, each with its own enable toggle and the sections
/// タップ, スワイプ and ピンチ.
pub fn gesture(
    ui: &mut Ui,
    t: &Tokens,
    cfg: &mut Config,
    win: &win_gestures::Values,
) -> Option<Cmd> {
    widgets::page_header(ui, t, Page::Gesture.label(), Page::Gesture.subtitle());
    let pending = PENDING_TAB.swap(usize::MAX, std::sync::atomic::Ordering::Relaxed);
    if pending != usize::MAX {
        ui.data_mut(|d| d.insert_temp(gesture_tab_id(ui), pending));
        if let Ok(mut f) = FORCE_TAB.lock()
            && f.is_some()
        {
            *f = Some(pending);
        }
    }
    let pinned = FORCE_TAB.lock().ok().and_then(|f| *f);
    let mut tab: usize = pinned
        .or_else(|| ui.data(|d| d.get_temp(gesture_tab_id(ui))))
        .unwrap_or(1)
        .min(2);
    let before = tab;
    ui.add_space(t.space_sm);
    super::corner::segmented_tabs(ui, t, "gesture-tab", &["3本指", "4本指", "5本指"], &mut tab);
    if tab != before {
        ui.data_mut(|d| d.insert_temp(gesture_tab_id(ui), tab));
        if let Ok(mut f) = FORCE_TAB.lock()
            && f.is_some()
        {
            *f = Some(tab);
        }
    }
    let n: u8 = 3 + tab as u8;
    let (icon, title) = match tab {
        0 => (icons::FOUR_FINGER, "3本指ジェスチャーを有効にする"),
        1 => (icons::FOUR_FINGER, "4本指ジェスチャーを有効にする"),
        _ => (icons::FIVE_FINGER, "5本指ジェスチャーを有効にする"),
    };
    let flag = match tab {
        0 => &mut cfg.three.enabled,
        1 => &mut cfg.four.enabled,
        _ => &mut cfg.five.enabled,
    };
    group(ui, t, |g| {
        g.toggle_row(
            Some(icon),
            title,
            "オフにすると、この本数のジェスチャーは無視されます。",
            flag,
        );
    });
    let on = *flag;
    let mut cmd = windows_conflict(ui, t, win, n, on);
    if on
        && cfg
            .drag
            .row(n as usize)
            .is_some_and(|r| r.enabled && r.start == DragStart::TouchMove)
    {
        group(ui, t, |g| {
            g.wrapped_row(|ui| {
                ui.label(
                    RichText::new(format!(
                        "ドラッグに {n} 本指を使っているため、スワイプは使えません"
                    ))
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
        section_header(ui, t, "タップ");
        group(ui, t, |g| {
            let (id, slot) = match tab {
                0 => ("three-tap", &mut cfg.three.tap),
                1 => ("four-tap", &mut cfg.four.tap),
                _ => ("five-tap", &mut cfg.five.tap),
            };
            g.action_row("タップ", &format!("{n}本指で軽く触れたとき"), id, slot);
        });
        section_header(ui, t, "スワイプ");
        group(ui, t, |g| {
            let rows: [(&str, &str, &str, &mut Action); 4] = match tab {
                0 => {
                    let f = &mut cfg.three;
                    [
                        ("左にスワイプ", "左", "three-left", &mut f.swipe_left),
                        ("右にスワイプ", "右", "three-right", &mut f.swipe_right),
                        ("上にスワイプ", "上", "three-up", &mut f.swipe_up),
                        ("下にスワイプ", "下", "three-down", &mut f.swipe_down),
                    ]
                }
                1 => {
                    let f = &mut cfg.four;
                    [
                        ("左にスワイプ", "左", "four-left", &mut f.swipe_left),
                        ("右にスワイプ", "右", "four-right", &mut f.swipe_right),
                        ("上にスワイプ", "上", "four-up", &mut f.swipe_up),
                        ("下にスワイプ", "下", "four-down", &mut f.swipe_down),
                    ]
                }
                _ => {
                    let f = &mut cfg.five;
                    [
                        ("左にスワイプ", "左", "five-left", &mut f.swipe_left),
                        ("右にスワイプ", "右", "five-right", &mut f.swipe_right),
                        ("上にスワイプ", "上", "five-up", &mut f.swipe_up),
                        ("下にスワイプ", "下", "five-down", &mut f.swipe_down),
                    ]
                }
            };
            for (title, dir, id, slot) in rows {
                g.action_row(title, &format!("{n}本指で{dir}へなぞったとき"), id, slot);
            }
        });
        section_header(ui, t, "ピンチ");
        group(ui, t, |g| {
            let (a, b) = match tab {
                0 => (
                    ("three-in", &mut cfg.three.pinch_in),
                    ("three-out", &mut cfg.three.pinch_out),
                ),
                1 => (
                    ("four-in", &mut cfg.four.pinch_in),
                    ("four-out", &mut cfg.four.pinch_out),
                ),
                _ => (
                    ("five-in", &mut cfg.five.pinch_in),
                    ("five-out", &mut cfg.five.pinch_out),
                ),
            };
            g.action_row_of(
                "ピンチイン",
                &format!("{n}本指で指を寄せたとき"),
                a.0,
                a.1,
                &ActionKind::PINCH,
            );
            g.action_row_of(
                "ピンチアウト",
                &format!("{n}本指で指を広げたとき"),
                b.0,
                b.1,
                &ActionKind::PINCH,
            );
        });
    });
    cmd
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
        widgets::sections2_ordered(ui, t, &[0, 1, 2], &[3, 4, 5, 6], &[0, 3, 1, 2, 4, 5, 6], |ui, i| {
        if i == 0 {
        section_header(ui, t, "使い方");
        group(ui, t, |g| {
            g.wrapped_row(|ui| press_guide(ui, t, cfg));
        });
        return;
        }
        if i == 3 {
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
        return;
        }
        {
            let n = if i < 3 { i } else { i - 1 };
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
    });
    // Keep deep above light whichever slider was moved.
    cfg.normalize();
    cmd
}

/// The long hold page: what a 1 to 5 finger long hold does. "リングメニュー" on 3 to 5 fingers is
/// what makes the ring open there; the ring page only shows which counts those are.
pub fn long_hold(ui: &mut Ui, t: &Tokens, cfg: &mut Config) -> Option<Cmd> {
    let on = feature_top(ui, t, cfg, Page::LongHold);
    if on {
        overlap_note(ui, t, cfg);
    };
    let mut cmd = None;
    ui.add_enabled_ui(on, |ui| {
        section_header(ui, t, "使い方");
        group(ui, t, |g| {
            g.wrapped_row(|ui| hold_guide(ui, t, cfg));
        });
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
        section_header(ui, t, "使い方");
        group(ui, t, |g| {
            g.wrapped_row(|ui| combo_guide(ui, t, thumb, &*list));
        });
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
    // Corner zones in use are left out of the bands (corners: TL, TR, BL, BR).
    let gaps = |side: Side| {
        let (a, b) = match side {
            Side::Left => (0, 2),
            Side::Right => (1, 3),
            Side::Top => (0, 1),
        };
        [a, b].map(|i| {
            if cfg.corner.owns(i) {
                cfg.corner.size_mm
            } else {
                0.0
            }
        })
    };
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
            gap_mm: gaps(side),
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
            if cfg.drag.on(n)
                || (cfg.three.enabled && n == 3)
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

/// How custom press works: the depth bar with the light and deep marks.
fn press_guide(ui: &mut Ui, t: &Tokens, cfg: &Config) {
    // The animation shows the first finger count that is switched on (2 when none is).
    let n = (1..=5usize)
        .find(|n| cfg.press.fingers[n - 1].enabled)
        .unwrap_or(2);
    ui.vertical(|ui| {
        let anim = howto::Anim::new(ui);
        let step = std::cell::Cell::new(0);
        howto::two_col(
            ui,
            t,
            howto_scenes::press_scene_width(ui.available_width()),
            |ui| step.set(howto_scenes::press_scene(ui, t, anim, n)),
            |ui| {
                howto::summary(
                    ui,
                    t,
                    "押し込む強さで、操作を使い分けます",
                    &[
                        "指を置いたまま、トラックパッドを軽く押し込むと「軽い押し込み」、さらに深く押し込むと「深い押し込み」の操作が実行されます。".to_owned(),
                        "強さの目安は本数ごとのスライダーで決めます。本数が多いほど力が分かれるため、低めの値が向いています。".to_owned(),
                    ],
                );
                ui.add_space(t.space_md);
                howto::steps(
                    ui,
                    t,
                    &[Mini::RingPlace(n), Mini::Push(n, false), Mini::Push(n, true)],
                    &[&format!("{n}本の指を置く"), "軽く押し込む", "深く押し込む"],
                    anim.live().then_some(step.get()),
                );
            },
        );
    });
}

/// How the long hold works: a ring fills while the fingers rest, then the action runs.
fn hold_guide(ui: &mut Ui, t: &Tokens, cfg: &Config) {
    let n = (1..=5usize)
        .find(|n| *cfg.long_hold.actions.get(*n) != Action::None)
        .unwrap_or(1);
    let hold_ms = cfg.sensitivity.hold_ms;
    ui.vertical(|ui| {
        let anim = howto::Anim::new(ui);
        let step = std::cell::Cell::new(0);
        howto::two_col(
            ui,
            t,
            howto_scenes::hold_scene_width(ui.available_width()),
            |ui| step.set(howto_scenes::hold_scene(ui, t, anim, n, hold_ms)),
            |ui| {
                howto::summary(
                    ui,
                    t,
                    "指を置いたままにして、操作を実行します",
                    &[
                        format!("指を {hold_ms} ms ほど動かさずに置き続けると、本数ごとに割り当てた操作が実行されます。"),
                        "動かしたり、指の本数が変わったりすると、実行されずに取り消されます。".to_owned(),
                    ],
                );
                ui.add_space(t.space_md);
                howto::steps(
                    ui,
                    t,
                    &[Mini::RingPlace(n), Mini::RingWait(n), Mini::Run(n)],
                    &[&format!("{n}本の指を置く"), "動かさずに待つ", "操作が実行される"],
                    anim.live().then_some(step.get()),
                );
            },
        );
    });
}

/// How the finger / thumb hold works: one finger rests, a second one taps or swipes.
fn combo_guide(ui: &mut Ui, t: &Tokens, thumb: bool, list: &HoldCombos) {
    let trigger = list.combos.first().map_or(HoldTrigger::Tap, |c| c.trigger);
    let keys = list.combos.first().map_or("", |c| c.keys.as_str());
    let who = if thumb { "親指" } else { "指" };
    let second = match trigger {
        HoldTrigger::Tap => "別の指でタップする",
        HoldTrigger::Swipe => "別の指でスワイプする",
    };
    ui.vertical(|ui| {
        let anim = howto::Anim::new(ui);
        let step = std::cell::Cell::new(0);
        howto::two_col(
            ui,
            t,
            howto_scenes::combo_scene_width(ui.available_width()),
            |ui| step.set(howto_scenes::combo_scene(ui, t, anim, thumb, trigger, keys)),
            |ui| {
                howto::summary(
                    ui,
                    t,
                    &format!("{who}を置いたまま、別の指でキーを送ります"),
                    &[
                        format!("{who}をトラックパッドに置いたまま、別の指でタップまたはスワイプすると、設定したキーの組み合わせが送られます。"),
                        "どの指を置くか、タップかスワイプかは、組み合わせごとに選べます。".to_owned(),
                    ],
                );
                ui.add_space(t.space_md);
                howto::steps(
                    ui,
                    t,
                    &[
                        Mini::Combo(thumb, howto::ComboStage::Hold),
                        Mini::Combo(thumb, howto::ComboStage::Second),
                        Mini::Run(2),
                    ],
                    &[&format!("{who}を置く"), second, "キーが送られる"],
                    anim.live().then_some(step.get()),
                );
            },
        );
    });
}
