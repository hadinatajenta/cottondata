use super::query_tab::{InlineCommitMove, RecordViewState};
use super::result_table::TableAction;
use super::row_selection::RowSelectMode;
use super::{CottonDataApp, PendingDiscardAction};
use crate::worker::SlotCmd;
use cottondata_core::EnvironmentTag;
use cottondata_grid::{
    export_rows, infer_result_table, resolve_editability, CellValue, Editability, ExportFormat, ReadOnlyReason,
    SortDirection, SqlInsertTarget, TableEditMetadata,
};
use cottondata_introspect::TableKind;
use eframe::egui;

const CLIPBOARD_ROW_FORMAT: ExportFormat = ExportFormat::Tsv;

impl CottonDataApp {
    pub(super) fn resolve_active_tab_editability(&self) -> Editability {
        let tab = &self.tabs[self.active_tab_idx];
        let Some(page) = &tab.page else {
            return Editability::ReadOnly(ReadOnlyReason::NoBaseTable);
        };
        let slot = tab.slot.or(self.active_slot);
        let server_read_only = slot
            .and_then(|s| self.registry.get(s))
            .map(|sl| sl.profile.read_only)
            .unwrap_or(false);
        let fallback_schema = slot
            .and_then(|s| self.registry.get(s))
            .and_then(|sl| sl.runtime.as_ref())
            .and_then(|rt| rt.current_schema.as_deref());
        let bound_table = tab.bound_table.as_ref().map(|(s, t)| (s.as_str(), t.as_str()));

        let inferred = infer_result_table(page, bound_table, fallback_schema).ok();
        let (is_view, pks, unique_key) = match (slot, inferred) {
            (Some(slot_id), Some((schema, table))) => {
                if let Some(catalog) = &self.catalogs[slot_id.index()] {
                    (
                        catalog.table_kind(&schema, &table) == Some(TableKind::View),
                        catalog.primary_keys_for(&schema, &table),
                        catalog.unique_key_columns_for(&schema, &table),
                    )
                } else {
                    (false, Vec::new(), Vec::new())
                }
            }
            _ => (false, Vec::new(), Vec::new()),
        };

        let catalog_meta = Some(TableEditMetadata {
            is_view,
            primary_keys: &pks,
            unique_key: &unique_key,
        });
        resolve_editability(page, bound_table, fallback_schema, server_read_only, catalog_meta)
    }

    pub(super) fn trigger_save_active_tab(&mut self, force_preview: bool) {
        self.tabs[self.active_tab_idx].commit_inline_edit(InlineCommitMove::Stay);
        let editability = self.resolve_active_tab_editability();
        let Some(target) = editability.target() else {
            if let Some(reason) = editability.reason() {
                self.toast_banner = Some((reason.message().into(), true));
            }
            return;
        };
        let active_slot = self.tabs[self.active_tab_idx].slot.or(self.active_slot);
        let Some(slot) = active_slot else {
            return;
        };
        let dialect = self.slot_dialect(Some(slot));
        let is_production = self
            .registry
            .get(slot)
            .is_some_and(|sl| sl.profile.environment == EnvironmentTag::Production);
        let tab = &self.tabs[self.active_tab_idx];
        let Some(page) = &tab.page else {
            return;
        };
        let tab_id = tab.id;
        match tab
            .mutator
            .build_pending_sql(dialect, page, &target.schema, &target.table, &target.key_columns)
        {
            Ok(batch) if !batch.is_empty() => {
                if force_preview || is_production {
                    self.sql_preview_modal = Some((tab_id, batch.as_script(), batch.statements));
                } else {
                    self.tabs[self.active_tab_idx].is_submitting_mutations = true;
                    self.hub.send(
                        slot,
                        SlotCmd::SubmitMutations {
                            tab_id,
                            statements: batch.statements,
                        },
                    );
                }
            }
            Ok(_) => {}
            Err(err) => {
                self.after_save_action = None;
                self.toast_banner = Some((err, true));
            }
        }
    }

