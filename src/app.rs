//! App shell: top bar with search, navigation pane, page routing, input draining, debounced saves.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Area, CentralPanel, Frame, Id, Key, Margin, Order, Panel, Pos2, RichText, ScrollArea,
    Sense, Shape, Stroke, Ui,
};

use crate::config::{self, Config};
use crate::engine::Engine;
use crate::icons;
use crate::nav::{NAV, Node, Page};
use crate::pages::{
    Cmd, apps, diagnostics::Diagnostics, features, general, general_io, home, overview, press,
    sensitivity,
};
use crate::resident::{self, Resident};
use crate::search::{self, Entry};
use crate::theme::{self, Tokens};
use crate::widgets::{device_status, icon_button, nav_group_header, nav_item, paint_icon};
use crate::win_gestures;

const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

pub struct App {
    /// The file on disk was written by a newer yubi: it is never overwritten from here.
    read_only: bool,
    page: Page,
    cfg: Config,
    saved: Config,
    config_path: PathBuf,
    dirty_since: Option<Instant>,
    /// The old-layout file has no backup yet (see `config::backup_before_first_save`).
    needs_backup: bool,
    engine: Engine,
    diagnostics: Diagnostics,
    /// 押し込みと触覚 page: calibration wizard and meter state.
    press_ui: press::PressUi,
    /// Frame counter of the fake pad used by the `YUBI_SHOT_PRESS` screenshots.
    demo_frames: u64,
    /// Windows gesture registry values, read when the 全般 page opens and after each change.
    win_values: win_gestures::Values,
    /// Windows' two-finger scroll state (`None`: unknown).
    win_pan: Option<bool>,
    /// When `win_pan` was last read, and whether the window had focus in the previous frame.
    win_pan_read_at: Option<std::time::Instant>,
    was_focused: bool,
    win_read_error: Option<String>,
    win_message: Option<String>,
    win_page_was_open: bool,
    /// Export / import and haptic-permission state of the 全般 page.
    general_state: general_io::GeneralState,
    /// Per-app editor and overview page state.
    apps: apps::AppsState,
    overview: overview::OverviewState,
    applied_theme: config::ThemeChoice,
    /// User preference for the nav pane when the window is wide.
    nav_collapsed: bool,
    /// User override for the nav pane when the window is narrow (rail by default, full when true).
    nav_expanded_narrow: bool,
    group_open: [bool; NAV.len()],
    query: String,
    search_open: bool,
    search_index: Vec<Entry>,
    devshot: Option<crate::devshot::DevShot>,
    resident: Option<Resident>,
    pause_notice: crate::pause_notice::PauseNotice,
    quitting: bool,
    /// Frames left in which to keep the window hidden (`--background`; eframe shows it after the first frame).
    hide_frames: u8,
    /// `--background` without an icon after this time: show the window instead of staying invisible.
    icon_deadline: Option<Instant>,
    /// 練習モード (dry run): recognized gestures are shown, not run. Never saved.
    practice_on: bool,
    /// The pause generation last seen (`u64::MAX` before the first look, so the start is silent).
    seen_pause_gen: u64,
    /// The pause shortcut text last handed to the tray.
    applied_shortcut: Option<String>,
}

