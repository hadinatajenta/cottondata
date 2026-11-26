use super::navigator::contains_ascii_ignore_case;
use super::server_form::ServerModalState;
use super::CottonDataApp;
use crate::theme::{Palette, GUTTER, RADIUS_CONTROL};
use crate::ui_kit::{
    busy_indicator, caption, card_frame, ghost_button, icon, icon_button, icon_text, pill, secondary_button,
    section_title, status_dot, tree_toggle_icon,
};
use cottondata_core::{ConnectionStatus, ServerSlot, ServerSlotId};
use eframe::egui::{self, collapsing_header::CollapsingState, Color32, CornerRadius, Margin, RichText, Sense, Stroke};

const SERVER_TITLE_MIN_WIDTH: f32 = 40.0;
const EXPLORER_SEARCH_ID: &str = "explorer_filter_input";

pub(super) enum ServerAction {
    Select(ServerSlotId),
    Connect(ServerSlotId),
    Disconnect(ServerSlotId),
    NewConsole(ServerSlotId),
    Edit(ServerSlotId),
    Duplicate(ServerSlotId),
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
                let rect = ui.available_rect_before_wrap();
                ui.allocate_rect(rect, Sense::hover());
                let mut clipped = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                clipped.set_clip_rect(rect.intersect(ui.clip_rect()));
                card_frame(palette).show(&mut clipped, |ui| {
                    ui.set_max_width(ui.available_width());
                    ui.set_min_size(ui.available_size());
                    self.render_unified_explorer(ui, palette);
                });
            });
    }

    fn render_unified_explorer(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let inner_w = ui.available_width();
        ui.set_max_width(inner_w);

        let (header_rect, _) = ui.allocate_exact_size(egui::vec2(inner_w, 26.0), Sense::hover());
        let mut header_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(header_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        header_ui.set_clip_rect(header_rect.intersect(ui.clip_rect()));
        let can_add = !self.registry.is_full();
        if header_ui
            .add_enabled_ui(can_add, |ui| icon_button(ui, palette, icon::PLUS, "Add a server"))
            .inner
            .clicked()
        {
            self.modal = Some(ServerModalState::new_empty());
        }
        if header_ui
            .toggle_value(&mut self.show_system_schemas, RichText::new("System").size(11.5))
            .on_hover_text("Show system schemas and databases")
            .changed()
        {
            self.mark_session_dirty();
        }
        let title_rect = header_ui.available_rect_before_wrap();
        if title_rect.width() > 24.0 {
            let mut title_ui = header_ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(title_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            title_ui.set_clip_rect(title_rect.intersect(ui.clip_rect()));
            let title = if inner_w >= 265.0 {
                format!("Database Explorer {}/{}", self.registry.count(), self.registry.max_servers())
            } else {
                format!("Explorer {}/{}", self.registry.count(), self.registry.max_servers())
            };
            section_title(&mut title_ui, palette, icon::TREE_STRUCTURE, &title);
        }
        ui.add_space(4.0);

        let search_id = egui::Id::new(EXPLORER_SEARCH_ID);
        let is_search_focused = ui.memory(|m| m.has_focus(search_id));
        let (search_rect, search_bg_resp) = ui.allocate_exact_size(egui::vec2(inner_w, 30.0), Sense::click());
        let border_color = if is_search_focused {
            palette.accent
        } else {
            palette.border
        };
        ui.painter().rect(
            search_rect,
            CornerRadius::same(RADIUS_CONTROL),
            palette.bg_surface,
            Stroke::new(1.0, border_color),
            egui::StrokeKind::Inside,
        );

        let content_rect = search_rect.shrink2(egui::vec2(8.0, 4.0));
        let icon_rect = egui::Rect::from_min_size(content_rect.min, egui::vec2(18.0, content_rect.height()));
        let clear_rect = egui::Rect::from_min_max(
            egui::pos2((content_rect.right() - 22.0).max(icon_rect.right()), content_rect.top()),
            content_rect.max,
        );
        let input_rect = egui::Rect::from_min_max(
            egui::pos2(icon_rect.right() + 4.0, content_rect.top()),
            egui::pos2((clear_rect.left() - 4.0).max(icon_rect.right() + 24.0), content_rect.bottom()),
        );

        let mut icon_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(icon_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        icon_ui.label(RichText::new(icon::MAGNIFYING_GLASS).size(13.5).color(palette.text_muted));

        let mut input_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(input_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        input_ui.set_clip_rect(input_rect.intersect(ui.clip_rect()));
        let input_resp = input_ui.add(
            egui::TextEdit::singleline(&mut self.explorer_filter)
                .id(search_id)
                .hint_text("Filter servers, schemas, tables…")
                .clip_text(true)
                .frame(false)
                .desired_width(input_rect.width()),
        );
        if search_bg_resp.clicked() && !input_resp.has_focus() {
            input_resp.request_focus();
        }

        if !self.explorer_filter.is_empty() {
            let mut clear_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(clear_rect)
                    .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
            );
            clear_ui.spacing_mut().button_padding = egui::vec2(2.0, 0.0);
            if icon_button(&mut clear_ui, palette, icon::X, "Clear filter").clicked() {
                self.explorer_filter.clear();
                input_resp.request_focus();
            }
        }
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

        let scroll_max_w = ui.available_width();
        egui::ScrollArea::vertical()
            .id_salt("unified_explorer_scroll")
            .max_width(scroll_max_w)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(scroll_max_w);
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
        if contains_ascii_ignore_case(&slot.profile.name, filter) {
            return true;
        }
        let Some(catalog) = &self.catalogs[slot.slot_id.index()] else {
            return false;
        };
        catalog.schemas().iter().any(|schema| {
            if schema.is_system && !self.show_system_schemas {
                return false;
            }
            contains_ascii_ignore_case(catalog.resolve(schema.name), filter)
                || schema
                    .tables
                    .iter()
                    .any(|t| contains_ascii_ignore_case(catalog.resolve(t.name), filter))
                || schema
                    .routines
                    .iter()
                    .any(|(r, _)| contains_ascii_ignore_case(catalog.resolve(*r), filter))
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
            ServerAction::Duplicate(slot_id) => {
                if !self.registry.is_full() {
                    if let Some(slot) = self.registry.get(slot_id) {
                        let password = self.storage.get_password(&slot.profile.id);
                        self.modal = Some(ServerModalState::from_duplicate(&slot.profile, password));
                    }
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
            self.notify_error(format!("Could not save server list: {err}"));
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
            ui.set_max_width(ui.available_width());
            ui.horizontal(|ui| {
                state.show_toggle_button(ui, tree_toggle_icon);
                if matches!(slot.status, ConnectionStatus::Connecting) {
                    ui.add(egui::Spinner::new().size(10.0).color(palette.warning));
                } else {
                    status_dot(ui, status_color);
                }
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
                    let remaining_w = ui.available_width().max(SERVER_TITLE_MIN_WIDTH);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_max_width(remaining_w);
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
                            if ui.button(format!("{}  Duplicate server…", icon::COPY)).clicked() {
                                action = Some(ServerAction::Duplicate(slot_id));
                                ui.close_menu();
                            }
                            if ui.button(format!("{}  Remove server", icon::TRASH)).clicked() {
                                action = Some(ServerAction::Remove(slot_id));
                                ui.close_menu();
                            }
                        });
                    });
                });
            });
        })
        .response;

    (frame_resp, action)
}
