use super::query_tab::QueryTab;
use super::CottonDataApp;
use crate::theme::{AppTheme, Palette, RADIUS_CONTROL};
use crate::ui_kit::{
    banner, caption, field_label, icon, icon_button, icon_text, inset_frame, modal_shell, pill, primary_button,
    secondary_button, styled_text_input, tinted_ghost_button, ModalConfig,
};
use crate::worker::SlotCmd;
use cottondata_core::{ConnectionStatus, ServerSlotId};
use cottondata_storage::{now_secs, ConnectedServerState};
use eframe::egui::{self, CornerRadius, Margin};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(super) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(45);
pub(super) const SESSION_AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(400);

pub(super) struct SessionModalState {
    pub(super) new_session_name: String,
    pub(super) renaming_id: Option<String>,
    pub(super) rename_buf: String,
    pub(super) error_msg: Option<String>,
}

impl SessionModalState {
    pub(super) fn new() -> Self {
        Self {
            new_session_name: String::new(),
            renaming_id: None,
            rename_buf: String::new(),
            error_msg: None,
        }
    }
}

enum SessionModalAction {
    Switch(String),
    Create(String),
    Duplicate(String),
    CommitRename(String, String),
    Delete(String),
    ReconnectAll,
    SaveNow,
}

impl CottonDataApp {
    pub(super) fn slot_for_server_id(&self, server_id: &str) -> Option<ServerSlotId> {
        self.registry
            .iter()
            .find(|s| s.profile.id == server_id)
            .map(|s| s.slot_id)
    }

    pub(super) fn server_id_for_slot(&self, slot: ServerSlotId) -> Option<String> {
        self.registry.get(slot).map(|s| s.profile.id.clone())
    }

    pub(super) fn restore_active_session(&mut self, auto_connect: bool) {
        let session = self.sessions_store.active_session().clone();

        self.active_slot = session
            .active_server_id
            .as_deref()
            .and_then(|id| self.slot_for_server_id(id))
            .or_else(|| self.registry.iter().next().map(|s| s.slot_id));

        let mut restored_tabs = Vec::with_capacity(session.tabs.len());
        for saved_tab in &session.tabs {
            let slot = saved_tab
                .server_id
                .as_deref()
                .and_then(|id| self.slot_for_server_id(id))
                .or(self.active_slot);
            restored_tabs.push(QueryTab::from_session_state(saved_tab, slot));
        }

        if restored_tabs.is_empty() {
            restored_tabs.push(QueryTab::new_console(1, self.active_slot));
        }

        self.tabs = restored_tabs;
        self.active_tab_idx = session.active_tab_idx.min(self.tabs.len().saturating_sub(1));
        let max_id = self.tabs.iter().map(|t| t.id).max().unwrap_or(0);
        self.next_tab_id = session.next_tab_id.max(max_id + 1);
        self.editor_height = (session.editor_height as f32).clamp(100.0, 640.0);
        self.ddl_view = None;

        if !auto_connect {
            return;
        }

        let mut already_connected_slots = Vec::new();
        for conn_state in &session.connected_servers {
            let Some(slot_id) = self.slot_for_server_id(&conn_state.server_id) else {
                continue;
            };
            self.desired_connected_ids.insert(conn_state.server_id.clone());
            let status = self
                .registry
                .get(slot_id)
                .map(|s| s.status.clone())
                .unwrap_or(ConnectionStatus::Disconnected);
            match status {
                ConnectionStatus::Connected => {
                    if let Some(saved_schema) = &conn_state.active_schema {
                        let current_schema = self
                            .registry
                            .get(slot_id)
                            .and_then(|s| s.runtime.as_ref())
                            .and_then(|r| r.current_schema.as_deref());
                        if current_schema != Some(saved_schema.as_str()) {
                            self.hub.send(slot_id, SlotCmd::SetSchema(saved_schema.clone()));
                        }
                    }
                    let current_auto = self
                        .registry
                        .get(slot_id)
                        .and_then(|s| s.runtime.as_ref())
                        .map(|r| r.autocommit)
                        .unwrap_or(true);
                    if current_auto != conn_state.autocommit {
                        self.hub.send(slot_id, SlotCmd::SetAutocommit(conn_state.autocommit));
                    }
                    already_connected_slots.push(slot_id);
                }
                ConnectionStatus::Connecting => {
                    self.pending_restore[slot_id.index()] = Some(conn_state.clone());
                }
                ConnectionStatus::Disconnected | ConnectionStatus::Error(_) => {
                    self.pending_restore[slot_id.index()] = Some(conn_state.clone());
                    self.connect_slot_internal(slot_id, false);
                }
            }
        }

        for slot_id in already_connected_slots {
            self.reload_bound_tables_for_slot(slot_id);
        }
    }

