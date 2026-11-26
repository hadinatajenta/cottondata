use super::CottonDataApp;
use crate::theme::{AppTheme, Palette};
use crate::ui_kit::{
    banner, caption, icon, inset_frame, modal_shell, option_card, primary_button, secondary_button, section_heading,
    ModalConfig,
};
use cottondata_core::SERVER_SLOT_CAPACITY;
use eframe::egui::{self, RichText};

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
        let mut is_save_clicked = false;
        let mut is_cancel_clicked = false;
        let mut switched_theme: Option<AppTheme> = None;
        let minimum_servers = self.registry.count().max(1);
        let current_theme = self.theme;

        let outcome = modal_shell(
            ctx,
            palette,
            ModalConfig {
                id: "settings_modal",
                icon_glyph: icon::GEAR_SIX,
                icon_color: palette.accent,
                title: "Settings",
                subtitle: Some("Appearance, server limits, and workspace persistence"),
                accent_bar: None,
                width: 480.0,
                max_height: None,
                dismiss_on_backdrop: true,
            },
            |ui| {
                let full_w = ui.available_width();
                if let Some(message) = &state.error_msg {
                    banner(ui, palette, palette.danger, icon::WARNING, message, false);
                    ui.add_space(10.0);
                }

                section_heading(ui, palette, "Appearance");
                ui.add_space(6.0);
                let gap = 10.0;
                let card_w = (full_w - gap) / 2.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    if option_card(
                        ui,
                        palette,
                        icon::SUN,
                        "Light Mode",
                        "Crisp daylight canvas",
                        current_theme == AppTheme::Light,
                        card_w,
                    )
                    .clicked()
                    {
                        switched_theme = Some(AppTheme::Light);
                    }
                    if option_card(
                        ui,
                        palette,
                        icon::MOON,
                        "Dark Mode",
                        "Low-glare night palette",
                        current_theme == AppTheme::Dark,
                        card_w,
                    )
                    .clicked()
                    {
                        switched_theme = Some(AppTheme::Dark);
                    }
                });

                ui.add_space(14.0);
                section_heading(ui, palette, "Server Slots");
                ui.add_space(6.0);
                inset_frame(palette).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Maximum saved servers")
                                .size(13.0)
                                .strong()
                                .color(palette.text_primary),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if secondary_button(
                                ui,
                                palette,
                                icon::PLUS,
                                "",
                                state.max_servers < SERVER_SLOT_CAPACITY,
                            )
                            .clicked()
                            {
                                state.max_servers = (state.max_servers + 1).min(SERVER_SLOT_CAPACITY);
                            }
                            ui.add(
                                egui::DragValue::new(&mut state.max_servers)
                                    .range(minimum_servers..=SERVER_SLOT_CAPACITY)
                                    .speed(0.2),
                            );
                            if secondary_button(ui, palette, icon::MINUS, "", state.max_servers > minimum_servers)
                                .clicked()
                            {
                                state.max_servers = state.max_servers.saturating_sub(1).max(minimum_servers);
                            }
                        });
                    });
                    ui.add_space(4.0);
                    caption(
                        ui,
                        palette,
                        format!(
                            "Between {minimum_servers} and {SERVER_SLOT_CAPACITY}. Only connected servers consume RAM; \
                             Cmd+1..0 switches the first ten."
                        ),
                    );
                });

                ui.add_space(12.0);
                section_heading(ui, palette, "Workspace Auto-Save");
                ui.add_space(6.0);
                inset_frame(palette).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    caption(
                        ui,
                        palette,
                        "Open consoles, SQL drafts, editor layout, and active connections auto-save within 400 ms and on quit. Press Cmd+Shift+T to reopen closed tabs.",
                    );
                });
            },
            |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    is_save_clicked = primary_button(ui, palette, icon::FLOPPY_DISK, "Save Settings", true).clicked();
                    is_cancel_clicked = secondary_button(ui, palette, icon::X, "Cancel", true).clicked();
                });
            },
        );

        if let Some(new_theme) = switched_theme {
            if new_theme != self.theme {
                self.theme = new_theme;
                self.theme.apply(ctx);
                self.persist_sessions_now();
            }
        }

        if is_save_clicked {
            match self.registry.set_max_servers(state.max_servers) {
                Ok(()) => match self.storage.save_registry(&self.registry) {
                    Ok(()) => {
                        self.notify_success("Settings saved.");
                        return;
                    }
                    Err(err) => state.error_msg = Some(format!("Could not save settings: {err}")),
                },
                Err(err) => state.error_msg = Some(err.to_string()),
            }
        }
        if !outcome.closed && !is_cancel_clicked {
            self.settings_modal = Some(state);
        }
    }
}
