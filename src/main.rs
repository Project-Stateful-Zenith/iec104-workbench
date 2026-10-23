#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod commands;
mod driver;
mod files;
mod link;
mod points;
mod script;
mod settings;
mod simulator;
mod telegram;
mod update;

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => gui(),
        Some("run") => script::main(args),
        Some(_) => {
            eprintln!("usage: iec104-workbench [run <script.rhai> ...]");
            ExitCode::from(2)
        }
    }
}

fn gui() -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("IEC 104 Workbench")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    match eframe::run_native(
        "iec104-workbench",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
