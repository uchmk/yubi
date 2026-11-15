//! App shell: left navigation, page routing, input draining, debounced config saves.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eframe::egui::{self, Button, Frame, Margin, Panel, RichText, ScrollArea, Stroke, Ui, Vec2};

use crate::config::{self, Config};
use crate::input::InputEvent;
use crate::pages::{diagnostics::Diagnostics, settings};
use crate::theme::Tokens;

const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Diagnostics,
    Gestures,
    Press,
    Profiles,
    Ring,
}

impl Page {
    const ALL: [Self; 5] = [
        Self::Diagnostics,
        Self::Gestures,
        Self::Press,
        Self::Profiles,
        Self::Ring,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Diagnostics => "診断",
            Self::Gestures => "ジェスチャー",
            Self::Press => "押し込みと触覚",
            Self::Profiles => "アプリ別プロファイル",
            Self::Ring => "リングメニュー",
        }
    }
}

pub struct App {
    page: Page,
    cfg: Config,
    saved: Config,
    config_path: PathBuf,
    dirty_since: Option<Instant>,
    rx: Receiver<InputEvent>,
    diagnostics: Diagnostics,
}

impl App {
    pub fn new(rx: Receiver<InputEvent>) -> Self {
        let config_path = config::default_path();
        let cfg = config::load(&config_path);
        Self {
            page: Page::Diagnostics,
            saved: cfg.clone(),
            cfg,
            config_path,
            dirty_since: None,
            rx,
            diagnostics: Diagnostics::default(),
        }
    }

    fn drain_input(&mut self, now: f64) {
        while let Ok(ev) = self.rx.try_recv() {
            self.diagnostics.ingest(ev, now);
        }
    }

    /// Save at most once per `SAVE_DEBOUNCE` while the config has unsaved changes (throttle, so a long
    /// slider drag writes a few times rather than waiting for release).
    fn save_if_due(&mut self, ctx: &egui::Context, force: bool) {
        if self.cfg == self.saved {
            self.dirty_since = None;
            return;
        }
        let since = *self.dirty_since.get_or_insert_with(Instant::now);
        if force || since.elapsed() >= SAVE_DEBOUNCE {
            match config::save(&self.config_path, &self.cfg) {
                Ok(()) => self.saved = self.cfg.clone(),
                Err(e) => eprintln!("yubi: cannot save config: {e}"),
            }
            self.dirty_since = Some(Instant::now());
        }
        ctx.request_repaint_after(SAVE_DEBOUNCE);
    }

    fn sidebar(&mut self, ui: &mut Ui, t: &Tokens) {
        ui.add_space(t.space_sm);
        ui.label(RichText::new("yubi").strong().size(18.0));
        ui.add_space(t.space_md);
        let width = ui.available_width();
        for page in Page::ALL {
            let selected = self.page == page;
            // The marker and bold text show the selection without relying on color alone.
            let text = if selected {
                RichText::new(format!("▎{}", page.label()))
                    .strong()
                    .color(t.accent)
            } else {
                RichText::new(page.label())
            };
            let button = Button::new(text)
                .min_size(Vec2::new(width, 32.0))
                .fill(if selected {
                    t.accent_soft
                } else {
                    egui::Color32::TRANSPARENT
                })
                .stroke(Stroke::NONE)
                .truncate();
            if ui.add(button).clicked() {
                self.page = page;
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let t = Tokens::of(ui.ctx());
        let now = ui.input(|i| i.time);
        self.drain_input(now);

        Panel::left("nav")
            .exact_size(t.sidebar_width)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(t.surface)
                    .stroke(Stroke::new(1.0, t.border))
                    .inner_margin(Margin::same(t.space_md as i8)),
            )
            .show(ui, |ui| self.sidebar(ui, &t));

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(t.bg)
                    .inner_margin(Margin::same(t.space_lg as i8)),
            )
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.page {
                        Page::Diagnostics => self.diagnostics.ui(ui, &t, now),
                        Page::Gestures => settings::gestures(ui, &t, &mut self.cfg),
                        Page::Press => settings::press(ui, &t, &mut self.cfg),
                        Page::Profiles => settings::profiles(ui, &t, &mut self.cfg),
                        Page::Ring => settings::ring(ui, &t, &mut self.cfg),
                    });
            });

        let ctx = ui.ctx().clone();
        self.save_if_due(&ctx, false);
    }

    fn on_exit(&mut self) {
        if self.cfg != self.saved {
            let _ = config::save(&self.config_path, &self.cfg);
        }
    }
}