    pub(super) fn request_load_full_cell(&mut self, row: usize, column: usize) {
        let editability = self.resolve_active_tab_editability();
        let Some(target) = editability.target() else {
            self.toast_banner = Some(("Loading full cell value requires a primary/unique key.".into(), true));
            return;
        };
        let active_slot = self.tabs[self.active_tab_idx].slot.or(self.active_slot);
        let Some(slot) = active_slot else {
            return;
        };
        let dialect = self.slot_dialect(Some(slot));
        let tab = &mut self.tabs[self.active_tab_idx];
        let Some(page) = &tab.page else {
            return;
        };
        let Some(col_meta) = page.columns.get(column) else {
            return;
        };
        let column_name = col_meta.sql_column_name().to_string();
        let pk_where = match target.build_key_where(dialect, page, row) {
            Ok(where_sql) => where_sql,
            Err(err) => {
                self.toast_banner = Some((err, true));
                return;
            }
        };
        tab.is_loading_full_cell = true;
        let tab_id = tab.id;
        self.hub.send(
            slot,
            SlotCmd::FetchFullCell {
                tab_id,
                row,
                column,
                schema: target.schema.clone(),
                table: target.table.clone(),
                column_name,
                pk_where,
            },
        );
    }

    pub(super) fn execute_pending_discard_action(&mut self, action: PendingDiscardAction) {
        match action {
            PendingDiscardAction::CloseTab(idx) => self.force_close_tab(idx),
            PendingDiscardAction::RunQuery(run_all) => {
                self.tabs[self.active_tab_idx].mutator.revert_all();
                self.tabs[self.active_tab_idx].inline_editor = None;
                self.execute_active_tab(run_all);
            }
            PendingDiscardAction::ReloadTablePage { page_index } => {
                self.tabs[self.active_tab_idx].mutator.revert_all();
                self.tabs[self.active_tab_idx].inline_editor = None;
                self.tabs[self.active_tab_idx].filter_sort.page_index = page_index;
                self.send_reload_active_table_page();
            }
            PendingDiscardAction::SortColumn(col_name) => {
                self.tabs[self.active_tab_idx].mutator.revert_all();
                self.tabs[self.active_tab_idx].inline_editor = None;
                self.apply_sort_column(col_name);
            }
        }
    }

    pub(super) fn send_reload_active_table_page(&mut self) {
        let active_slot = self.tabs[self.active_tab_idx].slot.or(self.active_slot);
        let bound_table = self.tabs[self.active_tab_idx].bound_table.clone();
        if let (Some(slot), Some((schema, table))) = (active_slot, bound_table) {
            let dialect = self.slot_dialect(Some(slot));
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.sql = tab.filter_sort.build_select_sql(dialect, &schema, &table);
            tab.is_running = true;
            let tid = tab.id;
            let fs = tab.filter_sort.clone();
            self.hub.send(
                slot,
                SlotCmd::LoadTablePage {
                    tab_id: tid,
                    schema,
                    table,
                    filter_sort: fs,
                },
            );
            self.mark_session_dirty();
        }
    }

    pub(super) fn apply_sort_column(&mut self, col_name: String) {
        let active_slot = self.tabs[self.active_tab_idx].slot.or(self.active_slot);
        let bound_table = self.tabs[self.active_tab_idx].bound_table.clone();
        if let (Some(slot), Some((schema, table))) = (active_slot, bound_table) {
            let dialect = self.slot_dialect(Some(slot));
            let tab = &mut self.tabs[self.active_tab_idx];
            let next_dir = match &tab.filter_sort.order_by {
                Some((c, SortDirection::Asc)) if c == &col_name => Some(SortDirection::Desc),
                Some((c, SortDirection::Desc)) if c == &col_name => None,
                _ => Some(SortDirection::Asc),
            };
            tab.filter_sort.order_by = next_dir.map(|d| (col_name, d));
            tab.filter_sort.page_index = 0;
            tab.sql = tab.filter_sort.build_select_sql(dialect, &schema, &table);
            tab.is_running = true;
            let tid = tab.id;
            let fs = tab.filter_sort.clone();
            self.hub.send(
                slot,
                SlotCmd::LoadTablePage {
                    tab_id: tid,
                    schema,
                    table,
                    filter_sort: fs,
                },
            );
            self.mark_session_dirty();
        }
    }

