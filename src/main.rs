#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod devshot;
mod engine;
mod fonts;
mod haptic;
mod icons;
mod input;
mod nav;
mod os;
mod pages;
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
    let record = record_path();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("yubi")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "yubi",
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            let cfg = config::load(&config::default_path());
            let engine = engine::Engine::start(cfg, move || ctx.request_repaint(), record);
            Ok(Box::new(app::App::new(&cc.egui_ctx, engine)))
        }),
    )
}
