use super::server_form::ServerModalState;
use super::CottonDataApp;
use crate::theme::{Palette, GUTTER, RADIUS_CONTROL};
use crate::ui_kit::{
    caption, card_frame, icon, icon_button, icon_text, pill, secondary_button, section_title, status_dot,
};
use cottondata_core::{ConnectionStatus, ServerSlot, ServerSlotId, MAX_SERVERS};
use eframe::egui::{self, Color32, CornerRadius, Margin, Sense};

const SERVER_LIST_MAX_HEIGHT: f32 = 220.0;
const SERVER_ROW_BADGE_WIDTH: f32 = 64.0;

enum ServerAction {
    Select(ServerSlotId),
    Connect(ServerSlotId),
    Disconnect(ServerSlotId),
    Edit(ServerSlotId),
    Remove(ServerSlotId),
}

impl CottonDataApp {
    pub(super) fn render_sidebar(&mut self, ctx: &egui::Context, palette: Palette) {
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .show_separator_line(false)
            .default_width(290.0)
            .min_width(240.0)
            .max_width(460.0)
            .frame(egui::Frame::new().fill(palette.bg_backdrop).inner_margin(Margin {
                left: GUTTER,
                right: 0,
                top: GUTTER,
                bottom: GUTTER,
            }))
            .show(ctx, |ui| {
                card_frame(palette).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    self.render_servers_card(ui, palette);
                });
                ui.add_space(f32::from(GUTTER));
                card_frame(palette).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    self.render_navigator_card(ui, palette);
                });
            });
    }

    fn render_servers_card(&mut self, ui: &mut egui::Ui, palette: Palette) {
        ui.horizontal(|ui| {
            section_title(ui, palette, icon::PLUGS, "Servers");
            caption(ui, palette, format!("{}/{}", self.registry.count(), MAX_SERVERS));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_add = !self.registry.is_full();
                let add_clicked = ui
                    .add_enabled_ui(can_add, |ui| icon_button(ui, palette, icon::PLUS, "Add a MySQL server"))
                    .inner
                    .clicked();
                if add_clicked {
                    self.modal = Some(ServerModalState::new_empty());
                }
                caption(ui, palette, "Cmd+1..0");
            });
        });
        ui.add_space(4.0);

        if self.registry.count() == 0 {
            caption(ui, palette, "No MySQL servers yet. Add up to 10 with no JVM overhead.");
            ui.add_space(4.0);
            if secondary_button(ui, palette, icon::PLUS, "Add MySQL server", true).clicked() {
                self.modal = Some(ServerModalState::new_empty());
            }
            return;
        }

        let mut pending_action = None;
        egui::ScrollArea::vertical()
            .id_salt("server_list_scroll")
            .max_height(SERVER_LIST_MAX_HEIGHT)
            .show(ui, |ui| {
                for slot in self.registry.iter() {
                    let is_selected = self.active_slot == Some(slot.slot_id);
                    if let Some(action) = render_server_row(ui, palette, slot, is_selected) {
                        pending_action = Some(action);
                    }
                }
            });

        if let Some(action) = pending_action {
            self.apply_server_action(action);
        }
    }

    fn apply_server_action(&mut self, action: ServerAction) {
        match action {
            ServerAction::Select(slot_id) => {
                self.active_slot = Some(slot_id);
                self.mark_session_dirty();
            }
            ServerAction::Connect(slot_id) => self.connect_slot(slot_id),
            ServerAction::Disconnect(slot_id) => self.disconnect_slot(slot_id),
            ServerAction::Edit(slot_id) => {
                if let Some(slot) = self.registry.get(slot_id) {
                    let password = self.storage.get_password(&slot.profile.id);
                    self.modal = Some(ServerModalState::from_existing(slot_id, &slot.profile, password));
                }
            }
            ServerAction::Remove(slot_id) => self.remove_server(slot_id),
        }
    }

    fn remove_server(&mut self, slot_id: ServerSlotId) {
        self.disconnect_slot(slot_id);
        let Ok(removed) = self.registry.remove(slot_id) else {
            return;
        };
        self.desired_connected_ids.remove(&removed.profile.id);
        self.storage.delete_password(&removed.profile.id);
        if let Err(err) = self.storage.save_registry(&self.registry) {
            self.toast_banner = Some((format!("Could not save server list: {err}"), true));
        }
        if self.active_slot == Some(slot_id) {
            self.active_slot = self.registry.iter().next().map(|slot| slot.slot_id);
        }
        self.persist_sessions_now();
    }
}

fn render_server_row(
    ui: &mut egui::Ui,
    palette: Palette,
    slot: &ServerSlot,
    is_selected: bool,
) -> Option<ServerAction> {
    let mut action = None;
    let slot_id = slot.slot_id;
    let [red, green, blue] = slot.profile.environment.rgb();
    let status_color = match &slot.status {
        ConnectionStatus::Connected => palette.success,
        ConnectionStatus::Connecting => palette.warning,
        ConnectionStatus::Error(_) => palette.danger,
        ConnectionStatus::Disconnected => palette.text_muted,
    };

    egui::Frame::new()
        .fill(if is_selected {
            palette.accent_soft
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                status_dot(ui, status_color);
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - SERVER_ROW_BADGE_WIDTH).max(40.0));
                    let title = format!("{} {}", slot_id, slot.profile.name);
                    let label = egui::Label::new(icon_text(
                        icon::DATABASE,
                        palette.icon_schema,
                        &title,
                        palette.text_primary,
                        13.0,
                    ))
                    .truncate()
                    .sense(Sense::click());
                    if ui.add(label).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        action = Some(ServerAction::Select(slot_id));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    pill(
                        ui,
                        slot.profile.environment.label(),
                        Color32::from_rgb(red, green, blue),
                    );
                });
            });
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let address = format!("{}@{}", slot.profile.username, slot.profile.endpoint.display_address());
                let status_text = match &slot.status {
                    ConnectionStatus::Connecting => "Connecting…".to_string(),
                    ConnectionStatus::Error(message) => message.clone(),
                    _ => address,
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(row_action) = render_server_row_actions(ui, palette, slot) {
                        action = Some(row_action);
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(status_text).size(11.5).color(palette.text_muted))
                                .truncate(),
                        );
                    });
                });
            });
        });
    action
}

fn render_server_row_actions(ui: &mut egui::Ui, palette: Palette, slot: &ServerSlot) -> Option<ServerAction> {
    let slot_id = slot.slot_id;
    let is_remove_clicked = icon_button(ui, palette, icon::TRASH, "Remove server").clicked();
    let is_edit_clicked = icon_button(ui, palette, icon::PENCIL_SIMPLE, "Edit server").clicked();
    let connection_action = match &slot.status {
        ConnectionStatus::Connected => icon_button(ui, palette, icon::PLUGS_CONNECTED, "Disconnect")
            .clicked()
            .then_some(ServerAction::Disconnect(slot_id)),
        ConnectionStatus::Connecting => None,
        _ => icon_button(ui, palette, icon::PLUG, "Connect")
            .clicked()
            .then_some(ServerAction::Connect(slot_id)),
    };
    if is_remove_clicked {
        return Some(ServerAction::Remove(slot_id));
    }
    if is_edit_clicked {
        return Some(ServerAction::Edit(slot_id));
    }
    connection_action
}
