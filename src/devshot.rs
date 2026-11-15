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

const WIDE: Vec2 = Vec2::new(1200.0, 800.0);
const NARROW: Vec2 = Vec2::new(700.0, 520.0);

/// The per-app data a shot runs with, so the pages show the same thing on every machine.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fixture {
    /// Whatever is on disk.
    Keep,
    /// No profiles at all.
    NoProfiles,
    /// A few profiles with overrides, a stopped app and long names; some conflicting settings.
    DemoProfiles,
}

impl Fixture {
    /// Set the fixture's data (idempotent: called every frame).
    pub fn apply(self, cfg: &mut Config) {
        match self {
            Self::Keep => {}
            Self::NoProfiles => cfg.profiles.clear(),
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
            ("four", Page::Four),
            ("five", Page::Five),
            ("press", Page::CustomPress),
            ("longhold", Page::LongHold),
            ("fingerhold", Page::FingerHold),
            ("thumbhold", Page::ThumbHold),
            ("corner", Page::Corner),
            ("edge", Page::Edge),
            ("window", Page::Window),
            ("ring", Page::Ring),
            ("profiles", Page::Profiles),
            ("sensitivity", Page::Sensitivity),
            ("haptic", Page::Haptic),
            ("monitor", Page::Monitor),
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
            });
        }
        for &(n, page) in &[
            ("home", Page::Home),
            ("four", Page::Four),
            ("five", Page::Five),
            ("edge", Page::Edge),
            ("sensitivity", Page::Sensitivity),
            ("fingerhold", Page::FingerHold),
            ("diagnostics", Page::Diagnostics),
            ("monitor", Page::Monitor),
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
            });
        }
        // The practice page with a made-up state: three fingers down and a short history.
        for (n, size) in [("wide", WIDE), ("narrow", NARROW)] {
            shots.push(Shot {
                name: format!("practice-fake-{n}"),
                page: Page::Practice,
                theme: ThemeChoice::Black,
                size,
                fixture: Fixture::Keep,
                profile: None,
                fake: true,
                practice: true,
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
                ("overview", Page::Overview, Fixture::DemoProfiles, None),
                ("overview-empty", Page::Overview, Fixture::NoProfiles, None),
            ] {
                shots.push(Shot {
                    name: format!("cfg-{size_name}-{n}"),
                    page,
                    theme: ThemeChoice::Black,
                    size,
                    fixture,
                    profile,
                    fake: false,
                    practice: false,
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
                });
            }
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
        if self.frames == SETTLE_FRAMES && !self.requested {
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
