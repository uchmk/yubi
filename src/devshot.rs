//! Development aid: `YUBI_SHOT=<dir>` walks through pages, themes and window sizes, saves each frame as
//! a BMP into `<dir>`, then closes the app. The config on disk is never touched while it runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::{fs, io};

use eframe::egui::{self, ColorImage, Vec2, ViewportCommand};

use crate::config::{Action, Config, EdgeTarget, Profile, ThemeChoice};
use crate::nav::Page;

/// Frames to wait after changing page/theme/size before capturing (layout and animations settle).
const SETTLE_FRAMES: u32 = 20;
/// The picker popup fades in by wall-clock time, so its shots wait longer.
const PICKER_SETTLE_FRAMES: u32 = 90;

const WIDE: Vec2 = Vec2::new(1200.0, 800.0);
const NARROW: Vec2 = Vec2::new(700.0, 520.0);
/// The window's minimum inner size.
const MINIMUM: Vec2 = Vec2::new(640.0, 420.0);

/// The per-app data a shot runs with, so the pages show the same thing on every machine.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fixture {
    /// Whatever is on disk.
    Keep,
    /// No profiles at all.
    NoProfiles,
    /// A few profiles with overrides, a stopped app and long names; some conflicting settings.
    DemoProfiles,
    /// `DemoProfiles` with the cursor speed managed (12) and a 16 in the first profile.
    DemoSpeedOn,
    /// `DemoProfiles` with the cursor speed not managed.
    DemoSpeedOff,
    /// Corner swipes on, a mix of set and off slots (one is the window resize).
    CornerDemo,
    /// `DemoProfiles` plus a 3-finger drag and a 3-finger long hold that opens the ring.
    OverlapDemo,
    /// Every corner trigger on with slots set in corner 0 (all six) and a few elsewhere; the corner
    /// page shows tab `tab` with the slot `sel` of corner 0 selected (0: none, else 1 + the index in
    /// `CornerSlot::ALL`). With `off` the tab's trigger is switched off.
    CornerView { tab: u8, sel: u8, off: bool },
    /// `DemoProfiles` and `CornerView` data: the edge bands leave gaps at the corners in use.
    CornerEdge,
    /// A ring with eight items, including an app launch with a long path.
    RingFull,
    /// The ジェスチャー page on tab `0` (4 fingers) or `1` (5 fingers), with some actions set.
    GestureTab(u8),
    /// The first action picker on the page is open: 0 unfiltered, 1 filtered, 2 nothing matches.
    Picker(u8),
}

impl Fixture {
    /// Corners on, every trigger on, corner 0 with all six slots set.
    fn corner_all(cfg: &mut Config) {
        Self::CornerDemo.apply(cfg);
        cfg.corner.tap_enabled = true;
        cfg.corner.swipe_enabled = true;
        cfg.corner.press_enabled = true;
        cfg.corner.hold_enabled = true;
        let c = &mut cfg.corner.corners;
        c[0].tap = Action::TaskView;
        c[0].press = Action::Find;
        c[0].hold = Action::ShowDesktop;
        c[1].tap = Action::DesktopNext;
        c[2].hold = Action::Find;
        c[3].press = Action::ShowDesktop;
    }

