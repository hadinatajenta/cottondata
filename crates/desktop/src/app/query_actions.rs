use super::params_modal::ParamModalState;
use super::query_tab::QueryTab;
use super::{CottonDataApp, PendingDiscardAction};
use crate::worker::SlotCmd;
use cottondata_core::ServerSlotId;
use cottondata_sql::{extract_params_for_statements, query_param_fingerprint, split_statements, statement_at_offset};

impl CottonDataApp {
    pub(super) fn open_table_tab(&mut self, slot: ServerSlotId, schema: String, table: String) {
        let existing_tab = self.tabs.iter().position(|tab| {
            tab.slot == Some(slot)
                && tab.filter_sort.where_clause.trim().is_empty()
                && tab
                    .bound_table
                    .as_ref()
                    .is_some_and(|(bound_schema, bound_table)| bound_schema == &schema && bound_table == &table)
        });
        if let Some(idx) = existing_tab {
            self.activate_tab(idx);
            self.ensure_table_detail_loaded(slot, &schema, &table);
            return;
        }
        self.open_filtered_table_tab(slot, schema, table, String::new());
    }

    pub(super) fn open_filtered_table_tab(
        &mut self,
        slot: ServerSlotId,
        schema: String,
        table: String,
        where_clause: String,
    ) {
        let page_size = self.registry.get(slot).map(|s| s.profile.page_size).unwrap_or(500);
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let dialect = self.slot_dialect(Some(slot));
        let mut tab = QueryTab::new_table(id, slot, dialect, schema.clone(), table.clone(), page_size);
        if !where_clause.trim().is_empty() {
            tab.filter_sort.where_clause = where_clause;
            tab.sql = tab.filter_sort.build_select_sql(dialect, &schema, &table);
        }
        let filter_sort = tab.filter_sort.clone();
        self.tabs.push(tab);
        self.active_tab_idx = self.tabs.len() - 1;
        self.hub.send(
            slot,
            SlotCmd::LoadTablePage {
                tab_id: id,
                schema: schema.clone(),
                table: table.clone(),
                filter_sort,
            },
        );
        self.ensure_table_detail_loaded(slot, &schema, &table);
        self.persist_sessions_now();
    }

    pub(super) fn request_execute_active_tab(&mut self, run_all: bool) {
        if self.tabs[self.active_tab_idx].has_any_unsaved_changes() {
            self.unsaved_guard_modal = Some(PendingDiscardAction::RunQuery(run_all));
            return;
        }
        self.execute_active_tab(run_all);
    }

    pub(super) fn execute_active_tab(&mut self, run_all: bool) {
        let Some(tab) = self.tabs.get_mut(self.active_tab_idx) else {
            return;
        };
        if tab.is_running {
            tab.last_status_msg = "A query is already running on this tab. Stop it or wait for it to finish".into();
            return;
        }
        let slot = tab.slot.or(self.active_slot);
        let Some(slot) = slot else {
            self.notify_error("Add and connect a server first.");
            return;
        };
        tab.slot = Some(slot);
        let max_rows = self.registry.get(slot).map(|s| s.profile.page_size).unwrap_or(500);
        self.clear_toast();
        let tab = &mut self.tabs[self.active_tab_idx];

        let statements: Vec<String> = if run_all {
            split_statements(&tab.sql)
                .into_iter()
                .map(|s| s.sql.to_string())
                .filter(|s| !s.trim().is_empty())
                .collect()
        } else if let Some((start, end)) = tab.selection_byte_range {
            let safe_start = start.min(tab.sql.len());
            let safe_end = end.min(tab.sql.len());
            if safe_start < safe_end
                && tab.sql.is_char_boundary(safe_start)
                && tab.sql.is_char_boundary(safe_end)
            {
                let selected = &tab.sql[safe_start..safe_end];
                let split: Vec<String> = split_statements(selected)
                    .into_iter()
                    .map(|s| s.sql.to_string())
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                if split.is_empty() && !selected.trim().is_empty() {
                    vec![selected.trim().to_string()]
                } else {
                    split
                }
            } else {
                Vec::new()
            }
        } else {
            let single = statement_at_offset(&tab.sql, tab.cursor_byte)
                .map(|s| s.sql.to_string())
                .unwrap_or_else(|| tab.sql.trim().to_string());
            if single.trim().is_empty() {
                Vec::new()
            } else {
                vec![single]
            }
        };

        if statements.is_empty() {
            return;
        }
        let tab_id = tab.id;
        let params = extract_params_for_statements(&statements);
        if !params.is_empty() {
            let fingerprint = query_param_fingerprint(&statements.join("\n"));
            let saved = self.param_history.get(&fingerprint);
            self.param_modal = Some(ParamModalState::new(
                slot,
                tab_id,
                max_rows,
                fingerprint,
                statements,
                params,
                saved,
            ));
            return;
        }
        self.dispatch_statements(slot, tab_id, statements, max_rows);
    }

    pub(super) fn dispatch_statements(
        &mut self,
        slot: ServerSlotId,
        tab_id: u64,
        statements: Vec<String>,
        max_rows: u32,
    ) {
        if statements.is_empty() {
            return;
        }
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.is_running = true;
            if statements.len() > 1 {
                tab.last_status_msg = format!("Executing {} statements…", statements.len());
            } else {
                tab.last_status_msg = "Executing query…".into();
            }
        }
        if statements.len() > 1 {
            self.hub.send(
                slot,
                SlotCmd::ExecuteScript {
                    tab_id,
                    statements,
                    max_rows,
                },
            );
        } else {
            self.hub.send(
                slot,
                SlotCmd::ExecuteSql {
                    tab_id,
                    sql: statements.into_iter().next().unwrap_or_default(),
                    max_rows,
                },
            );
        }
        self.persist_sessions_now();
    }
}
