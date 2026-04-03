#![cfg_attr(windows, windows_subsystem = "windows")]

mod adb;
mod config;
mod device;
mod ui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("ADB UI"),
        ..Default::default()
    };

    eframe::run_native(
        "ADB UI",
        options,
        Box::new(|cc| Ok(Box::new(ui::App::new(cc)))),
    )
}