    /// Set the fixture's data (idempotent: called every frame).
    pub fn apply(self, cfg: &mut Config) {
        crate::widgets::set_picker_preview(match self {
            Self::Picker(0) => Some(""),
            Self::Picker(1) => Some("デスク"),
            Self::Picker(_) => Some("zzz"),
            _ => None,
        });
        match self {
            Self::Keep | Self::Picker(_) => {}
            Self::NoProfiles => cfg.profiles.clear(),
            Self::CornerDemo => {
                cfg.gestures_enabled = true;
                cfg.corner.enabled = true;
                let c = &mut cfg.corner.corners;
                c[0].horizontal = Action::Find;
                c[0].diagonal = Action::ResizeFromOpposite;
                c[1].vertical = Action::ShowDesktop;
                c[1].diagonal = Action::TaskView;
                c[2].horizontal = Action::DesktopPrev;
                c[3].horizontal = Action::DesktopNext;
                c[3].vertical = Action::Find;
                c[3].diagonal = Action::ResizeFromOpposite;
            }
            Self::OverlapDemo => {
                Self::DemoProfiles.apply(cfg);
                cfg.drag.enabled = true;
                cfg.drag.fingers = 3;
                cfg.long_hold.enabled = true;
                cfg.ring.enabled = true;
                *cfg.long_hold.actions.get_mut(3) = Action::RingMenu;
            }
            Self::CornerView { tab, sel, off } => {
                Self::corner_all(cfg);
                let tr = crate::config::CornerTrigger::ALL[usize::from(tab) % 4];
                *cfg.corner.trigger_enabled_mut(tr) = !off;
                let selected = usize::from(sel)
                    .checked_sub(1)
                    .map(|i| (0, crate::config::CornerSlot::ALL[i % 6]));
                crate::pages::corner::force_view(tr, selected);
            }
            Self::CornerEdge => {
                Self::DemoProfiles.apply(cfg);
                Self::corner_all(cfg);
                cfg.edge.top = EdgeTarget::Scroll;
                cfg.edge.right = EdgeTarget::Volume;
                // Bottom corners only: the top corners stay free for the band.
                for i in [0, 1] {
                    cfg.corner.corners[i] = crate::config::CornerCfg::default();
                }
            }
            Self::GestureTab(tab) => {
                cfg.gestures_enabled = true;
                cfg.four.enabled = true;
                cfg.five.enabled = true;
                cfg.five.tap = Action::Find;
                cfg.five.swipe_left = Action::DesktopPrev;
                cfg.five.swipe_right = Action::DesktopNext;
                cfg.five.swipe_up = Action::TaskView;
                cfg.five.pinch_in = Action::ShowDesktop;
                cfg.drag.fingers = 3;
                crate::pages::features::force_gesture_tab(usize::from(tab));
            }
            Self::RingFull => {
                use crate::config::RingItem;
                cfg.ring.enabled = true;
                let item = |label: &str, action: Action| RingItem {
                    label: label.into(),
                    action,
                };
                cfg.ring.items = vec![
                    item("タスクビュー", Action::TaskView),
                    item("コピー", Action::Keys("Ctrl+C".into())),
                    item("音量を上げる", Action::VolumeStep(5)),
                    item("検索", Action::Find),
                    item(
                        "アプリを起動",
                        Action::Launch {
                            path: r"C:\Windows\System32\notepad.exe".into(),
                            args: String::new(),
                        },
                    ),
                    item("デスクトップを表示", Action::ShowDesktop),
                    item("ミュート", Action::ToggleMute),
                    item("次のデスクトップ", Action::DesktopNext),
                ];
            }
            Self::DemoSpeedOn | Self::DemoSpeedOff => {
                Self::DemoProfiles.apply(cfg);
                let on = self == Self::DemoSpeedOn;
                cfg.cursor.speed = on.then_some(12);
                cfg.profiles[0].cursor_speed = on.then_some(16);
                cfg.scroll.enabled = true;
                cfg.profiles[0].scroll_speed = on.then_some(1.5);
            }
            Self::DemoProfiles => {
                cfg.gestures_enabled = true;
                cfg.four.enabled = true;
                cfg.four.swipe_left = Action::DesktopNext;
                cfg.four.swipe_right = Action::DesktopPrev;
                cfg.four.tap = Action::MiddleClick;
                cfg.five.enabled = false;
                cfg.five.pinch_in = Action::ShowDesktop;
                cfg.drag.enabled = true;
                cfg.drag.fingers = 4;
                cfg.edge.enabled = true;
                cfg.edge.left = EdgeTarget::Volume;
                let mut code = Profile {
                    exe_name: "code.exe".into(),
                    display_name: "Visual Studio Code".into(),
                    enabled: true,
                    ..Profile::default()
                };
                code.overrides
                    .insert("swipe.4.left".into(), Action::Keys("Ctrl+PageUp".into()));
                code.overrides
                    .insert("swipe.4.right".into(), Action::Keys("Ctrl+PageDown".into()));
                code.overrides.insert("tap.4".into(), Action::None);
                code.edge_overrides
                    .insert("edge.left".into(), EdgeTarget::Scroll);
                let mut browser = Profile {
                    exe_name: "chrome.exe".into(),
                    enabled: true,
                    ..Profile::default()
                };
                browser
                    .overrides
                    .insert("swipe.4.left".into(), Action::TabSwitch);
                let long = Profile {
                    exe_name: "very-long-launcher-executable-name-for-the-layout-check.exe".into(),
                    display_name:
                        "とても長い名前のゲームランチャー（レイアウト確認用の日本語表示名）".into(),
                    enabled: false,
                    ..Profile::default()
                };
                let quiet = Profile {
                    exe_name: "notepad.exe".into(),
                    enabled: true,
                    ..Profile::default()
                };
                cfg.profiles = vec![code, browser, long, quiet];
            }
        }
    }
}

