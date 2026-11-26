use super::query_tab::BottomDeckTab;
use super::CottonDataApp;
use crate::worker::{SlotCmd, WorkerEvent};
use cottondata_core::ConnectionStatus;
use cottondata_driver::QueryOutcome;
use cottondata_storage::ConnectedServerState;

impl CottonDataApp {
    pub(super) fn process_worker_events(&mut self) {
        let events = self.hub.poll_events();
        for ev in events {
            match ev {
                WorkerEvent::Connected { slot, runtime, catalog } => {
                    let current_schema = runtime.current_schema.clone();
                    let current_autocommit = runtime.autocommit;
                    if let Some(s) = self.registry.get_mut(slot) {
                        s.status = ConnectionStatus::Connected;
                        s.runtime = Some(runtime);
                    }
                    self.catalogs[slot.index()] = Some(catalog);
                    self.slot_activity[slot.index()].reset();
                    if let Some(restore) = self.pending_restore[slot.index()].take() {
                        if let Some(saved_schema) = restore.active_schema {
                            if current_schema.as_deref() != Some(saved_schema.as_str()) {
                                self.hub.send(slot, SlotCmd::SetSchema(saved_schema));
                            }
                        }
                        if restore.autocommit != current_autocommit {
                            self.hub.send(slot, SlotCmd::SetAutocommit(restore.autocommit));
                        }
                    }
                    self.reload_bound_tables_for_slot(slot);
                    self.persist_sessions_now();
                    self.toast_banner = Some((format!("Connected to slot {slot}"), false));
                }
                WorkerEvent::Disconnected { slot } => {
                    if let Some(s) = self.registry.get_mut(slot) {
                        s.status = ConnectionStatus::Disconnected;
                        s.runtime = None;
                    }
                    self.catalogs[slot.index()] = None;
                    self.processlists[slot.index()].clear();
                    self.health[slot.index()] = None;
                    self.clear_slot_activity(slot);
                    self.persist_sessions_now();
                    self.toast_banner = Some((format!("Disconnected slot {slot}"), false));
                }
                WorkerEvent::ConnectionLost { slot } => {
                    let server_state = self.registry.get(slot).map(|s| {
                        (
                            s.profile.id.clone(),
                            s.runtime.as_ref().and_then(|r| r.current_schema.clone()),
                            s.runtime.as_ref().map(|r| r.autocommit).unwrap_or(true),
                        )
                    });
                    self.catalogs[slot.index()] = None;
                    self.processlists[slot.index()].clear();
                    self.health[slot.index()] = None;
                    self.clear_slot_activity(slot);
                    if let Some(s) = self.registry.get_mut(slot) {
                        s.runtime = None;
                    }
                    if let Some((server_id, active_schema, autocommit)) = server_state {
                        if self.desired_connected_ids.contains(&server_id) {
                            self.pending_restore[slot.index()] = Some(ConnectedServerState {
                                server_id,
                                active_schema,
                                autocommit,
                            });
                            self.toast_banner =
                                Some((format!("Connection lost on slot {slot} — reconnecting…"), false));
                            self.connect_slot_internal(slot, false);
                        } else if let Some(s) = self.registry.get_mut(slot) {
                            s.status = ConnectionStatus::Disconnected;
                        }
                    }
                }
                WorkerEvent::Error { slot, tab_id, error } => {
                    let msg = error.to_string();
                    if tab_id.is_none() {
                        self.slot_activity[slot.index()].reset();
                        if let Some(s) = self.registry.get_mut(slot) {
                            if matches!(s.status, ConnectionStatus::Connecting) {
                                s.status = ConnectionStatus::Error(msg.clone());
                            }
                        }
                    }
                    if let Some(tid) = tab_id {
                        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tid) {
                            tab.is_running = false;
                            tab.is_counting_rows = false;
                            tab.is_submitting_mutations = false;
                            tab.is_loading_full_cell = false;
                            tab.last_status_msg = format!("Error: {msg}");
                        }
                    }
                    self.after_save_action = None;
                    self.toast_banner = Some((msg, true));
                }
                WorkerEvent::CatalogUpdated { slot, catalog } => {
                    self.catalogs[slot.index()] = Some(catalog);
                    self.slot_activity[slot.index()].loading_schemas.clear();
                }
                WorkerEvent::TableDetailLoaded { schema, table, detail } => {
                    for activity in &mut self.slot_activity {
                        if activity.is_loading_table_detail(&schema, &table) {
                            activity.loading_table_detail = None;
                        }
                    }
                    self.ddl_view = Some((schema, table, detail));
                    self.bottom_tab = BottomDeckTab::DdlInspector;
                }
                WorkerEvent::QueryFinished {
                    slot,
                    tab_id,
                    outcome,
                    runtime,
                } => {
                    let srv_name = self
                        .registry
                        .get(slot)
                        .map(|s| s.profile.name.clone())
                        .unwrap_or_else(|| slot.to_string());
                    let cur_schema = runtime.current_schema.clone();
                    let schema_changed = self
                        .registry
                        .get(slot)
                        .and_then(|s| s.runtime.as_ref())
                        .map(|r| r.current_schema != cur_schema)
                        .unwrap_or(false);
                    if let Some(s) = self.registry.get_mut(slot) {
                        s.runtime = Some(runtime);
                    }
                    if schema_changed {
                        self.mark_session_dirty();
                    }
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.is_running = false;
                        tab.mutator.revert_all();
                        tab.inline_editor = None;
                        tab.selected_cell = None;
                        match outcome {
                            QueryOutcome::Rows(page) => {
                                let bytes = page.approx_bytes();
                                let more = if page.has_more_rows { "+" } else { "" };
                                tab.last_status_msg = format!(
                                    "{}{} rows · {} cols · {} ms · {:.1} KiB",
                                    page.row_count,
                                    more,
                                    page.columns.len(),
                                    page.elapsed_ms,
                                    bytes as f64 / 1024.0
                                );
                                self.history = self.storage.record_history(
                                    srv_name,
                                    cur_schema,
                                    page.executed_sql.clone(),
                                    page.elapsed_ms,
                                    true,
                                );
                                let evicted = self.memory_budget.record_allocation(format!("tab-{}", tab.id), bytes);
                                tab.page = Some(page);
                                tab.column_widths.clear();
                                tab.row_selection.clear();
                                self.evict_pages(&evicted);
                            }
                            QueryOutcome::Affected {
                                affected_rows,
                                last_insert_id,
                                warnings,
                                elapsed_ms,
                                executed_sql,
                            } => {
                                let lid = last_insert_id
                                    .filter(|&v| v > 0)
                                    .map(|v| format!(" · last_insert_id={v}"))
                                    .unwrap_or_default();
                                tab.last_status_msg = format!(
                                    "OK · {affected_rows} rows affected{lid} · {warnings} warnings · {elapsed_ms} ms"
                                );
                                self.history =
                                    self.storage
                                        .record_history(srv_name, cur_schema, executed_sql, elapsed_ms, true);
                            }
                        }
                    }
                    self.bottom_tab = BottomDeckTab::DataGrid;
                }
                WorkerEvent::RowCountLoaded { tab_id, total_rows } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.is_counting_rows = false;
                        if let Some(page) = tab.page.as_mut() {
                            page.total_rows_known = Some(total_rows);
                        }
                    }
                }
                WorkerEvent::MutationsSubmitted {
                    slot,
                    tab_id,
                    statement_count,
                    elapsed_ms,
                } => {
                    self.toast_banner = Some((
                        format!("Saved {statement_count} SQL mutation(s) in {elapsed_ms} ms"),
                        false,
                    ));
                    let max_rows = self.registry.get(slot).map(|s| s.profile.page_size).unwrap_or(500);
                    let pending_after = self.after_save_action.take();
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.is_submitting_mutations = false;
                        tab.mutator.revert_all();
                        tab.inline_editor = None;
                        if pending_after.is_none() {
                            if let Some((schema, table)) = tab.bound_table.clone() {
                                tab.is_running = true;
                                self.hub.send(
                                    slot,
                                    SlotCmd::LoadTablePage {
                                        tab_id,
                                        schema,
                                        table,
                                        filter_sort: tab.filter_sort.clone(),
                                    },
                                );
                            } else if let Some(executed_sql) = tab
                                .page
                                .as_ref()
                                .map(|page| page.executed_sql.clone())
                                .filter(|sql| !sql.trim().is_empty())
                            {
                                tab.is_running = true;
                                self.hub.send(
                                    slot,
                                    SlotCmd::ExecuteSql {
                                        tab_id,
                                        sql: executed_sql,
                                        max_rows,
                                    },
                                );
                            }
                        }
                    }
                    if let Some(action) = pending_after {
                        self.execute_pending_discard_action(action);
                    }
                }
                WorkerEvent::FullCellLoaded {
                    tab_id,
                    row,
                    column,
                    value,
                } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.is_loading_full_cell = false;
                        if tab.selected_cell == Some((row, column)) {
                            match value {
                                Some(full_text) => {
                                    tab.cell_edit_is_null = false;
                                    tab.cell_edit_is_default = false;
                                    tab.cell_edit_buf = full_text;
                                    tab.is_inspector_open = true;
                                    tab.last_status_msg = "Loaded full cell value into inspector".into();
                                }
                                None => {
                                    tab.cell_edit_is_null = true;
                                    tab.cell_edit_is_default = false;
                                    tab.cell_edit_buf.clear();
                                    tab.is_inspector_open = true;
                                }
                            }
                        }
                    }
                }
                WorkerEvent::ExplainFinished { tab_id, result } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.is_running = false;
                        tab.explain = Some(result);
                    }
                    self.bottom_tab = BottomDeckTab::ExplainPlan;
                }
                WorkerEvent::RuntimeUpdated { slot, runtime } => {
                    self.slot_activity[slot.index()].is_session_command_pending = false;
                    if let Some(s) = self.registry.get_mut(slot) {
                        s.runtime = Some(runtime);
                    }
                    self.persist_sessions_now();
                }
                WorkerEvent::ProcesslistLoaded { slot, entries } => {
                    self.processlists[slot.index()] = entries;
                }
                WorkerEvent::HealthLoaded { slot, metrics } => {
                    self.health[slot.index()] = Some(metrics);
                    self.slot_activity[slot.index()].is_refreshing_monitor = false;
                }
            }
        }
    }

    fn clear_slot_activity(&mut self, slot: cottondata_core::ServerSlotId) {
        self.slot_activity[slot.index()].reset();
        for tab in self.tabs.iter_mut().filter(|tab| tab.slot == Some(slot)) {
            tab.is_running = false;
            tab.is_counting_rows = false;
            tab.is_submitting_mutations = false;
        }
    }

    fn evict_pages(&mut self, keys: &[String]) {
        for key in keys {
            if let Some(id_str) = key.strip_prefix("tab-") {
                if let Ok(tid) = id_str.parse::<u64>() {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tid) {
                        tab.page = None;
                        tab.last_status_msg =
                            "Page evicted by the 32 MiB RAM budget. Run or reload to fetch it again".into();
                    }
                }
            }
        }
    }
}
