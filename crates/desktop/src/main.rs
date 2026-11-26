#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod column_glyph;
mod drivers;
mod theme;
mod ui_kit;
mod updater;
mod worker;

use eframe::egui;
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

extern "C" {
    fn mi_collect(force: bool);
}

pub(crate) fn purge_unused_memory() {
    // SAFETY: `mi_collect` is an exported, thread-safe mimalloc C API function
    // linked via the global `MiMalloc` allocator.
    unsafe {
        mi_collect(true);
    }
}

const WINDOW_ICON_PNG: &[u8] = include_bytes!("../../../assets/AppIcon-256.png");

#[cfg(target_os = "macos")]
fn is_macos_app_bundle_exe(exe: &std::path::Path) -> bool {
    let Some(macos_dir) = exe.parent() else {
        return false;
    };
    let Some(contents_dir) = macos_dir.parent() else {
        return false;
    };
    let Some(bundle_dir) = contents_dir.parent() else {
        return false;
    };
    macos_dir.file_name().is_some_and(|name| name == "MacOS")
        && contents_dir
            .file_name()
            .is_some_and(|name| name == "Contents")
        && bundle_dir.extension().is_some_and(|ext| ext == "app")
        && contents_dir.join("Resources").join("AppIcon.icns").is_file()
}

fn load_window_icon() -> Option<egui::IconData> {
    #[cfg(target_os = "macos")]
    if std::env::current_exe()
        .ok()
        .is_some_and(|exe| is_macos_app_bundle_exe(&exe))
    {
        return Some(egui::IconData::default());
    }

    eframe::icon_data::from_png_bytes(WINDOW_ICON_PNG).ok()
}

fn main() -> eframe::Result<()> {
    updater::cleanup_stale_old_binaries();
    drivers::install_tls_crypto_provider();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("CottonData")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([920.0, 600.0]);

    if cfg!(target_os = "macos") {
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false);
    }

    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(icon);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_window_icon_is_high_resolution() {
        let icon = eframe::icon_data::from_png_bytes(WINDOW_ICON_PNG)
            .expect("embedded app icon PNG should decode");
        assert!(icon.width >= 256, "expected width >= 256, got {}", icon.width);
        assert!(icon.height >= 256, "expected height >= 256, got {}", icon.height);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_bundle_detection_requires_app_structure_and_icns() {
        let base = std::env::temp_dir().join(format!(
            "cottondata-icon-test-{}",
            std::process::id()
        ));
        let macos_dir = base.join("CottonData.app/Contents/MacOS");
        let res_dir = base.join("CottonData.app/Contents/Resources");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&macos_dir).unwrap();
        std::fs::create_dir_all(&res_dir).unwrap();

        let exe = macos_dir.join("CottonData");
        assert!(!is_macos_app_bundle_exe(&exe));

        std::fs::write(res_dir.join("AppIcon.icns"), b"icns").unwrap();
        assert!(is_macos_app_bundle_exe(&exe));
        assert!(!is_macos_app_bundle_exe(std::path::Path::new(
            "/tmp/target/release/CottonData"
        )));

        let _ = std::fs::remove_dir_all(&base);
    }
}