    pub(super) fn reload_bound_tables_for_slot(&mut self, slot: ServerSlotId) {
        let mut to_load = Vec::new();
        for tab in &mut self.tabs {
            if tab.slot == Some(slot) && tab.page.is_none() && !tab.is_running {
                if let Some((schema, table)) = tab.bound_table.clone() {
                    tab.is_running = true;
                    tab.last_status_msg = "Loading table page…".into();
                    to_load.push((tab.id, schema, table, tab.filter_sort.clone()));
                }
            }
        }
        for (tab_id, schema, table, filter_sort) in to_load {
            self.hub.send(
                slot,
                SlotCmd::LoadTablePage {
                    tab_id,
                    schema,
                    table,
                    filter_sort,
                },
            );
        }
    }

    pub(super) fn sync_active_session_to_store(&mut self) {
        self.sessions_store.dark_theme = matches!(self.theme, AppTheme::Dark);
        self.sessions_store.show_system_schemas = self.show_system_schemas;

        let active_server_id = self.active_slot.and_then(|s| self.server_id_for_slot(s));
        let tabs = self
            .tabs
            .iter()
            .map(|tab| tab.to_session_state(tab.slot.and_then(|s| self.server_id_for_slot(s))))
            .collect();

        let previous_saved: HashMap<String, ConnectedServerState> = self
            .sessions_store
            .active_session()
            .connected_servers
            .iter()
            .cloned()
            .map(|c| (c.server_id.clone(), c))
            .collect();

        let mut connected_servers = Vec::new();
        for slot in self.registry.iter() {
            let should_persist = matches!(
                slot.status,
                ConnectionStatus::Connected | ConnectionStatus::Connecting
            ) || self.desired_connected_ids.contains(&slot.profile.id);
            if !should_persist {
                continue;
            }
            let pending = self.pending_restore[slot.slot_id.index()].as_ref();
            let prev = previous_saved.get(&slot.profile.id);

            let active_schema = slot
                .runtime
                .as_ref()
                .and_then(|r| r.current_schema.clone())
                .or_else(|| pending.and_then(|p| p.active_schema.clone()))
                .or_else(|| prev.and_then(|p| p.active_schema.clone()))
                .or_else(|| slot.profile.default_schema.clone());
            let autocommit = slot
                .runtime
                .as_ref()
                .map(|r| r.autocommit)
                .or_else(|| pending.map(|p| p.autocommit))
                .or_else(|| prev.map(|p| p.autocommit))
                .unwrap_or(true);

            connected_servers.push(ConnectedServerState {
                server_id: slot.profile.id.clone(),
                active_schema,
                autocommit,
            });
        }

        let active_tab_idx = self.active_tab_idx;
        let next_tab_id = self.next_tab_id;
        let editor_height = self.editor_height.round().clamp(100.0, 640.0) as u32;
        let session = self.sessions_store.active_session_mut();
        session.active_server_id = active_server_id;
        session.active_tab_idx = active_tab_idx;
        session.next_tab_id = next_tab_id;
        session.tabs = tabs;
        session.connected_servers = connected_servers;
        session.editor_height = editor_height;
        session.updated_at_secs = now_secs();
    }

    pub(super) fn persist_sessions_now(&mut self) {
        self.sync_active_session_to_store();
        self.session_dirty_since = None;
        if let Err(err) = self.storage.save_sessions(&self.sessions_store) {
            self.notify_error(format!("Could not save workspace session: {err}"));
        }
    }

    pub(super) fn mark_session_dirty(&mut self) {
        if self.session_dirty_since.is_none() {
            self.session_dirty_since = Some(Instant::now());
        }
    }

    pub(super) fn flush_debounced_session(&mut self, ctx: &egui::Context) {
        if let Some(since) = self.session_dirty_since {
            let elapsed = since.elapsed();
            if elapsed >= SESSION_AUTOSAVE_DEBOUNCE {
                self.persist_sessions_now();
            } else {
                ctx.request_repaint_after(SESSION_AUTOSAVE_DEBOUNCE.saturating_sub(elapsed));
            }
        }
    }

    pub(super) fn tick_keepalive(&mut self, ctx: &egui::Context) {
        let connected_slots: Vec<ServerSlotId> = self
            .registry
            .iter()
            .filter(|s| matches!(s.status, ConnectionStatus::Connected))
            .map(|s| s.slot_id)
            .collect();
        if connected_slots.is_empty() {
            return;
        }
        let elapsed = self.last_ping_at.elapsed();
        if elapsed >= KEEPALIVE_INTERVAL {
            self.last_ping_at = Instant::now();
            for slot in connected_slots {
                self.hub.send(slot, SlotCmd::Ping);
            }
            ctx.request_repaint_after(KEEPALIVE_INTERVAL);
        } else {
            ctx.request_repaint_after(
                KEEPALIVE_INTERVAL
                    .saturating_sub(elapsed)
                    .max(Duration::from_secs(1)),
            );
        }
    }

