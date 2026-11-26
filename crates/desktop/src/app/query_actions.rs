use super::query_tab::QueryTab;
use super::{CottonDataApp, PendingDiscardAction};
use crate::worker::SlotCmd;
use cottondata_core::ServerSlotId;
use cottondata_sql::{split_statements, statement_at_offset};

impl CottonDataApp {
    pub(super) fn open_table_tab(&mut self, slot: ServerSlotId, schema: String, table: String) {
        let existing_tab = self.tabs.iter().position(|tab| {
            tab.slot == Some(slot)
                && tab
                    .bound_table
                    .as_ref()
                    .is_some_and(|(bound_schema, bound_table)| bound_schema == &schema && bound_table == &table)
        });
        if let Some(idx) = existing_tab {
            self.activate_tab(idx);
            return;
        }
        let page_size = self.registry.get(slot).map(|s| s.profile.page_size).unwrap_or(500);
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let dialect = self.slot_dialect(Some(slot));
        let tab = QueryTab::new_table(id, slot, dialect, schema.clone(), table.clone(), page_size);
        let filter_sort = tab.filter_sort.clone();
        self.tabs.push(tab);
        self.active_tab_idx = self.tabs.len() - 1;
        self.hub.send(
            slot,
            SlotCmd::LoadTablePage {
                tab_id: id,
                schema,
                table,
                filter_sort,
            },
        );
        self.persist_sessions_now();
    }

    pub(super) fn request_execute_active_tab(&mut self, run_all: bool) {
        if self.tabs[self.active_tab_idx].mutator.has_changes() {
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
            self.toast_banner = Some(("Add and connect a server first.".into(), true));
            return;
        };
        tab.slot = Some(slot);
        let max_rows = self.registry.get(slot).map(|s| s.profile.page_size).unwrap_or(500);

        let sql_to_run = if run_all {
            let stmts = split_statements(&tab.sql);
            stmts
                .last()
                .map(|s| s.sql.to_string())
                .unwrap_or_else(|| tab.sql.trim().to_string())
        } else {
            statement_at_offset(&tab.sql, tab.cursor_byte)
                .map(|s| s.sql.to_string())
                .unwrap_or_else(|| tab.sql.trim().to_string())
        };

        if sql_to_run.trim().is_empty() {
            return;
        }
        tab.is_running = true;
        tab.last_status_msg = "Executing query…".into();
        let tab_id = tab.id;
        self.hub.send(
            slot,
            SlotCmd::ExecuteSql {
                tab_id,
                sql: sql_to_run,
                max_rows,
            },
        );
    }
}
