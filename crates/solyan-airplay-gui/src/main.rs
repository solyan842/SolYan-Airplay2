#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod theme;
mod worker;

use app::SolYanAirPlayApp;
use base64::Engine as _;
use eframe::egui;

fn load_app_icon() -> egui::IconData {
    let logo_png = base64::engine::general_purpose::STANDARD
        .decode(concat!(
            include_str!("../assets/solyan-airplay-logo.0.b64"),
            include_str!("../assets/solyan-airplay-logo.1.b64"),
            include_str!("../assets/solyan-airplay-logo.2.b64"),
            include_str!("../assets/solyan-airplay-logo.3.b64")
        ))
        .expect("embedded SolYan AirPlay logo must decode");

    eframe::icon_data::from_png_bytes(&logo_png)
        .expect("embedded SolYan AirPlay logo must be a valid PNG")
}

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("SolYan AirPlay2")
            .with_app_id("com.solyan.airplay2")
            .with_icon(load_app_icon())
            .with_inner_size([1360.0, 820.0])
            .with_min_inner_size([1080.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "SolYan AirPlay2",
        native_options,
        Box::new(|cc| Ok(Box::new(SolYanAirPlayApp::new(cc)))),
    )
}
