#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod config_io;
mod devshot;
mod engine;
mod fonts;
mod haptic;
mod hook;
mod icons;
mod input;
mod nav;
mod os;
mod overlay;
mod pages;
mod resident;
mod search;
mod theme;
mod widgets;
mod win_gestures;

use eframe::egui;

/// `--record <file>`: write every frame as a JSON line for `engine::replay`.
fn record_path() -> Option<std::path::PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        if a == "--record" {
            return args.next().map(Into::into);
        }
    }
    None
}

fn main() -> eframe::Result {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let record = record_path();
    // Recording and screenshot runs are throwaway: no tray, no single-instance check, no autostart.
    let plain = record.is_some() || std::env::var_os("YUBI_SHOT").is_some();
    let background = !plain && resident::is_background(&args);
    if !plain && !resident::enter(background) {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("yubi")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([640.0, 420.0])
            .with_visible(!background),
        ..Default::default()
    };
    eframe::run_native(
        "yubi",
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            let config_path = config::default_path();
            let loaded = config::load_report(&config_path);
            let cfg = loaded.config.clone();
            let tray = if plain {
                None
            } else {
                resident::autostart::sync(cfg.general.start_with_windows);
                resident::Resident::start(cc.egui_ctx.clone(), config_path.parent().map(Into::into))
            };
            if background && tray.is_none() {
                // No tray icon means no way back to a hidden window.
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::Visible(true));
            }
            let engine = engine::Engine::start(cfg, move || ctx.request_repaint(), record);
            let mut app = app::App::new(&cc.egui_ctx, engine, loaded);
            app.set_resident(tray, background);
            Ok(Box::new(app))
        }),
    )
}
