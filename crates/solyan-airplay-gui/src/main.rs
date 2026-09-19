#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod theme;
mod worker;

use app::SolYanAirPlayApp;
use eframe::egui;

const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/solyan-airplay-logo-64.png");

fn load_app_icon() -> egui::IconData {
    match image::load_from_memory(APP_ICON_PNG) {
        Ok(image) => {
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        }
        Err(_) => egui::IconData::default(),
    }
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