    pub(super) fn switch_session(&mut self, session_id: &str) {
        if self.sessions_store.active_session_id == session_id {
            return;
        }
        self.persist_sessions_now();
        if self.sessions_store.select_session(session_id) {
            self.restore_active_session(true);
            let _ = self.storage.save_sessions(&self.sessions_store);
            let name = self.sessions_store.active_session().name.clone();
            self.notify_info(format!("Switched to session '{name}'"));
        }
    }

    pub(super) fn reconnect_all_session_servers(&mut self) {
        let target_ids: Vec<String> = self.desired_connected_ids.iter().cloned().collect();
        let mut started = 0usize;
        for server_id in target_ids {
            let Some(slot_id) = self.slot_for_server_id(&server_id) else {
                continue;
            };
            let is_busy_or_up = self
                .registry
                .get(slot_id)
                .map(|s| matches!(s.status, ConnectionStatus::Connected | ConnectionStatus::Connecting))
                .unwrap_or(true);
            if !is_busy_or_up {
                self.connect_slot_internal(slot_id, false);
                started += 1;
            }
        }
        if started > 0 {
            self.notify_info(format!("Reconnecting {started} session server(s)…"));
        } else {
            self.notify_info("All session servers are already connected.");
        }
    }

    pub(super) fn render_sessions_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(mut state) = self.session_modal.take() else {
            return;
        };
        let mut keep_open = true;
        let mut action: Option<SessionModalAction> = None;
        let mut footer_action: Option<SessionModalAction> = None;

