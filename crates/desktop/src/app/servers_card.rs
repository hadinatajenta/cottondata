use super::server_form::ServerModalState;
use super::CottonDataApp;
use crate::theme::{Palette, GUTTER, RADIUS_CONTROL};
use crate::ui_kit::{
    busy_indicator, caption, card_frame, ghost_button, icon, icon_button, icon_text, inset_frame, pill,
    secondary_button, section_title, status_dot, tree_toggle_icon,
};
use cottondata_core::{ConnectionStatus, ServerSlot, ServerSlotId};
use eframe::egui::{self, collapsing_header::CollapsingState, Color32, CornerRadius, Margin, RichText, Sense};

const SERVER_ROW_BADGE_WIDTH: f32 = 64.0;

pub(super) enum ServerAction {
    Select(ServerSlotId),
    Connect(ServerSlotId),
    Disconnect(ServerSlotId),
    NewConsole(ServerSlotId),
    Edit(ServerSlotId),
    Remove(ServerSlotId),
}

impl CottonDataApp {
    pub(super) fn render_sidebar(&mut self, ctx: &egui::Context, palette: Palette) {
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .show_separator_line(false)
            .default_width(300.0)
            .min_width(240.0)
            .max_width(480.0)
            .frame(egui::Frame::new().fill(palette.bg_backdrop).inner_margin(Margin {
                left: GUTTER,
                right: 0,
                top: GUTTER,
                bottom: GUTTER,
            }))
            .show(ctx, |ui| {
                card_frame(palette).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    self.render_unified_explorer(ui, palette);
                });
            });
    }

    fn render_unified_explorer(&mut self, ui: &mut egui::Ui, palette: Palette) {
        ui.horizontal(|ui| {
            section_title(ui, palette, icon::TREE_STRUCTURE, "Database Explorer");
            caption(
                ui,
                palette,
                format!("{}/{}", self.registry.count(), self.registry.max_servers()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_add = !self.registry.is_full();
                if ui
                    .add_enabled_ui(can_add, |ui| icon_button(ui, palette, icon::PLUS, "Add a server"))
                    .inner
                    .clicked()
                {
                    self.modal = Some(ServerModalState::new_empty());
                }
                if ui
                    .toggle_value(&mut self.show_system_schemas, RichText::new("System").size(11.5))
                    .on_hover_text("Show system schemas and databases")
                    .changed()
                {
                    self.mark_session_dirty();
                }
            });
        });
        ui.add_space(4.0);

        inset_frame(palette)
            .inner_margin(Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(palette.text_muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.explorer_filter)
                            .hint_text("Filter servers, schemas, tables…")
                            .frame(false)
                            .desired_width(f32::INFINITY),
                    );
                    if !self.explorer_filter.is_empty()
                        && icon_button(ui, palette, icon::X, "Clear filter").clicked()
                    {
                        self.explorer_filter.clear();
                    }
                });
            });
        ui.add_space(6.0);

        if self.registry.count() == 0 {
            caption(ui, palette, "No servers yet. Add MySQL or SQL Server connections.");
            ui.add_space(6.0);
            if secondary_button(ui, palette, icon::PLUS, "Add server", true).clicked() {
                self.modal = Some(ServerModalState::new_empty());
            }
            return;
        }

        let filter = self.explorer_filter.trim().to_ascii_lowercase();
        let slot_ids: Vec<ServerSlotId> = self.registry.iter().map(|slot| slot.slot_id).collect();
        let mut pending_server_action = None;
        let mut pending_nav = Vec::new();

        egui::ScrollArea::vertical()
            .id_salt("unified_explorer_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for slot_id in slot_ids {
                    let Some(slot) = self.registry.get(slot_id) else {
                        continue;
                    };
                    if !filter.is_empty() && !self.server_matches_filter(slot, &filter) {
                        continue;
                    }
                    let is_selected = self.active_slot == Some(slot_id);
                    let is_connected = matches!(slot.status, ConnectionStatus::Connected);
                    let is_connecting = matches!(slot.status, ConnectionStatus::Connecting);
                    let node_id = ui.make_persistent_id(("explorer_server", slot_id.index()));
                    let mut state = CollapsingState::load_with_default_open(ui.ctx(), node_id, is_connected);
                    if !filter.is_empty() && is_connected {
                        state.set_open(true);
                    }

                    let (header_resp, row_action) =
                        render_server_tree_header(ui, palette, slot, is_selected, &mut state);
                    if let Some(act) = row_action {
                        pending_server_action = Some(act);
                    }

                    let is_open = state.is_open();
                    let was_open = self.expanded_servers[slot_id.index()];
                    if is_open && !was_open && !is_connected && !is_connecting {
                        pending_server_action = Some(ServerAction::Connect(slot_id));
                    }
                    self.expanded_servers[slot_id.index()] = is_open;

                    state.show_body_indented(&header_resp, ui, |ui| {
                        let Some(slot) = self.registry.get(slot_id) else {
                            return;
                        };
                        match &slot.status {
                            ConnectionStatus::Connecting => {
                                ui.horizontal(|ui| busy_indicator(ui, palette, "Connecting…"));
                            }
                            ConnectionStatus::Error(message) => {
                                ui.label(RichText::new(message).size(11.0).color(palette.danger));
                                if ghost_button(ui, palette, icon::ARROW_CLOCKWISE, "Retry connection", true).clicked()
                                {
                                    pending_server_action = Some(ServerAction::Connect(slot_id));
                                }
                            }
                            ConnectionStatus::Disconnected => {
                                if ghost_button(ui, palette, icon::PLUG, "Connect", true).clicked() {
                                    pending_server_action = Some(ServerAction::Connect(slot_id));
                                }
                            }
                            ConnectionStatus::Connected => {
                                if let Some(req) = self.render_slot_schemas(ui, palette, slot_id, &filter) {
                                    pending_nav.push((slot_id, req));
                                }
                            }
                        }
                    });
                    ui.add_space(2.0);
                }
            });

        if let Some(action) = pending_server_action {
            self.apply_server_action(action);
        }
        for (slot_id, req) in pending_nav {
            self.apply_navigator_requests(ui.ctx(), slot_id, req);
        }
    }

    fn server_matches_filter(&self, slot: &ServerSlot, filter: &str) -> bool {
        if slot.profile.name.to_ascii_lowercase().contains(filter) {
            return true;
        }
        let Some(catalog) = &self.catalogs[slot.slot_id.index()] else {
            return false;
        };
        catalog.schemas().iter().any(|schema| {
            if schema.is_system && !self.show_system_schemas {
                return false;
            }
            catalog.resolve(schema.name).to_ascii_lowercase().contains(filter)
                || schema
                    .tables
                    .iter()
                    .any(|t| catalog.resolve(t.name).to_ascii_lowercase().contains(filter))
                || schema
                    .routines
                    .iter()
                    .any(|(r, _)| catalog.resolve(*r).to_ascii_lowercase().contains(filter))
        })
    }

    fn apply_server_action(&mut self, action: ServerAction) {
        match action {
            ServerAction::Select(slot_id) => {
                self.active_slot = Some(slot_id);
                self.mark_session_dirty();
            }
            ServerAction::Connect(slot_id) => self.connect_slot(slot_id),
            ServerAction::Disconnect(slot_id) => self.disconnect_slot(slot_id),
            ServerAction::NewConsole(slot_id) => self.open_console_tab_for_slot(Some(slot_id), None),
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

fn render_server_tree_header(
    ui: &mut egui::Ui,
    palette: Palette,
    slot: &ServerSlot,
    is_selected: bool,
    state: &mut CollapsingState,
) -> (egui::Response, Option<ServerAction>) {
    let mut action = None;
    let slot_id = slot.slot_id;
    let [red, green, blue] = slot.profile.environment.rgb();
    let status_color = match &slot.status {
        ConnectionStatus::Connected => palette.success,
        ConnectionStatus::Connecting => palette.warning,
        ConnectionStatus::Error(_) => palette.danger,
        ConnectionStatus::Disconnected => palette.text_muted,
    };

    let frame_resp = egui::Frame::new()
        .fill(if is_selected {
            palette.accent_soft
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(Margin::symmetric(4, 3))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                state.show_toggle_button(ui, tree_toggle_icon);
                if matches!(slot.status, ConnectionStatus::Connecting) {
                    ui.add(egui::Spinner::new().size(10.0).color(palette.warning));
                } else {
                    status_dot(ui, status_color);
                }
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - SERVER_ROW_BADGE_WIDTH - 24.0).max(40.0));
                    let title = format!("{} {}", slot_id, slot.profile.name);
                    let address = format!(
                        "{}@{} ({})",
                        slot.profile.username,
                        slot.profile.endpoint.display_address(),
                        slot.profile.engine.label()
                    );
                    let label = egui::Label::new(icon_text(
                        icon::DATABASE,
                        palette.icon_schema,
                        &title,
                        palette.text_primary,
                        13.0,
                    ))
                    .truncate()
                    .sense(Sense::click());
                    let resp = ui
                        .add(label)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(address);
                    if resp.double_clicked() {
                        state.toggle(ui);
                        if matches!(slot.status, ConnectionStatus::Disconnected | ConnectionStatus::Error(_)) {
                            action = Some(ServerAction::Connect(slot_id));
                        }
                    } else if resp.clicked() {
                        action = Some(ServerAction::Select(slot_id));
                    }
                    resp.context_menu(|ui| {
                        ui.set_min_width(180.0);
                        match &slot.status {
                            ConnectionStatus::Connected => {
                                if ui.button(format!("{}  Disconnect", icon::PLUGS_CONNECTED)).clicked() {
                                    action = Some(ServerAction::Disconnect(slot_id));
                                    ui.close_menu();
                                }
                            }
                            ConnectionStatus::Connecting => {}
                            _ => {
                                if ui.button(format!("{}  Connect", icon::PLUG)).clicked() {
                                    action = Some(ServerAction::Connect(slot_id));
                                    state.set_open(true);
                                    ui.close_menu();
                                }
                            }
                        }
                        if ui.button(format!("{}  New SQL Console", icon::TERMINAL_WINDOW)).clicked() {
                            action = Some(ServerAction::NewConsole(slot_id));
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button(format!("{}  Edit server…", icon::PENCIL_SIMPLE)).clicked() {
                            action = Some(ServerAction::Edit(slot_id));
                            ui.close_menu();
                        }
                        if ui.button(format!("{}  Remove server", icon::TRASH)).clicked() {
                            action = Some(ServerAction::Remove(slot_id));
                            ui.close_menu();
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match &slot.status {
                        ConnectionStatus::Connected => {
                            if icon_button(ui, palette, icon::PLUGS_CONNECTED, "Disconnect").clicked() {
                                action = Some(ServerAction::Disconnect(slot_id));
                            }
                        }
                        ConnectionStatus::Connecting => {}
                        _ => {
                            if icon_button(ui, palette, icon::PLUG, "Connect").clicked() {
                                action = Some(ServerAction::Connect(slot_id));
                                state.set_open(true);
                            }
                        }
                    }
                    pill(
                        ui,
                        slot.profile.environment.label(),
                        Color32::from_rgb(red, green, blue),
                    );
                });
            });
        })
        .response;

    (frame_resp, action)
}
