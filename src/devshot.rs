//! Development aid: `YUBI_SHOT=<dir>` walks through pages, themes and window sizes, saves each frame as
//! a BMP into `<dir>`, then closes the app. The config on disk is never touched while it runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::{fs, io};

use eframe::egui::{self, ColorImage, Vec2, ViewportCommand};

use crate::config::ThemeChoice;
use crate::nav::Page;

/// Frames to wait after changing page/theme/size before capturing (layout and animations settle).
const SETTLE_FRAMES: u32 = 20;

const WIDE: Vec2 = Vec2::new(1200.0, 800.0);
const NARROW: Vec2 = Vec2::new(700.0, 520.0);

pub struct Shot {
    pub name: String,
    pub page: Page,
    pub theme: ThemeChoice,
    pub size: Vec2,
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
            ("haptic", Page::Haptic),
            ("monitor", Page::Monitor),
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
            })
            .collect();
        for &(n, page) in &[("home", Page::Home), ("haptic", Page::Haptic)] {
            shots.push(Shot {
                name: format!("light-{n}"),
                page,
                theme: ThemeChoice::Light,
                size: WIDE,
            });
        }
        for &(n, page) in &[
            ("home", Page::Home),
            ("four", Page::Four),
            ("five", Page::Five),
            ("fingerhold", Page::FingerHold),
            ("diagnostics", Page::Diagnostics),
            ("monitor", Page::Monitor),
            ("general", Page::General),
        ] {
            shots.push(Shot {
                name: format!("narrow-{n}"),
                page,
                theme: ThemeChoice::Black,
                size: NARROW,
            });
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