        let active_id = self.sessions_store.active_session_id.clone();
        let active_tab_count = self.tabs.len();
        let active_conn_count = self
            .registry
            .iter()
            .filter(|s| {
                matches!(s.status, ConnectionStatus::Connected | ConnectionStatus::Connecting)
                    || self.desired_connected_ids.contains(&s.profile.id)
            })
            .count();
        let session_count = self.sessions_store.sessions.len();
        let snapshots: Vec<(String, String, usize, usize)> = self
            .sessions_store
            .sessions
            .iter()
            .map(|s| {
                let is_active = s.id == active_id;
                let tabs = if is_active { active_tab_count } else { s.tabs.len().max(1) };
                let conns = if is_active {
                    active_conn_count
                } else {
                    s.connected_servers.len()
                };
                (s.id.clone(), s.name.clone(), tabs, conns)
            })
            .collect();

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "workspace_sessions_modal",
                icon_glyph: icon::STACK,
                icon_color: p.accent,
                title: "Workspace Sessions",
                subtitle: Some("Console tabs, SQL drafts, and connections auto-save and restore on launch"),
                accent_bar: None,
                width: 540.0,
                max_height: None,
                dismiss_on_backdrop: true,
            },
            |ui| {
                if let Some(err) = &state.error_msg {
                    banner(ui, p, p.danger, icon::WARNING, err, false);
                    ui.add_space(10.0);
                }

                inset_frame(p).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("sessions_modal_list")
                        .max_height(240.0)
                        .show(ui, |ui| {
                            for (sid, sname, tabs, conns) in &snapshots {
                                let is_active = *sid == active_id;
                                let is_renaming = state.renaming_id.as_deref() == Some(sid.as_str());
                                egui::Frame::new()
                                    .fill(if is_active {
                                        p.accent_soft
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    })
                                    .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                                    .inner_margin(Margin::symmetric(10, 7))
                                    .show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            if is_renaming {
                                                let resp = styled_text_input(
                                                    ui,
                                                    p,
                                                    ("session_rename", sid),
                                                    &mut state.rename_buf,
                                                    "Session name",
                                                    210.0,
                                                );
                                                let enter =
                                                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                                if enter || icon_button(ui, p, icon::CHECK, "Save name").clicked() {
                                                    action = Some(SessionModalAction::CommitRename(
                                                        sid.clone(),
                                                        state.rename_buf.clone(),
                                                    ));
                                                }
                                                if icon_button(ui, p, icon::X, "Cancel rename").clicked() {
                                                    state.renaming_id = None;
                                                }
                                            } else {
                                                ui.label(icon_text(
                                                    icon::STACK,
                                                    if is_active { p.accent } else { p.text_muted },
                                                    sname,
                                                    p.text_primary,
                                                    13.0,
                                                ));
                                                if is_active {
                                                    pill(ui, "ACTIVE", p.accent);
                                                }
                                                caption(ui, p, format!("{tabs} tabs · {conns} servers"));
                                            }

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if session_count > 1
                                                        && tinted_ghost_button(ui, p.danger, icon::TRASH, "", true)
                                                            .on_hover_text("Delete session")
                                                            .clicked()
                                                    {
                                                        action = Some(SessionModalAction::Delete(sid.clone()));
                                                    }
                                                    if !is_renaming
                                                        && icon_button(ui, p, icon::PENCIL_SIMPLE, "Rename session")
                                                            .clicked()
                                                    {
                                                        state.renaming_id = Some(sid.clone());
                                                        state.rename_buf = sname.clone();
                                                    }
                                                    if !is_active
                                                        && !is_renaming
                                                        && secondary_button(ui, p, icon::PLAY, "Open", true).clicked()
                                                    {
                                                        action = Some(SessionModalAction::Switch(sid.clone()));
                                                    }
                                                },
                                            );
                                        });
                                    });
                            }
                        });
                });

                ui.add_space(12.0);
                field_label(ui, p, "Create or Duplicate Session");
                let full_w = ui.available_width();
                ui.horizontal(|ui| {
                    let input_w = (full_w - 230.0).max(160.0);
                    styled_text_input(
                        ui,
                        p,
                        "new_session_name_input",
                        &mut state.new_session_name,
                        "e.g. Prod Debug, Migration, Analytics",
                        input_w,
                    );
                    if primary_button(ui, p, icon::PLUS, "Create", true).clicked() {
                        action = Some(SessionModalAction::Create(state.new_session_name.clone()));
                    }
                    if secondary_button(ui, p, icon::COPY, "Duplicate", true).clicked() {
                        action = Some(SessionModalAction::Duplicate(state.new_session_name.clone()));
                    }
                });
            },
            |ui| {
                if secondary_button(ui, p, icon::ARROW_CLOCKWISE, "Reconnect servers", true).clicked() {
                    footer_action = Some(SessionModalAction::ReconnectAll);
                }
                if secondary_button(ui, p, icon::FLOPPY_DISK, "Save now", true).clicked() {
                    footer_action = Some(SessionModalAction::SaveNow);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if secondary_button(ui, p, icon::X, "Close", true).clicked() {
                        keep_open = false;
                    }
                });
            },
        );
        if outcome.closed {
            keep_open = false;
        }
        let action = action.or(footer_action);

        if let Some(act) = action {
            match act {
                SessionModalAction::Switch(sid) => {
                    self.switch_session(&sid);
                    state.error_msg = None;
                }
                SessionModalAction::Create(name) => {
                    self.persist_sessions_now();
                    match self.sessions_store.create_session(name) {
                        Ok(_) => {
                            self.restore_active_session(false);
                            self.persist_sessions_now();
                            state.new_session_name.clear();
                            state.error_msg = None;
                        }
                        Err(err) => state.error_msg = Some(err),
                    }
                }
                SessionModalAction::Duplicate(name) => {
                    self.persist_sessions_now();
                    let target_name = if name.trim().is_empty() {
                        format!("{} Copy", self.sessions_store.active_session().name)
                    } else {
                        name
                    };
                    match self.sessions_store.duplicate_active_session(target_name) {
                        Ok(_) => {
                            self.restore_active_session(true);
                            self.persist_sessions_now();
                            state.new_session_name.clear();
                            state.error_msg = None;
                        }
                        Err(err) => state.error_msg = Some(err),
                    }
                }
                SessionModalAction::CommitRename(sid, new_name) => {
                    match self.sessions_store.rename_session(&sid, new_name) {
                        Ok(()) => {
                            state.renaming_id = None;
                            state.error_msg = None;
                            self.persist_sessions_now();
                        }
                        Err(err) => state.error_msg = Some(err),
                    }
                }
                SessionModalAction::Delete(sid) => {
                    let was_active = self.sessions_store.active_session_id == sid;
                    match self.sessions_store.delete_session(&sid) {
                        Ok(()) => {
                            if was_active {
                                self.restore_active_session(true);
                            }
                            self.persist_sessions_now();
                            state.error_msg = None;
                        }
                        Err(err) => state.error_msg = Some(err),
                    }
                }
                SessionModalAction::ReconnectAll => {
                    self.reconnect_all_session_servers();
                }
                SessionModalAction::SaveNow => {
                    self.persist_sessions_now();
                    self.notify_success("Workspace session saved.");
                }
            }
        }

        if keep_open {
            self.session_modal = Some(state);
        }
    }
}