impl App {
    /// `loaded` is the one read of the config file (main reads it once: loading a v1 file rewrites
    /// it, so a second read would lose the warnings).
    pub fn new(ctx: &egui::Context, engine: Engine, loaded: config::Loaded) -> Self {
        let config_path = config::default_path();
        let config::Loaded {
            config: cfg,
            warnings,
            future_version,
            needs_backup,
        } = loaded;
        theme::apply(ctx, cfg.general.theme);
        let mut general_state = general_io::GeneralState::new(ctx);
        general_state.load_notes = warnings;
        general_state.read_only = future_version.is_some();
        Self {
            read_only: future_version.is_some(),
            page: Page::Home,
            saved: cfg.clone(),
            applied_theme: cfg.general.theme,
            cfg,
            config_path,
            dirty_since: None,
            needs_backup,
            engine,
            diagnostics: Diagnostics::default(),
            press_ui: press::PressUi::default(),
            demo_frames: 0,
            win_values: [None; 4],
            win_pan: None,
            win_pan_read_at: None,
            was_focused: true,
            win_read_error: None,
            win_message: None,
            win_page_was_open: false,
            general_state,
            apps: apps::AppsState::default(),
            overview: overview::OverviewState::default(),
            nav_collapsed: false,
            nav_expanded_narrow: false,
            group_open: [true; NAV.len()],
            query: String::new(),
            search_open: false,
            search_index: search::index(),
            devshot: crate::devshot::DevShot::from_env(),
            resident: None,
            pause_notice: Default::default(),
            quitting: false,
            hide_frames: 0,
            icon_deadline: None,
            practice_on: false,
            seen_pause_gen: u64::MAX,
            applied_shortcut: None,
        }
    }

    /// Attach the tray: closing the window then hides it instead of exiting.
    pub fn set_resident(&mut self, resident: Option<Resident>, background: bool) {
        self.hide_frames = if background && resident.is_some() {
            3
        } else {
            0
        };
        self.icon_deadline =
            (background && resident.is_some()).then(|| Instant::now() + Duration::from_secs(35));
        self.resident = resident;
    }

