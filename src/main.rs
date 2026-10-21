#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod commands;
mod driver;
mod files;
mod link;
mod points;
mod settings;
mod simulator;
mod telegram;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("IEC 104 Workbench")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "iec104-workbench",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
