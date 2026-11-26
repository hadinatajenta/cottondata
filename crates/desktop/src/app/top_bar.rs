use super::server_form::ServerModalState;
use super::sessions::SessionModalState;
use super::CottonDataApp;
use crate::theme::{AppTheme, Palette, GUTTER};
use crate::ui_kit::{card_frame, ghost_button, icon, icon_button, icon_text, pill, primary_button, toolbar_divider};
use crate::worker::SlotCmd;
use cottondata_core::{ServerSlotId, MAX_SERVERS};
use eframe::egui::{self, Color32, Margin, RichText};

const LABELED_TOOLBAR_MIN_WIDTH: f32 = 1180.0;
const NO_SCHEMA_LABEL: &str = "(no schema)";

impl CottonDataApp {
    pub(super) fn render_top_bar(&mut self, ctx: &egui::Context, palette: Palette) {
        egui::TopBottomPanel::top("toolbar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(palette.bg_backdrop).inner_margin(Margin {
                left: GUTTER,
                right: GUTTER,
                top: GUTTER,
                bottom: 0,
            }))
            .show(ctx, |ui| {
                card_frame(palette)
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let show_labels = ui.available_width() > LABELED_TOOLBAR_MIN_WIDTH;
                            self.render_brand_group(ui, palette, show_labels);
                            toolbar_divider(ui, palette);
                            self.render_transaction_group(ui, palette, show_labels);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                self.render_connection_group(ui, palette);
                            });
                        });
                    });
            });
    }

    fn render_brand_group(&mut self, ui: &mut egui::Ui, palette: Palette, show_labels: bool) {
        ui.label(icon_text(
            icon::DATABASE,
            palette.accent,
            "CottonData",
            palette.text_primary,
            14.0,
        ));
        toolbar_divider(ui, palette);
        self.render_session_picker(ui, palette);
        toolbar_divider(ui, palette);
        let console_label = if show_labels { "Console" } else { "" };
        if ghost_button(ui, palette, icon::TERMINAL_WINDOW, console_label, true)
            .on_hover_text("Open a new SQL console")
            .clicked()
        {
            self.open_console_tab();
        }
    }

    fn render_session_picker(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let active_id = self.sessions_store.active_session_id.clone();
        let active_name = self.sessions_store.active_session().name.clone();
        let options: Vec<(String, String)> = self
            .sessions_store
            .sessions
            .iter()
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();

        let mut chosen_id = active_id.clone();
        let mut open_manager = false;
        egui::ComboBox::from_id_salt("active_session_combo")
            .selected_text(icon_text(
                icon::STACK,
                palette.accent,
                &active_name,
                palette.text_primary,
                13.0,
            ))
            .width(126.0)
            .show_ui(ui, |ui| {
                for (sid, sname) in &options {
                    ui.selectable_value(&mut chosen_id, sid.clone(), sname);
                }
                ui.separator();
                if ui.selectable_label(false, "Manage sessions…").clicked() {
                    open_manager = true;
                }
            });
        if icon_button(ui, palette, icon::FLOPPY_DISK, "Manage workspace sessions").clicked() {
            open_manager = true;
        }
        if chosen_id != active_id {
            self.switch_session(&chosen_id);
        }
        if open_manager {
            self.session_modal = Some(SessionModalState::new());
        }
    }

    fn render_connection_group(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let add_label = format!("Server {}/{}", self.registry.count(), MAX_SERVERS);
        if primary_button(ui, palette, icon::PLUS, &add_label, !self.registry.is_full())
            .on_hover_text("Add a MySQL server")
            .clicked()
        {
            self.modal = Some(ServerModalState::new_empty());
        }

        let theme_glyph = match self.theme {
            AppTheme::Light => icon::MOON,
            AppTheme::Dark => icon::SUN,
        };
        if icon_button(ui, palette, theme_glyph, "Switch theme").clicked() {
            self.toggle_theme(ui.ctx());
        }
        toolbar_divider(ui, palette);

        self.render_schema_picker(ui, palette);
        self.render_server_picker(ui, palette);
        self.render_server_badges(ui, palette);
    }

    fn render_server_picker(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let selected_label = self
            .active_slot
            .and_then(|slot_id| self.registry.get(slot_id))
            .map(|slot| format!("{} {}", slot.slot_id, slot.profile.name))
            .unwrap_or_else(|| "No server".into());
        let server_options: Vec<(ServerSlotId, String)> = self
            .registry
            .iter()
            .map(|slot| (slot.slot_id, format!("{} {}", slot.slot_id, slot.profile.name)))
            .collect();

        let mut chosen_slot = self.active_slot;
        egui::ComboBox::from_id_salt("active_server_combo")
            .selected_text(icon_text(
                icon::DATABASE,
                palette.icon_schema,
                &selected_label,
                palette.text_primary,
                13.0,
            ))
            .width(170.0)
            .show_ui(ui, |ui| {
                for (slot_id, label) in &server_options {
                    ui.selectable_value(&mut chosen_slot, Some(*slot_id), label);
                }
            });
        if chosen_slot != self.active_slot {
            self.active_slot = chosen_slot;
            self.mark_session_dirty();
        }
    }

    fn render_schema_picker(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let Some(slot_id) = self.active_slot else {
            return;
        };
        let Some(current_schema) = self
            .registry
            .get(slot_id)
            .and_then(|slot| slot.runtime.as_ref())
            .map(|runtime| runtime.current_schema.clone().unwrap_or_else(|| NO_SCHEMA_LABEL.into()))
        else {
            return;
        };
        let schema_names: Vec<String> = self.catalogs[slot_id.index()]
            .as_ref()
            .map(|catalog| {
                catalog
                    .schemas()
                    .iter()
                    .filter(|schema| !schema.is_system || self.show_system_schemas)
                    .map(|schema| catalog.resolve(schema.name).to_string())
                    .collect()
            })
            .unwrap_or_default();

        let mut chosen_schema = current_schema.clone();
        let is_command_pending = self.slot_activity[slot_id.index()].is_session_command_pending;
        ui.add_enabled_ui(!is_command_pending, |ui| {
            egui::ComboBox::from_id_salt("active_schema_combo")
                .selected_text(icon_text(
                    icon::STACK,
                    palette.icon_table,
                    &current_schema,
                    palette.text_primary,
                    13.0,
                ))
                .width(150.0)
                .show_ui(ui, |ui| {
                    for schema_name in &schema_names {
                        ui.selectable_value(&mut chosen_schema, schema_name.clone(), schema_name);
                    }
                })
        });
        if chosen_schema != current_schema && chosen_schema != NO_SCHEMA_LABEL {
            self.send_session_command(slot_id, SlotCmd::SetSchema(chosen_schema));
        }
    }

    fn render_server_badges(&self, ui: &mut egui::Ui, palette: Palette) {
        let Some(slot) = self.active_slot.and_then(|slot_id| self.registry.get(slot_id)) else {
            return;
        };
        if slot.profile.read_only {
            pill(ui, "READ-ONLY", palette.warning);
        }
        let [red, green, blue] = slot.profile.environment.rgb();
        pill(
            ui,
            slot.profile.environment.label(),
            Color32::from_rgb(red, green, blue),
        );
        if slot.runtime.is_none() {
            ui.label(RichText::new("offline").size(11.5).color(palette.text_muted));
        }
    }
}