    pub(super) fn apply_table_action(&mut self, ctx: &egui::Context, action: TableAction, editability: &Editability) {
        let tab = &mut self.tabs[self.active_tab_idx];
        match action {
            TableAction::SelectCell { row, column, mode } => {
                if tab.inline_editor.is_some() {
                    tab.commit_inline_edit(InlineCommitMove::Stay);
                }
                tab.row_selection.select(row, mode);
                if mode == RowSelectMode::Replace {
                    tab.select_cell(row, column);
                }
            }
            TableAction::SelectRow { row, mode } => {
                if tab.inline_editor.is_some() {
                    tab.commit_inline_edit(InlineCommitMove::Stay);
                }
                tab.row_selection.select(row, mode);
                tab.selected_cell = None;
                tab.is_inspector_open = false;
            }
            TableAction::StartInlineEdit { row, column } => {
                if editability.is_column_editable(column) {
                    tab.row_selection.select(row, RowSelectMode::Replace);
                    tab.start_inline_edit(row, column, None);
                } else {
                    tab.row_selection.select(row, RowSelectMode::Replace);
                    tab.select_cell(row, column);
                    tab.is_inspector_open = true;
                }
            }
            TableAction::CommitInlineEdit(movement) => {
                tab.commit_inline_edit(movement);
                if let Some((r, _)) = tab.selected_cell {
                    tab.row_selection.select(r, RowSelectMode::Replace);
                }
            }
            TableAction::CancelInlineEdit => {
                tab.cancel_inline_edit();
            }
            TableAction::PromoteInlineToInspector => {
                if let Some(editor) = tab.inline_editor.take() {
                    tab.selected_cell = Some((editor.row, editor.column));
                    tab.cell_edit_is_null = false;
                    tab.cell_edit_is_default = false;
                    tab.cell_edit_buf = editor.buffer;
                    tab.is_inspector_open = true;
                }
            }
            TableAction::SetCellNull { row, column } => {
                if editability.is_column_editable(column) {
                    tab.cancel_inline_edit();
                    tab.stage_cell_value(row, column, CellValue::Null);
                }
            }
            TableAction::SetCellDefault { row, column } => {
                if editability.is_column_editable(column) {
                    tab.cancel_inline_edit();
                    tab.stage_cell_value(row, column, CellValue::DefaultExpr);
                }
            }
            TableAction::RevertCell { row, column } => {
                tab.cancel_inline_edit();
                if let Some(page) = &tab.page {
                    if row < page.row_count {
                        tab.mutator.revert_cell(row, column);
                        tab.select_cell(row, column);
                    }
                }
            }
            TableAction::RevertRow { row } => {
                tab.cancel_inline_edit();
                if let Some(page) = &tab.page {
                    if row < page.row_count {
                        tab.mutator.revert_row(row);
                        if let Some((sel_r, sel_c)) = tab.selected_cell {
                            if sel_r == row {
                                tab.select_cell(sel_r, sel_c);
                            }
                        }
                    }
                }
            }
            TableAction::CloneRow { row } => {
                if editability.is_editable() {
                    if let Some(page) = &tab.page {
                        if row < page.row_count {
                            let new_inserted_idx = tab.mutator.clone_row(page, row);
                            let new_row = page.row_count + new_inserted_idx;
                            tab.row_selection.select(new_row, RowSelectMode::Replace);
                            tab.select_cell(new_row, 0);
                        }
                    }
                }
            }
            TableAction::ToggleDeleteRow { row } => {
                if editability.is_editable() {
                    if let Some(page) = &tab.page {
                        if row < page.row_count {
                            tab.mutator.toggle_delete_row(row);
                        } else {
                            tab.mutator.remove_inserted_row(row - page.row_count);
                            tab.selected_cell = None;
                        }
                    }
                }
            }
            TableAction::OpenInspector { row, column } => {
                tab.cancel_inline_edit();
                tab.row_selection.select(row, RowSelectMode::Replace);
                tab.select_cell(row, column);
                tab.record_view = None;
                tab.is_inspector_open = true;
            }
            TableAction::OpenRecordView { row } => {
                tab.cancel_inline_edit();
                tab.row_selection.select(row, RowSelectMode::Replace);
                tab.select_cell(row, 0);
                tab.is_inspector_open = false;
                let (filter, hide_nulls, pretty_json) = tab
                    .record_view
                    .as_ref()
                    .map(|rv| (rv.filter.clone(), rv.hide_nulls, rv.pretty_json))
                    .unwrap_or_else(|| (String::new(), false, true));
                tab.record_view = Some(RecordViewState {
                    row,
                    filter,
                    hide_nulls,
                    pretty_json,
                });
            }
            TableAction::CompareSelectedRows(row_a, row_b) => {
                tab.diff_rows = Some((row_a, row_b, true));
            }
            TableAction::QuickFilterValue { row, column, negate } => {
                self.apply_quick_filter_by_cell(row, column, negate);
            }
            TableAction::CopyColumnAsIn { row, column } => {
                self.copy_column_as_in_clause(ctx, row, column);
            }
            TableAction::CopyValue { row, column } => self.copy_cell_value(ctx, row, column),
            TableAction::CopyRows(format) => self.copy_selected_rows(ctx, format),
            TableAction::SelectAll => self.select_all_rows(),
        }
    }

