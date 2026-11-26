#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod column_glyph;
mod drivers;
mod theme;
mod ui_kit;
mod worker;

use eframe::egui;
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

const ICON_64_RGBA: &[u8] = include_bytes!("../../../assets/AppIcon-64.rgba");

fn main() -> eframe::Result<()> {
    drivers::install_tls_crypto_provider();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("CottonData")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([920.0, 600.0]);

    if ICON_64_RGBA.len() == 64 * 64 * 4 {
        viewport = viewport.with_icon(egui::IconData {
            rgba: ICON_64_RGBA.to_vec(),
            width: 64,
            height: 64,
        });
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "CottonData",
        options,
        Box::new(|cc| Ok(Box::new(app::CottonDataApp::new(cc)))),
    )
}