pub struct Shot {
    pub name: String,
    pub page: Page,
    pub theme: ThemeChoice,
    pub size: Vec2,
    pub fixture: Fixture,
    /// The profile whose editor is open (per-app page only).
    pub profile: Option<usize>,
    /// Show a made-up pad state (fingers and judgements) instead of the engine's.
    pub fake: bool,
    /// Switch 練習モード on.
    pub practice: bool,
    /// Pin the how-to animations to this time (seconds); `None` lets them run.
    pub anim_t: Option<f32>,
    /// Behave as if Windows' animation effects were off.
    pub reduced: bool,
}

pub struct DevShot {
    dir: PathBuf,
    shots: Vec<Shot>,
    index: usize,
    frames: u32,
    requested: bool,
    tx: Sender<Arc<ColorImage>>,
    rx: Receiver<Arc<ColorImage>>,
}

impl DevShot {
    pub fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("YUBI_SHOT")?);
        let all = [
            ("home", Page::Home),
            ("drag", Page::Drag),
            ("gesture", Page::Gesture),
            ("press", Page::CustomPress),
            ("longhold", Page::LongHold),
            ("fingerhold", Page::FingerHold),
            ("thumbhold", Page::ThumbHold),
            ("corner", Page::Corner),
            ("edge", Page::Edge),
            ("ring", Page::Ring),
            ("profiles", Page::Profiles),
            ("sensitivity", Page::Sensitivity),
            ("haptic", Page::Haptic),
            ("practice", Page::Practice),
            ("diagnostics", Page::Diagnostics),
            ("general", Page::General),
        ];
        let mut shots: Vec<Shot> = all
            .iter()
            .map(|&(n, page)| Shot {
                name: format!("black-{n}"),
                page,
                theme: ThemeChoice::Black,
                size: WIDE,
                fixture: Fixture::Keep,
                profile: None,
                fake: false,
                practice: false,
                anim_t: None,
                reduced: false,
            })
            .collect();
        for &(n, page) in &[("home", Page::Home), ("haptic", Page::Haptic)] {
            shots.push(Shot {
                name: format!("light-{n}"),
                page,
                theme: ThemeChoice::Light,
                size: WIDE,
                fixture: Fixture::Keep,
                profile: None,
                fake: false,
                practice: false,
                anim_t: None,
                reduced: false,
            });
        }
        for &(n, page) in &[
            ("home", Page::Home),
            ("gesture", Page::Gesture),
            ("longhold", Page::LongHold),
            ("edge", Page::Edge),
            ("ring", Page::Ring),
            ("sensitivity", Page::Sensitivity),
            ("fingerhold", Page::FingerHold),
            ("diagnostics", Page::Diagnostics),
            ("practice", Page::Practice),
            ("general", Page::General),
        ] {
            shots.push(Shot {
                name: format!("narrow-{n}"),
                page,
                theme: ThemeChoice::Black,
                size: NARROW,
                fixture: Fixture::Keep,
                profile: None,
                fake: false,
                practice: false,
                anim_t: None,
                reduced: false,
            });
        }
        // The 感度 page tall enough to show the cursor and scroll sections without scrolling.
        for (n, w) in [("wide", WIDE.x), ("narrow", NARROW.x)] {
            shots.push(Shot {
                name: format!("sensitivity-tall-{n}"),
                page: Page::Sensitivity,
                theme: ThemeChoice::Black,
                size: Vec2::new(w, 1100.0),
                fixture: Fixture::Keep,
                profile: None,
                fake: false,
                practice: false,
                anim_t: None,
                reduced: false,
            });
        }
        // The ring page with a full ring (eight items), tall enough to show every row.
        for (n, w) in [("wide", WIDE.x), ("narrow", NARROW.x)] {
            shots.push(Shot {
                name: format!("ringfull-{n}"),
                page: Page::Ring,
                theme: ThemeChoice::Black,
                size: Vec2::new(w, 1500.0),
                fixture: Fixture::RingFull,
                profile: None,
                fake: false,
                practice: false,
                anim_t: None,
                reduced: false,
            });
        }
        if std::env::var_os("YUBI_SHOT_NAV").is_some() {
            // The navigation (both groups open) and the ジェスチャー page, both tabs, dark and light.
            let mut list = Vec::new();
            list.push(Shot {
                name: "nav-wide".into(),
                page: Page::Drag,
                theme: ThemeChoice::Black,
                size: Vec2::new(WIDE.x, 900.0),
                fixture: Fixture::Keep,
                profile: None,
                fake: false,
                practice: false,
                anim_t: None,
                reduced: false,
            });
            for tab in 0..2u8 {
                for (n, theme) in [("dark", ThemeChoice::Black), ("light", ThemeChoice::Light)] {
                    list.push(Shot {
                        name: format!("gesture-tab{}-{n}", 4 + tab),
                        page: Page::Gesture,
                        theme,
                        size: Vec2::new(WIDE.x, 1000.0),
                        fixture: Fixture::GestureTab(tab),
                        profile: None,
                        fake: false,
                        practice: false,
                        anim_t: None,
                        reduced: false,
                    });
                }
            }
            shots = list;
        }
        // The practice page with a made-up state: three fingers down and a short history.
        for (n, size) in [("wide", WIDE), ("narrow", NARROW)] {
            for (mode, on) in [("on", true), ("off", false)] {
                shots.push(Shot {
                    name: format!("practice-fake-{mode}-{n}"),
                    page: Page::Practice,
                    theme: ThemeChoice::Black,
                    size,
                    fixture: Fixture::Keep,
                    profile: None,
                    fake: true,
                    practice: on,
                    anim_t: None,
                    reduced: false,
                });
            }
        }
        // The same page tall enough to show 判定 and 最近の判定 (the edge slider entry) unscrolled.
        for (n, theme) in [("black", ThemeChoice::Black), ("light", ThemeChoice::Light)] {
            shots.push(Shot {
                name: format!("practice-fake-tall-{n}"),
                page: Page::Practice,
                theme,
                size: Vec2::new(1100.0, 1500.0),
                fixture: Fixture::Keep,
                profile: None,
                fake: true,
                practice: true,
                anim_t: None,
                reduced: false,
            });
        }
        // Per-app pages with fixed data: list, empty list, two editors, the overview.
        for (size_name, size) in [("wide", WIDE), ("narrow", NARROW)] {
            for (n, page, fixture, profile) in [
                ("apps-list", Page::Profiles, Fixture::DemoProfiles, None),
                ("apps-empty", Page::Profiles, Fixture::NoProfiles, None),
                (
                    "apps-detail",
                    Page::Profiles,
                    Fixture::DemoProfiles,
                    Some(0),
                ),
                (
                    "apps-detail-long",
                    Page::Profiles,
                    Fixture::DemoProfiles,
                    Some(2),
                ),
                (
                    "apps-detail-speed-on",
                    Page::Profiles,
                    Fixture::DemoSpeedOn,
                    Some(0),
                ),
                (
                    "apps-detail-speed-off",
                    Page::Profiles,
                    Fixture::DemoSpeedOff,
                    Some(0),
                ),
                ("overview", Page::Overview, Fixture::DemoProfiles, None),
                ("overview-empty", Page::Overview, Fixture::NoProfiles, None),
            ] {
                // The speed row sits low in the editor: a taller window shows it without scrolling.
                let size = if n.contains("speed") {
                    Vec2::new(size.x, 1000.0)
                } else {
                    size
                };
                shots.push(Shot {
                    name: format!("cfg-{size_name}-{n}"),
                    page,
                    theme: ThemeChoice::Black,
                    size,
                    fixture,
                    profile,
                    fake: false,
                    practice: false,
                    anim_t: None,
                    reduced: false,
                });
            }
        }
        if let Ok(mode) = std::env::var("YUBI_SHOT_PRESS") {
            // Only the press pages, on a fake pad (see `pages::press::demo`).
            let mut list = Vec::new();
            for (n, page, theme, size) in [
                ("haptic", Page::Haptic, ThemeChoice::Black, WIDE),
                ("haptic-light", Page::Haptic, ThemeChoice::Light, WIDE),
                ("haptic-narrow", Page::Haptic, ThemeChoice::Black, NARROW),
                ("custom", Page::CustomPress, ThemeChoice::Black, WIDE),
                ("custom-light", Page::CustomPress, ThemeChoice::Light, WIDE),
                (
                    "custom-narrow",
                    Page::CustomPress,
                    ThemeChoice::Black,
                    NARROW,
                ),
            ] {
                list.push(Shot {
                    name: format!("{mode}-{n}"),
                    page,
                    theme,
                    size,
                    fixture: Fixture::Keep,
                    profile: None,
                    fake: false,
                    practice: false,
                    anim_t: None,
                    reduced: false,
                });
            }
            shots = list;
        }
        if std::env::var_os("YUBI_SHOT_OVERLAP").is_some() {
            // The overview's overlap entry and the notes on the drag and long hold pages.
            let mut list = Vec::new();
            for (n, size) in [("wide", WIDE), ("narrow", NARROW)] {
                for (pn, page) in [
                    ("overview", Page::Overview),
                    ("drag", Page::Drag),
                    ("longhold", Page::LongHold),
                ] {
                    list.push(Shot {
                        name: format!("overlap-{n}-{pn}"),
                        page,
                        theme: ThemeChoice::Black,
                        size,
                        fixture: Fixture::OverlapDemo,
                        profile: None,
                        fake: false,
                        practice: false,
                        anim_t: None,
                        reduced: false,
                    });
                }
            }
            shots = list;
        }
        if std::env::var_os("YUBI_SHOT_ANIM").is_some() {
            // Only the how-to illustrations, at fixed times, in both themes and sizes.
            let mut list = Vec::new();
            for (size_name, size) in [("wide", WIDE), ("narrow", NARROW), ("min", MINIMUM)] {
                for (n, page, fixture, profile, times) in [
                    (
                        "edge",
                        Page::Edge,
                        Fixture::DemoProfiles,
                        None,
                        &[0.15, 0.5, 2.0][..],
                    ),
                    (
                        "ring",
                        Page::Ring,
                        Fixture::Keep,
                        None,
                        &[0.2, 0.8, 2.0, 3.0][..],
                    ),
                    (
                        "apps",
                        Page::Profiles,
                        Fixture::NoProfiles,
                        None,
                        &[0.0][..],
                    ),
                ] {
                    for (i, &t) in times.iter().enumerate() {
                        list.push(Shot {
                            name: format!("anim-{n}-{size_name}-{i}"),
                            page,
                            theme: ThemeChoice::Black,
                            size,
                            fixture,
                            profile,
                            fake: false,
                            practice: false,
                            anim_t: Some(t),
                            reduced: false,
                        });
                    }
                }
                for (n, page, theme) in [
                    ("edge-light", Page::Edge, ThemeChoice::Light),
                    ("ring-light", Page::Ring, ThemeChoice::Light),
                    ("edge-still", Page::Edge, ThemeChoice::Black),
                    ("ring-still", Page::Ring, ThemeChoice::Black),
                ] {
                    list.push(Shot {
                        name: format!("anim-{n}-{size_name}"),
                        page,
                        theme,
                        size,
                        fixture: Fixture::DemoProfiles,
                        profile: None,
                        fake: false,
                        practice: false,
                        anim_t: (!n.ends_with("still")).then_some(2.0),
                        reduced: n.ends_with("still"),
                    });
                }
            }
            shots = list;
        }
        if std::env::var_os("YUBI_SHOT_PICKER").is_some() {
            let mut list = Vec::new();
            // `YUBI_SHOT_PICKER=wide|narrow` limits the window size; anything else takes both.
            let only = std::env::var("YUBI_SHOT_PICKER").unwrap_or_default();
            for (n, size) in [("wide", WIDE), ("narrow", NARROW)] {
                if (only == "wide" || only == "narrow") && only != n {
                    continue;
                }
                for (tn, theme) in [("dark", ThemeChoice::Black), ("light", ThemeChoice::Light)] {
                    for (fn_, stage) in [("open", 0u8), ("filtered", 1), ("none", 2)] {
                        list.push(Shot {
                            name: format!("picker-{fn_}-{n}-{tn}"),
                            page: Page::Gesture,
                            theme,
                            size,
                            fixture: Fixture::Picker(stage),
                            profile: None,
                            fake: false,
                            practice: false,
                            anim_t: None,
                            reduced: false,
                        });
                    }
                }
            }
            shots = list;
        }
        if std::env::var("YUBI_SHOT_CORNER").is_ok() {
            let mut list = Vec::new();
            // Tall enough to show the whole page without scrolling.
            for (n, size) in [
                ("wide", Vec2::new(WIDE.x, 1250.0)),
                ("narrow", Vec2::new(NARROW.x, 1650.0)),
            ] {
                for (tn, theme) in [("dark", ThemeChoice::Black), ("light", ThemeChoice::Light)] {
                    list.push(Shot {
                        name: format!("corner-{n}-{tn}"),
                        page: Page::Corner,
                        theme,
                        size,
                        fixture: Fixture::CornerDemo,
                        profile: None,
                        fake: false,
                        practice: false,
                        anim_t: None,
                        reduced: false,
                    });
                }
            }
            shots = list;
        }
        if std::env::var_os("YUBI_SHOT_ANIM2").is_some() {
            let mut list = Vec::new();
            let tall = |w: f32, h: f32| Vec2::new(w, h);
            let mut push = |name: String,
                            page: Page,
                            theme: ThemeChoice,
                            size: Vec2,
                            fixture: Fixture,
                            anim_t: Option<f32>| {
                list.push(Shot {
                    name,
                    page,
                    theme,
                    size,
                    fixture,
                    profile: None,
                    fake: false,
                    practice: false,
                    anim_t,
                    reduced: false,
                });
            };
            let dark = ThemeChoice::Black;
            let light = ThemeChoice::Light;
            let tabs = ["tap", "swipe", "press", "hold"];
            // Corner page: each tab, nothing selected and a slot selected; wide, narrow, light, off.
            for (tab, tn) in tabs.iter().enumerate() {
                let tab = tab as u8;
                // 1-based into `CornerSlot::ALL`: tap 4, swipe diagonal 3, press 5, hold 6.
                let sel = [4u8, 3, 5, 6][usize::from(tab)];
                for (sn, s) in [("none", 0u8), ("sel", sel)] {
                    let f = Fixture::CornerView {
                        tab,
                        sel: s,
                        off: false,
                    };
                    push(
                        format!("corner-{tn}-{sn}-wide-dark"),
                        Page::Corner,
                        dark,
                        tall(1200.0, 1700.0),
                        f,
                        Some(1.2),
                    );
                    push(
                        format!("corner-{tn}-{sn}-narrow-dark"),
                        Page::Corner,
                        dark,
                        tall(700.0, 2500.0),
                        f,
                        Some(1.2),
                    );
                }
                let f = Fixture::CornerView {
                    tab,
                    sel,
                    off: false,
                };
                push(
                    format!("corner-{tn}-sel-wide-light"),
                    Page::Corner,
                    light,
                    tall(1200.0, 1700.0),
                    f,
                    Some(1.2),
                );
                let f = Fixture::CornerView {
                    tab,
                    sel,
                    off: true,
                };
                push(
                    format!("corner-{tn}-off-wide-dark"),
                    Page::Corner,
                    dark,
                    tall(1200.0, 1700.0),
                    f,
                    Some(1.2),
                );
            }
            // The corner scene at a few times (the page's first screen).
            for (tab, tn) in tabs.iter().enumerate() {
                for (i, t) in [0.3f32, 0.9, 1.6, 2.4].into_iter().enumerate() {
                    let f = Fixture::CornerView {
                        tab: tab as u8,
                        sel: 0,
                        off: false,
                    };
                    push(
                        format!("scene-corner-{tn}-{i}"),
                        Page::Corner,
                        dark,
                        tall(1200.0, 700.0),
                        f,
                        Some(t),
                    );
                }
            }
            // The press, hold and combo scenes.
            for (n, page) in [
                ("press", Page::CustomPress),
                ("hold", Page::LongHold),
                ("finger", Page::FingerHold),
                ("thumb", Page::ThumbHold),
            ] {
                for (i, t) in [0.2f32, 1.2, 2.2, 3.0].into_iter().enumerate() {
                    push(
                        format!("scene-{n}-{i}"),
                        page,
                        dark,
                        tall(1200.0, 700.0),
                        Fixture::Keep,
                        Some(t),
                    );
                }
                push(
                    format!("scene-{n}-light"),
                    page,
                    light,
                    tall(1200.0, 700.0),
                    Fixture::Keep,
                    Some(2.2),
                );
                push(
                    format!("scene-{n}-narrow"),
                    page,
                    dark,
                    tall(700.0, 900.0),
                    Fixture::Keep,
                    Some(2.2),
                );
            }
            // The edge scene with corner gaps.
            for (tn, theme) in [("dark", dark), ("light", light)] {
                for (i, t) in [0.5f32, 2.0].into_iter().enumerate() {
                    push(
                        format!("scene-edge-gaps-{tn}-{i}"),
                        Page::Edge,
                        theme,
                        tall(1200.0, 800.0),
                        Fixture::CornerEdge,
                        Some(t),
                    );
                }
            }
            push(
                "scene-edge-gaps-narrow".to_owned(),
                Page::Edge,
                dark,
                tall(700.0, 900.0),
                Fixture::CornerEdge,
                Some(2.0),
            );
            shots = list;
        }
        let _ = fs::create_dir_all(&dir);
        let (tx, rx) = channel();
        Some(Self {
            tx,
            rx,
            dir,
            shots,
            index: 0,
            frames: 0,
            requested: false,
        })
    }

    /// The shot currently being prepared, if any.
    pub fn current(&self) -> Option<&Shot> {
        self.shots.get(self.index)
    }

    /// Drive the capture; call once per frame after the UI is built.
    pub fn tick(&mut self, ctx: &egui::Context) {
        ctx.request_repaint();
        let Some(shot) = self.shots.get(self.index) else {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        };
        if self.frames == 0 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(shot.size));
        }
        self.frames += 1;
        let settle = if matches!(shot.fixture, Fixture::Picker(_)) {
            PICKER_SETTLE_FRAMES
        } else {
            SETTLE_FRAMES
        };
        if self.frames == settle && !self.requested {
            self.requested = true;
            let tx = self.tx.clone();
            let repaint = ctx.clone();
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::ScreenshotCallback::new(
                move |image| {
                    let _ = tx.send(image);
                    repaint.request_repaint();
                },
            )));
        }
        if let Ok(image) = self.rx.try_recv() {
            let path = self.dir.join(format!("{}.bmp", shot.name));
            if let Err(e) = write_bmp(&path, &image) {
                eprintln!("yubi: cannot write {}: {e}", path.display());
            }
            self.index += 1;
            self.frames = 0;
            self.requested = false;
        }
    }
}

/// 32-bit top-down BMP (BGRA). No dependency needed.
fn write_bmp(path: &Path, img: &ColorImage) -> io::Result<()> {
    let [w, h] = img.size;
    let data_len = w * h * 4;
    let mut out = Vec::with_capacity(54 + data_len);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for p in &img.pixels {
        out.extend_from_slice(&[p.b(), p.g(), p.r(), 255]);
    }
    fs::write(path, out)
}