    fn apply_quick_filter_by_cell(&mut self, row: usize, column: usize, negate: bool) {
        let dialect = self.slot_dialect(self.tabs[self.active_tab_idx].slot.or(self.active_slot));
        let predicate = {
            let tab = &self.tabs[self.active_tab_idx];
            let Some(page) = &tab.page else {
                return;
            };
            let Some(col) = page.columns.get(column) else {
                return;
            };
            let col_ident = dialect.quote_ident(col.sql_column_name());
            let val = tab.cell_value(row, column).unwrap_or(CellValue::Null);
            if val.is_null() {
                if negate {
                    format!("{col_ident} IS NOT NULL")
                } else {
                    format!("{col_ident} IS NULL")
                }
            } else {
                let op = if negate { "!=" } else { "=" };
                let literal = cottondata_grid::mutator::format_sql_literal(dialect, &val, col.is_numeric);
                format!("{col_ident} {op} {literal}")
            }
        };
        let tab = &mut self.tabs[self.active_tab_idx];
        let existing = tab.filter_sort.where_clause.trim();
        tab.filter_sort.where_clause = if existing.is_empty() {
            predicate
        } else {
            format!("({existing}) AND {predicate}")
        };
        if tab.mutator.has_changes() {
            self.unsaved_guard_modal = Some(PendingDiscardAction::ReloadTablePage { page_index: 0 });
        } else {
            tab.filter_sort.page_index = 0;
            self.send_reload_active_table_page();
        }
    }

    fn copy_column_as_in_clause(&mut self, ctx: &egui::Context, row: usize, column: usize) {
        let dialect = self.slot_dialect(self.tabs[self.active_tab_idx].slot.or(self.active_slot));
        let tab = &mut self.tabs[self.active_tab_idx];
        let Some(page) = &tab.page else {
            return;
        };
        let Some(col) = page.columns.get(column) else {
            return;
        };
        let mut target_rows = tab.row_selection.sorted_rows_below(page.row_count);
        if target_rows.is_empty() || !target_rows.contains(&row) {
            target_rows = vec![row];
        }
        let mut literals = Vec::with_capacity(target_rows.len());
        for r in target_rows {
            if let Some(val) = tab.cell_value(r, column) {
                if !val.is_null() {
                    literals.push(cottondata_grid::mutator::format_sql_literal(
                        dialect,
                        &val,
                        col.is_numeric,
                    ));
                }
            }
        }
        let formatted = format!("({})", literals.join(", "));
        ctx.copy_text(formatted);
        tab.last_status_msg = format!("Copied {} value(s) of {} as IN (…)", literals.len(), col.name);
    }