    /// Apply tray requests and turn a window close into hide (unless the tray asked to quit).
    fn resident_tick(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.resident else {
            return;
        };
        let pause = self.engine.pause.view();
        r.set_paused(pause.paused, pause.paused || !self.cfg.gestures_enabled);
        if self.applied_shortcut.as_deref() != Some(self.cfg.general.pause_shortcut.as_str()) {
            r.set_pause_shortcut(&self.cfg.general.pause_shortcut);
            self.applied_shortcut = Some(self.cfg.general.pause_shortcut.clone());
        }
        let generation = self.engine.pause.generation();
        if generation != self.seen_pause_gen {
            if self.seen_pause_gen != u64::MAX {
                // The overlay card is visible about 3 s; the balloon only when it cannot be shown.
                if !self.pause_notice.show(pause.paused) {
                    r.toast(crate::pause_notice::text(pause.paused));
                }
            }
            self.seen_pause_gen = generation;
        }
        // A timed resume and the full-screen check happen in the engine, which does not wake the
        // UI: look again soon so the tray and the toast follow.
        if pause.remaining.is_some() || self.cfg.general.pause_in_fullscreen {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if self.hide_frames > 0 {
            self.hide_frames -= 1;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.request_repaint();
        }
        let events = r.poll();
        if let Some(deadline) = self.icon_deadline {
            if r.icon_ready() {
                self.icon_deadline = None;
            } else if Instant::now() >= deadline {
                // No tray icon after all the retries: do not stay invisible.
                self.icon_deadline = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }
        let close = ctx.input(|i| i.viewport().close_requested());
        if close && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            r.on_hidden();
        }
        let mut ack = false;
        for ev in events {
            match ev {
                resident::Event::TogglePause => self.engine.pause.toggle(),
                resident::Event::PauseForMinutes(m) => {
                    self.engine
                        .pause
                        .pause_for(Duration::from_secs(u64::from(m) * 60));
                }
                resident::Event::EndSession => {
                    // Logoff/shutdown: flush the config and release held keys now.
                    self.save_if_due(ctx, true);
                    self.engine.shutdown();
                    ack = true;
                }
                resident::Event::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if ack && let Some(r) = &self.resident {
            r.ack_end_session();
        }
        // `ui` does not run while the window is hidden, so publish and save here too.
        self.engine.config.publish(&self.cfg);
        self.save_if_due(ctx, false);
    }

    /// Hand the config and window focus to the engine and take its latest state.
    fn sync_engine(&mut self, ctx: &egui::Context) {
        self.engine.config.publish(&self.cfg);
        let focused = ctx.input(|i| i.focused);
        self.engine.focused.store(focused, Ordering::Relaxed);
        self.diagnostics.update(self.engine.snapshot());
    }

    /// The dry-run switch follows the page and the window focus, and goes to the engine.
    fn sync_practice(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        if self.devshot.is_none() && (self.page != Page::Practice || !focused) {
            self.practice_on = false;
        }
        self.engine
            .dry_run
            .store(self.practice_on, Ordering::Relaxed);
    }

    fn backup_path(&self) -> PathBuf {
        self.config_path.with_file_name(win_gestures::BACKUP_FILE)
    }

    fn read_win_pan(&mut self) {
        self.win_pan = win_gestures::pan_enabled();
        self.win_pan_read_at = Some(std::time::Instant::now());
    }

    fn refresh_win_values(&mut self) {
        self.read_win_pan();
        match win_gestures::read_values() {
            Ok(v) => {
                self.win_values = v;
                self.win_read_error = None;
            }
            Err(e) => {
                // The shown values may be stale; say so instead of keeping them silently.
                self.win_read_error = Some(format!(
                    "Windows の設定を読めません（表示は最新ではない可能性があります）: {e}"
                ));
            }
        }
    }

    fn report(&mut self, what: &str, r: Result<(), String>) {
        self.win_message = Some(match r {
            Ok(()) => format!("{what}しました。"),
            Err(e) => e,
        });
        self.refresh_win_values();
    }

    /// Save at most once per `SAVE_DEBOUNCE` while the config has unsaved changes (throttle, so a long
    /// slider drag writes a few times rather than waiting for release).
    fn save_if_due(&mut self, ctx: &egui::Context, force: bool) {
        if self.read_only {
            return;
        }
        if self.cfg == self.saved {
            self.dirty_since = None;
            return;
        }
        let since = *self.dirty_since.get_or_insert_with(Instant::now);
        if force || since.elapsed() >= SAVE_DEBOUNCE {
            match config::backup_before_first_save(&self.config_path, &mut self.needs_backup) {
                Ok(()) => match config::save(&self.config_path, &self.cfg) {
                    Ok(()) => self.saved = self.cfg.clone(),
                    Err(e) => eprintln!("yubi: cannot save config: {e}"),
                },
                Err(note) => {
                    eprintln!("yubi: {note}");
                    if !self.general_state.load_notes.contains(&note) {
                        self.general_state.load_notes.push(note);
                    }
                }
            }
            self.dirty_since = Some(Instant::now());
        }
        ctx.request_repaint_after(SAVE_DEBOUNCE);
    }

    fn run(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Goto(p) => self.page = p,
            Cmd::OpenProfile(i) => {
                self.apps.selected = Some(i);
                self.page = Page::Profiles;
            }
            Cmd::DisableWindowsGestures => {
                let r = win_gestures::disable(&self.backup_path());
                self.report("無効に", r);
            }
            Cmd::RestoreWindowsGestures => {
                let r = win_gestures::restore(&self.backup_path());
                self.report("元に戻", r);
            }
            Cmd::TogglePause => self.engine.pause.toggle(),
            Cmd::OpenTouchpadSettings => {
                let _ = Command::new("explorer")
                    .arg("ms-settings:devices-touchpad")
                    .spawn();
            }
            Cmd::OpenConfigFolder => {
                if let Some(dir) = self.config_path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                    let _ = Command::new("explorer").arg(dir).spawn();
                }
            }
        }
    }

    fn top_bar(&mut self, ui: &mut Ui, t: &Tokens, narrow: bool) {
        let mut goto = None;
        let mut text_rect = egui::Rect::NOTHING;
        ui.horizontal_centered(|ui| {
            if icon_button(ui, t, icons::MENU, "ナビゲーションの切り替え").clicked() {
                if narrow {
                    self.nav_expanded_narrow = !self.nav_expanded_narrow;
                } else {
                    self.nav_collapsed = !self.nav_collapsed;
                }
            }
            ui.label(RichText::new("yubi").font(t.font_semibold(t.fs_section)));
            ui.add_space(t.space_lg);
            let resp = ui.add_sized(
                [t.search_w, t.search_h],
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("設定を検索")
                    .margin(Margin::symmetric(t.space_md as i8, t.space_xs as i8))
                    .align(egui::Align2::LEFT_CENTER),
            );
            text_rect = resp.rect;
            paint_icon(
                ui.painter(),
                Pos2::new(resp.rect.right() - t.space_md, resp.rect.center().y),
                icons::SEARCH,
                t.fs_small,
                t.text_muted,
            );
            if resp.changed() || resp.gained_focus() {
                self.search_open = true;
            }
            if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                self.search_open = false;
                self.query.clear();
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                goto = search::filter(&self.search_index, &self.query)
                    .first()
                    .map(|e| e.page);
            }
        });
        if let Some(p) = goto {
            self.finish_search(p);
            return;
        }
        if self.search_open && !self.query.trim().is_empty() {
            let pos = text_rect.left_bottom() + egui::vec2(0.0, t.space_xs);
            let ctx = ui.ctx().clone();
            let results: Vec<(Page, &str, &str)> = search::filter(&self.search_index, &self.query)
                .into_iter()
                .take(t.search_results_max)
                .map(|e| (e.page, e.title, e.desc))
                .collect();
            let area = Area::new(Id::unique("search_results"))
                .order(Order::Foreground)
                .fixed_pos(pos)
                .show(&ctx, |ui| {
                    Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(t.search_w);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if results.is_empty() {
                            ui.label(RichText::new("一致する項目がありません").color(t.text_muted));
                        }
                        for (i, (page, title, desc)) in results.iter().enumerate() {
                            let bg = ui.painter().add(Shape::Noop);
                            let inner = ui
                                .scope(|ui| {
                                    ui.set_width(ui.available_width());
                                    ui.add_space(t.space_xs);
                                    ui.label(RichText::new(*title).size(t.fs_body));
                                    ui.label(
                                        RichText::new(format!("{} · {desc}", page.label()))
                                            .size(t.fs_small)
                                            .color(t.text_muted),
                                    );
                                    ui.add_space(t.space_xs);
                                })
                                .response;
                            let r = ui.interact(
                                inner.rect,
                                Id::unique(("search_row", i)),
                                Sense::click(),
                            );
                            if r.hovered() {
                                ui.painter().set(
                                    bg,
                                    Shape::rect_filled(inner.rect, t.radius_md, t.card_hover),
                                );
                            }
                            if r.clicked() {
                                goto = Some(*page);
                            }
                        }
                    });
                });
            if let Some(p) = goto {
                self.finish_search(p);
                return;
            }
            let clicked_outside = ui.input(|i| i.pointer.any_pressed())
                && ui
                    .input(|i| i.pointer.interact_pos())
                    .is_some_and(|p| !text_rect.contains(p) && !area.response.rect.contains(p));
            if clicked_outside {
                self.search_open = false;
            }
        }
    }

    fn finish_search(&mut self, page: Page) {
        self.page = page;
        self.search_open = false;
        self.query.clear();
    }

    fn nav(&mut self, ui: &mut Ui, t: &Tokens, collapsed: bool) {
        let connected = self.diagnostics.connected();
        let mut goto = None;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = t.nav_gap;
                device_status(ui, t, collapsed, connected);
                ui.add_space(t.space_xs);
                for (gi, node) in NAV.iter().enumerate() {
                    let mut item = |ui: &mut Ui, page: Page| {
                        let r = nav_item(
                            ui,
                            t,
                            collapsed,
                            self.page == page,
                            page.icon(),
                            page.label(),
                        );
                        if r.clicked() {
                            goto = Some(page);
                        }
                    };
                    match node {
                        Node::Item(p) => item(ui, *p),
                        Node::Group { label, pages } => {
                            if collapsed {
                                for p in *pages {
                                    item(ui, *p);
                                }
                            } else {
                                if nav_group_header(ui, t, label, self.group_open[gi]).clicked() {
                                    self.group_open[gi] = !self.group_open[gi];
                                }
                                if self.group_open[gi] {
                                    for p in *pages {
                                        item(ui, *p);
                                    }
                                }
                            }
                        }
                    }
                }
            });
        if let Some(p) = goto {
            self.page = p;
        }
    }

    fn content(&mut self, ui: &mut Ui, t: &Tokens, now: f64) -> Option<Cmd> {
        let shortcut = if self.devshot.is_some() {
            resident::ShortcutStatus::Active
        } else {
            self.resident
                .as_ref()
                .map_or(resident::ShortcutStatus::Off, Resident::shortcut_status)
        };
        let cfg = &mut self.cfg;
        match self.page {
            Page::Home => return home::home(ui, t, cfg, self.diagnostics.connected()),
            Page::Drag => return features::drag(ui, t, cfg, &self.win_values),
            Page::Four => return features::four(ui, t, cfg, &self.win_values),
            Page::Five => features::five(ui, t, cfg),
            Page::CustomPress => {
                let key = self
                    .diagnostics
                    .state()
                    .device
                    .as_ref()
                    .and_then(|d| crate::calibration::device_key(&d.id));
                return features::custom_press(ui, t, cfg, key.as_deref());
            }
            Page::LongHold => features::long_hold(ui, t, cfg),
            Page::FingerHold => features::hold_combos(ui, t, cfg, Page::FingerHold),
            Page::ThumbHold => features::hold_combos(ui, t, cfg, Page::ThumbHold),
            Page::Corner => features::corner(ui, t, cfg),
            Page::Edge => features::edge(ui, t, cfg),
            Page::Window => return features::window(ui, t, cfg, &self.win_values),
            Page::Ring => features::ring(ui, t, cfg),
            Page::Profiles => return apps::page(ui, t, cfg, &mut self.apps),
            Page::Overview => {
                return overview::page(ui, t, cfg, &self.win_values, &mut self.overview);
            }
            Page::Sensitivity => return sensitivity::sensitivity(ui, t, cfg, self.win_pan),
            Page::Haptic => press::page(ui, t, cfg, self.diagnostics.state(), &mut self.press_ui),
            Page::Monitor => self.diagnostics.monitor(ui, t),
            Page::Practice => self.diagnostics.practice(ui, t, &mut self.practice_on),
            Page::Diagnostics => self.diagnostics.info(ui, t, now),
            Page::General => {
                let view = general::WinGestureView {
                    values: &self.win_values,
                    backup_exists: self
                        .config_path
                        .with_file_name(win_gestures::BACKUP_FILE)
                        .exists(),
                    message: self
                        .win_read_error
                        .as_deref()
                        .or(self.win_message.as_deref()),
                };
                return general::general(
                    ui,
                    t,
                    cfg,
                    &self.config_path,
                    &view,
                    &mut self.general_state,
                    general::PauseInfo {
                        shortcut,
                        pause: self.engine.pause.view(),
                    },
                );
            }
        }
        None
    }
}

