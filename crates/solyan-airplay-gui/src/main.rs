#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod theme;
mod worker;

use app::SolYanAirPlayApp;
use eframe::egui;

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("SolYan AirPlay2")
            .with_app_id("com.solyan.airplay2")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([980.0, 660.0]),
        ..Default::default()
    };

    eframe::run_native(
        "SolYan AirPlay2",
        native_options,
        Box::new(|cc| Ok(Box::new(SolYanAirPlayApp::new(cc)))),
    )
}
