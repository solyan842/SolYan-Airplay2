#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod theme;
mod worker;

use app::SolYanAirPlayApp;
use base64::Engine as _;
use eframe::egui;
use std::sync::Arc;

fn embedded_logo_png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(include_str!("../assets/solyan-airplay-logo.b64").trim())
        .expect("embedded SolYan AirPlay logo must decode")
}

fn main() -> eframe::Result {
    let logo_png = embedded_logo_png();
    let icon = eframe::icon_data::from_png_bytes(&logo_png)
        .ok()
        .map(Arc::new);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("SolYan AirPlay2")
        .with_app_id("com.solyan.airplay2")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([1040.0, 700.0]);

    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "SolYan AirPlay2",
        native_options,
        Box::new(|cc| Ok(Box::new(SolYanAirPlayApp::new(cc)))),
    )
}
