use super::params_modal::ParamModalState;
use super::query_tab::{QueryTab, MAX_APPENDED_ROWS};
use super::{CottonDataApp, PendingDiscardAction};
use crate::worker::SlotCmd;
use cottondata_core::{ServerSlotId, SqlDialect, DEFAULT_PAGE_SIZE, UNLIMITED_ROWS};
use cottondata_grid::{format_keyset_literal, KeysetSeek, SortDirection};
use cottondata_sql::{
    build_paginated_query, build_sorted_query, can_auto_paginate, extract_params_for_statements,
    has_explicit_row_limit, query_param_fingerprint, split_statements, statement_at_offset,
};

impl CottonDataApp {
    pub(super) fn single_pk_for_table(&self, slot: ServerSlotId, schema: &str, table: &str) -> Option<String> {
        let catalog = self.catalogs[slot.index()].as_ref()?;
        let pks = catalog.primary_keys_for(schema, table);
        (pks.len() == 1).then(|| pks[0].clone())
    }

    pub(super) fn effective_tab_limit(&self, tab_idx: usize) -> u32 {
        let Some(tab) = self.tabs.get(tab_idx) else {
            return DEFAULT_PAGE_SIZE;
        };
        if let Some(limit) = tab.row_limit {
            return limit;
        }
        tab.slot
            .or(self.active_slot)
            .and_then(|s| self.registry.get(s))
            .map(|sl| sl.profile.page_size)
            .unwrap_or(DEFAULT_PAGE_SIZE)
    }

    pub(super) fn effective_tab_batch_size(&self, tab_idx: usize) -> u32 {
        let Some(tab) = self.tabs.get(tab_idx) else {
            return DEFAULT_PAGE_SIZE;
        };
        if tab.bound_table.is_some() {
            return tab.filter_sort.page_size.max(20);
        }
        let limit = self.effective_tab_limit(tab_idx);
        if limit == UNLIMITED_ROWS {
            tab.loaded_batch_size.max(DEFAULT_PAGE_SIZE)
        } else {
            limit.max(20)
        }
    }

    pub(super) fn can_load_more_for_tab(&self, tab_idx: usize) -> bool {
        let Some(tab) = self.tabs.get(tab_idx) else {
            return false;
        };
        let Some(page) = &tab.page else {
            return false;
        };
        if !page.has_more_rows || page.row_count >= MAX_APPENDED_ROWS {
            return false;
        }
        if tab.bound_table.is_some() {
            return true;
        }
        let dialect = self.slot_dialect(tab.slot.or(self.active_slot));
        if let (Some((_, col_name, dir)), Some(base_sql)) = (&tab.console_sort, &tab.console_base_sql) {
            if tab.console_sorted_indices.is_none()
                && build_sorted_query(dialect, base_sql, col_name, *dir == SortDirection::Desc, 1, 0).is_some()
            {
                return true;
            }
        }
        can_auto_paginate(dialect, &page.executed_sql)
    }

    pub(super) fn build_active_tab_keyset_seek(&self, dialect: SqlDialect) -> Option<KeysetSeek> {
        let tab = self.tabs.get(self.active_tab_idx)?;
        let (pk_col, direction) = tab.loaded_keyset_order.as_ref()?;
        let page = tab.page.as_ref()?;
        if page.row_count == 0 {
            return None;
        }
        let col_idx = page.columns.iter().position(|col| {
            col.sql_column_name().eq_ignore_ascii_case(pk_col) || col.name.eq_ignore_ascii_case(pk_col)
        })?;
        let last_row = page.row_count - 1;
        if page.is_null(last_row, col_idx) || page.cell_truncated_bytes(last_row, col_idx).is_some() {
            return None;
        }
        let raw = page.cell_str(last_row, col_idx)?;
        let last_value_sql = format_keyset_literal(dialect, &page.columns[col_idx], raw)?;
        Some(KeysetSeek {
            column: pk_col.clone(),
            direction: *direction,
            last_value_sql,
        })
    }

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
        let page_size = self
            .registry
            .get(slot)
            .map(|s| s.profile.page_size)
            .unwrap_or(DEFAULT_PAGE_SIZE);
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let dialect = self.slot_dialect(Some(slot));
        let single_pk = self.single_pk_for_table(slot, &schema, &table);
        let mut tab = QueryTab::new_table(id, slot, dialect, schema.clone(), table.clone(), page_size);
        if !where_clause.trim().is_empty() {
            tab.filter_sort.where_clause = where_clause;
        }
        tab.loaded_keyset_order = single_pk.clone().map(|pk| (pk, SortDirection::Asc));
        tab.sql = tab.filter_sort.build_select_sql_with_default_order(
            dialect,
            &schema,
            &table,
            single_pk.as_deref(),
        );
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
                default_order_column: single_pk,
                keyset_seek: None,
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

    pub(super) fn request_rerun_active_query_unlimited(&mut self) {
        if self.tabs[self.active_tab_idx].has_any_unsaved_changes() {
            self.unsaved_guard_modal = Some(PendingDiscardAction::RunQueryUnlimited);
            return;
        }
        self.rerun_active_query_unlimited();
    }

