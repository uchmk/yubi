#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod fonts;
mod input;
mod pages;
mod theme;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("yubi")
            .with_inner_size([880.0, 600.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "yubi",
        options,
        Box::new(|cc| {
            fonts::install(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            let rx = input::spawn(move || ctx.request_repaint());
            Ok(Box::new(app::App::new(rx)))
        }),
    )
}