impl eframe::App for App {
    /// Runs every frame, and also while the window is hidden (when `ui` does not).
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.resident_tick(ctx);
        self.sync_practice(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        if let Some(shot) = self.devshot.as_ref().and_then(|d| d.current()) {
            // Keep `saved` in step so the capture run never writes the config file.
            let (fake, practice) = (shot.fake, shot.practice);
            self.page = shot.page;
            shot.fixture.apply(&mut self.cfg);
            if shot.page == Page::Profiles {
                self.apps.selected = shot.profile;
            }
            self.practice_on = practice;
            self.diagnostics
                .force(fake.then(crate::pages::practice::fake_state));
            self.cfg.general.theme = shot.theme;
            self.saved.general.theme = shot.theme;
        }
        if self.cfg.general.theme != self.applied_theme {
            self.applied_theme = self.cfg.general.theme;
            theme::apply(ui.ctx(), self.applied_theme);
        }
        let t = Tokens::of(ui.ctx());
        let now = ui.input(|i| i.time);
        let ctx_now = ui.ctx().clone();
        self.sync_engine(&ctx_now);
        if self.devshot.is_some()
            && let Ok(mode) = std::env::var("YUBI_SHOT_PRESS")
        {
            self.demo_frames += 1;
            let mon = press::demo(&mode, &mut self.cfg, &mut self.press_ui, self.demo_frames);
            self.diagnostics.update(mon);
        }
        // Pages that show the Windows-gesture state (or warn about it) re-read it when opened.
        let general_open = matches!(
            self.page,
            Page::General | Page::Drag | Page::Window | Page::Overview | Page::Sensitivity
        );
        if general_open && !self.win_page_was_open {
            self.refresh_win_values();
        }
        self.win_page_was_open = general_open;
        // The user may change the Windows setting the 感度 page warns about and come back: read
        // it again when the window regains focus (a cheap registry read, at most once a second).
        let focused = ui.input(|i| i.focused);
        if focused
            && !self.was_focused
            && self.page == Page::Sensitivity
            && self
                .win_pan_read_at
                .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1))
        {
            self.read_win_pan();
        }
        self.was_focused = focused;
        self.general_state.set_visible(self.page == Page::General);
        let narrow = ui.ctx().content_rect().width() < t.nav_auto_collapse_w;
        let collapsed = if narrow {
            !self.nav_expanded_narrow
        } else {
            self.nav_collapsed
        };

        Panel::top("topbar")
            .exact_size(t.topbar_h)
            .frame(
                Frame::new()
                    .fill(t.bg)
                    .inner_margin(Margin::symmetric(t.space_md as i8, t.space_xs as i8)),
            )
            .show(ui, |ui| self.top_bar(ui, &t, narrow));

        Panel::left("nav")
            .exact_size(if collapsed {
                t.nav_rail_width
            } else {
                t.nav_width
            })
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(t.nav)
                    .stroke(Stroke::new(t.stroke_thin, t.separator))
                    .inner_margin(Margin::same(t.nav_pad as i8)),
            )
            .show(ui, |ui| self.nav(ui, &t, collapsed));

        let mut cmd = None;
        CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(t.bg)
                    .inner_margin(Margin::symmetric(t.space_lg as i8, 0)),
            )
            .show(ui, |ui| {
                let mut area = ScrollArea::vertical().auto_shrink([false, false]);
                if let Some(y) = general_io::shot_scroll() {
                    area = area.vertical_scroll_offset(y);
                }
                area.show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(t.content_max_w));
                    cmd = self.content(ui, &t, now);
                    ui.add_space(t.space_xl);
                });
            });
        if let Some(c) = cmd {
            self.run(c);
        }

        let ctx = ui.ctx().clone();
        if let Some(d) = self.devshot.as_mut() {
            d.tick(&ctx);
            return;
        }
        self.save_if_due(&ctx, false);
    }

    fn on_exit(&mut self) {
        self.engine.shutdown();
        if let Some(r) = self.resident.as_mut() {
            r.shutdown();
        }
        if self.devshot.is_none()
            && !self.read_only
            && self.cfg != self.saved
            && config::backup_before_first_save(&self.config_path, &mut self.needs_backup).is_ok()
        {
            let _ = config::save(&self.config_path, &self.cfg);
        }
    }
}
