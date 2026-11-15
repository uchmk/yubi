#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("yubi")
            .with_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native("yubi", options, Box::new(|_cc| Ok(Box::new(App))))
}

struct App;

impl eframe::App for App {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("yubi");
            ui.label("Magic Trackpad gesture engine (skeleton)");
        });
    }
}