    pub(super) fn rerun_active_query_unlimited(&mut self) {
        let Some(tab) = self.tabs.get(self.active_tab_idx) else {
            return;
        };
        if tab.is_running || tab.is_loading_more {
            return;
        }
        let Some(slot) = tab.slot.or(self.active_slot) else {
            self.notify_error("Add and connect a server first.");
            return;
        };
        let Some(sql) = tab
            .page
            .as_ref()
            .map(|page| page.executed_sql.clone())
            .filter(|sql| !sql.trim().is_empty())
        else {
            return;
        };
        let tab_id = tab.id;
        self.clear_toast();
        self.dispatch_statements(slot, tab_id, vec![sql], UNLIMITED_ROWS);
    }

    pub(super) fn request_load_more_active_tab(&mut self) {
        let tab_idx = self.active_tab_idx;
        let Some(tab) = self.tabs.get(tab_idx) else {
            return;
        };
        if tab.is_running || tab.is_loading_more || !tab.mutator.inserted_rows().is_empty() {
            return;
        }
        let Some(slot) = tab.slot.or(self.active_slot) else {
            return;
        };
        let Some(page) = &tab.page else {
            return;
        };
        if !page.has_more_rows || page.row_count >= MAX_APPENDED_ROWS {
            return;
        }

        let dialect = self.slot_dialect(Some(slot));
        let remaining_cap = MAX_APPENDED_ROWS.saturating_sub(page.row_count);
        let batch_size = self
            .effective_tab_batch_size(tab_idx)
            .min(remaining_cap.min(u32::MAX as usize) as u32)
            .max(1);
        let fetch_limit = batch_size.saturating_add(1);

        let next_sql = if let Some((schema, table)) = &tab.bound_table {
            if let Some(seek) = self.build_active_tab_keyset_seek(dialect) {
                tab.filter_sort
                    .build_keyset_select_sql(dialect, schema, table, &seek, fetch_limit)
            } else {
                let offset = tab.filter_sort.offset().saturating_add(page.row_count as u64);
                let default_order = tab.loaded_keyset_order.as_ref().map(|(col, _)| col.as_str());
                tab.filter_sort
                    .build_offset_select_sql(dialect, schema, table, default_order, fetch_limit, offset)
            }
        } else if let (Some((_, col_name, dir)), Some(base_sql)) = (&tab.console_sort, &tab.console_base_sql) {
            if tab.console_sorted_indices.is_none() {
                match build_sorted_query(
                    dialect,
                    base_sql,
                    col_name,
                    *dir == SortDirection::Desc,
                    fetch_limit,
                    page.row_count as u64,
                ) {
                    Some(sql) => sql,
                    None => return,
                }
            } else {
                match build_paginated_query(dialect, &page.executed_sql, fetch_limit, page.row_count as u64) {
                    Some(sql) => sql,
                    None => return,
                }
            }
        } else {
            match build_paginated_query(dialect, &page.executed_sql, fetch_limit, page.row_count as u64) {
                Some(sql) => sql,
                None => return,
            }
        };

        let tab = &mut self.tabs[tab_idx];
        tab.is_loading_more = true;
        tab.scroll_fetch_failed = false;
        let tab_id = tab.id;
        self.hub.send(
            slot,
            SlotCmd::FetchMoreRows {
                tab_id,
                sql: next_sql,
                max_rows: batch_size,
            },
        );
    }

    pub(super) fn execute_active_tab(&mut self, run_all: bool) {
        let max_rows = self.effective_tab_limit(self.active_tab_idx);
        let Some(tab) = self.tabs.get_mut(self.active_tab_idx) else {
            return;
        };
        if tab.is_running || tab.is_loading_more {
            tab.last_status_msg = "A query is already running on this tab. Stop it or wait for it to finish".into();
            return;
        }
        let slot = tab.slot.or(self.active_slot);
        let Some(slot) = slot else {
            self.notify_error("Add and connect a server first.");
            return;
        };
        tab.slot = Some(slot);
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
        let params = extract_params_for_statements(&statements, self.slot_dialect(Some(slot)));
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
            tab.is_loading_more = false;
            tab.scroll_fetch_failed = false;
            tab.loaded_keyset_order = None;
            tab.reset_console_sort();
            tab.console_base_sql = None;
            tab.loaded_batch_size = if max_rows == UNLIMITED_ROWS {
                DEFAULT_PAGE_SIZE
            } else {
                max_rows.max(20)
            };
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
            let sql = statements.into_iter().next().unwrap_or_default();
            let effective_max_rows = if max_rows != UNLIMITED_ROWS && has_explicit_row_limit(&sql) {
                UNLIMITED_ROWS
            } else {
                max_rows
            };
            self.hub.send(
                slot,
                SlotCmd::ExecuteSql {
                    tab_id,
                    sql,
                    max_rows: effective_max_rows,
                },
            );
        }
        self.persist_sessions_now();
    }
}
