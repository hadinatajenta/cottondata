use super::CottonDataApp;
use crate::theme::Palette;
use crate::ui_kit::{banner, caption, icon, primary_button, secondary_button};
use cottondata_core::SERVER_SLOT_CAPACITY;
use eframe::egui;

pub(super) struct SettingsModalState {
    max_servers: usize,
    error_msg: Option<String>,
}

impl SettingsModalState {
    pub(super) fn new(max_servers: usize) -> Self {
        Self {
            max_servers,
            error_msg: None,
        }
    }
}

impl CottonDataApp {
    pub(super) fn open_settings(&mut self) {
        self.settings_modal = Some(SettingsModalState::new(self.registry.max_servers()));
    }

    pub(super) fn render_settings_modal(&mut self, ctx: &egui::Context, palette: Palette) {
        let Some(mut state) = self.settings_modal.take() else {
            return;
        };
        let mut is_open = true;
        let mut is_save_clicked = false;
        let mut is_cancel_clicked = false;
        let minimum_servers = self.registry.count().max(1);

        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .open(&mut is_open)
            .show(ctx, |ui| {
                if let Some(message) = &state.error_msg {
                    banner(ui, palette, palette.danger, icon::WARNING, message, false);
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    ui.label("Maximum servers");
                    ui.add(
                        egui::DragValue::new(&mut state.max_servers)
                            .range(minimum_servers..=SERVER_SLOT_CAPACITY)
                            .speed(0.2),
                    );
                });
                caption(
                    ui,
                    palette,
                    format!(
                        "Between {minimum_servers} and {SERVER_SLOT_CAPACITY}. Saved servers cost almost no memory; \
                         only connected servers use RAM. Cmd+1..0 switches the first ten."
                    ),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    is_save_clicked = primary_button(ui, palette, icon::FLOPPY_DISK, "Save", true).clicked();
                    is_cancel_clicked = secondary_button(ui, palette, icon::X, "Cancel", true).clicked();
                });
            });

        if is_save_clicked {
            match self.registry.set_max_servers(state.max_servers) {
                Ok(()) => match self.storage.save_registry(&self.registry) {
                    Ok(()) => return,
                    Err(err) => state.error_msg = Some(format!("Could not save settings: {err}")),
                },
                Err(err) => state.error_msg = Some(err.to_string()),
            }
        }
        if is_open && !is_cancel_clicked {
            self.settings_modal = Some(state);
        }
    }
}