    pub(super) fn handle_table_shortcuts(&mut self, ctx: &egui::Context, editability: &Editability) {
        let modal_open = self.modal.is_some()
            || self.session_modal.is_some()
            || self.settings_modal.is_some()
            || self.sql_preview_modal.is_some()
            || self.unsaved_guard_modal.is_some();
        if modal_open {
            return;
        }

        let has_staged_or_inline = self.tabs[self.active_tab_idx].mutator.has_changes()
            || self.tabs[self.active_tab_idx].inline_editor.is_some();
        if has_staged_or_inline
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S))
        {
            self.trigger_save_active_tab(false);
            return;
        }

        if ctx.wants_keyboard_input() {
            return;
        }

        let (
            is_select_all,
            is_copy,
            is_escape,
            arrow_delta,
            is_enter,
            is_shift_enter,
            is_f2,
            is_set_null,
            is_revert_cell,
            is_clone_row,
            typed_text,
        ) = ctx.input_mut(|input| {
            let is_select_all = input.consume_key(egui::Modifiers::COMMAND, egui::Key::A);
            let is_copy = input.events.iter().any(|event| matches!(event, egui::Event::Copy));
            let is_escape = input.key_pressed(egui::Key::Escape) && input.modifiers.is_none();
            let arrow_delta = if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                Some((-1isize, 0isize))
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                Some((1, 0))
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                Some((0, -1))
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                Some((0, 1))
            } else {
                None
            };
            let is_shift_enter = input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter);
            let is_enter = input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            let is_f2 = input.consume_key(egui::Modifiers::NONE, egui::Key::F2);
            let is_set_null = input.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::ALT, egui::Key::N);
            let is_revert_cell = input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
            let is_clone_row = input.consume_key(egui::Modifiers::COMMAND, egui::Key::D);

            let typed_text = if !input.modifiers.command && !input.modifiers.ctrl && !input.modifiers.alt {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Text(text) if !text.is_empty() && text.chars().all(|c| !c.is_control()) => {
                        Some(text.clone())
                    }
                    _ => None,
                })
            } else {
                None
            };

            (
                is_select_all,
                is_copy,
                is_escape,
                arrow_delta,
                is_enter,
                is_shift_enter,
                is_f2,
                is_set_null,
                is_revert_cell,
                is_clone_row,
                typed_text,
            )
        });

        if is_select_all {
            self.select_all_rows();
        }
        if is_copy && self.has_selected_rows() {
            self.copy_selected_rows(ctx, CLIPBOARD_ROW_FORMAT);
        }
        if is_escape {
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.row_selection.clear();
            tab.is_inspector_open = false;
        }

        if let Some((dr, dc)) = arrow_delta {
            let tab = &mut self.tabs[self.active_tab_idx];
            let total_rows = tab.total_grid_rows();
            let col_count = tab.page.as_ref().map(|p| p.columns.len()).unwrap_or(0);
            if total_rows > 0 && col_count > 0 {
                let (cur_r, cur_c) = tab.selected_cell.unwrap_or((0, 0));
                let next_r = (cur_r as isize + dr).clamp(0, (total_rows - 1) as isize) as usize;
                let next_c = (cur_c as isize + dc).clamp(0, (col_count - 1) as isize) as usize;
                tab.row_selection.select(next_r, RowSelectMode::Replace);
                tab.select_cell(next_r, next_c);
            }
            return;
        }

        let Some((row, col)) = self.tabs[self.active_tab_idx].selected_cell else {
            return;
        };

        if is_shift_enter {
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.select_cell(row, col);
            tab.is_inspector_open = true;
            return;
        }

        if is_enter || is_f2 {
            let tab = &mut self.tabs[self.active_tab_idx];
            if editability.is_column_editable(col) {
                tab.start_inline_edit(row, col, None);
            } else {
                tab.is_inspector_open = true;
            }
            return;
        }

        if is_set_null && editability.is_column_editable(col) {
            let is_nullable = self.tabs[self.active_tab_idx]
                .page
                .as_ref()
                .and_then(|p| p.columns.get(col))
                .map(|c| c.is_nullable)
                .unwrap_or(true);
            if is_nullable {
                self.tabs[self.active_tab_idx].stage_cell_value(row, col, CellValue::Null);
            } else {
                self.toast_banner = Some(("This column does not allow NULL values.".into(), true));
            }
            return;
        }

        if is_revert_cell && editability.is_editable() {
            let tab = &mut self.tabs[self.active_tab_idx];
            if let Some(page) = &tab.page {
                if row < page.row_count {
                    tab.mutator.revert_cell(row, col);
                    tab.select_cell(row, col);
                }
            }
            return;
        }

        if is_clone_row && editability.is_editable() {
            let tab = &mut self.tabs[self.active_tab_idx];
            if let Some(page) = &tab.page {
                if row < page.row_count {
                    let ins = tab.mutator.clone_row(page, row);
                    let new_row = page.row_count + ins;
                    tab.row_selection.select(new_row, RowSelectMode::Replace);
                    tab.select_cell(new_row, col);
                }
            }
            return;
        }

        if let Some(text) = typed_text {
            if editability.is_column_editable(col) {
                self.tabs[self.active_tab_idx].start_inline_edit(row, col, Some(text));
            }
        }
    }

    fn has_selected_rows(&self) -> bool {
        let tab = &self.tabs[self.active_tab_idx];
        tab.page
            .as_ref()
            .is_some_and(|page| !tab.row_selection.sorted_rows_below(page.row_count).is_empty())
    }

    fn select_all_rows(&mut self) {
        let tab = &mut self.tabs[self.active_tab_idx];
        if let Some(page) = &tab.page {
            tab.row_selection.select_all(page.row_count);
            tab.last_status_msg = format!("Selected {} rows", page.row_count);
        }
    }

    fn copy_selected_rows(&mut self, ctx: &egui::Context, format: ExportFormat) {
        let tab = &self.tabs[self.active_tab_idx];
        let Some(page) = &tab.page else {
            return;
        };
        let rows = tab.row_selection.sorted_rows_below(page.row_count);
        if rows.is_empty() {
            self.tabs[self.active_tab_idx].last_status_msg = "Select one or more rows to copy".into();
            return;
        }
        let target_table = tab.insert_target_table();
        let target = SqlInsertTarget {
            dialect: self.slot_dialect(tab.slot),
            table_name: &target_table,
        };
        let mut buffer = Vec::new();
        if let Err(err) = export_rows(page, &rows, format, &target, &mut buffer) {
            self.toast_banner = Some((format!("Copy failed: {err}"), true));
            return;
        }
        ctx.copy_text(String::from_utf8_lossy(&buffer).into_owned());
        let row_noun = if rows.len() == 1 { "row" } else { "rows" };
        self.tabs[self.active_tab_idx].last_status_msg =
            format!("Copied {} {row_noun} as {}", rows.len(), format.short_label());
    }

    fn copy_cell_value(&mut self, ctx: &egui::Context, row: usize, column: usize) {
        let tab = &mut self.tabs[self.active_tab_idx];
        let Some(column_name) = tab
            .page
            .as_ref()
            .and_then(|page| page.columns.get(column))
            .map(|meta| meta.name.clone())
        else {
            return;
        };
        let Some(value) = tab.cell_value(row, column) else {
            return;
        };
        let text = if value.is_null() {
            "NULL"
        } else {
            value.as_display_str()
        };
        ctx.copy_text(text.to_string());
        tab.last_status_msg = format!("Copied value of {column_name}");
    }
}
