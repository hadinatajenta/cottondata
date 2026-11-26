use super::result_table::TableAction;
use super::row_selection::RowSelectMode;
use super::CottonDataApp;
use cottondata_grid::{export_rows, ExportFormat, SqlInsertTarget};
use eframe::egui;

const CLIPBOARD_ROW_FORMAT: ExportFormat = ExportFormat::Tsv;

impl CottonDataApp {
    pub(super) fn apply_table_action(&mut self, ctx: &egui::Context, action: TableAction) {
        let tab = &mut self.tabs[self.active_tab_idx];
        match action {
            TableAction::SelectCell { row, column, mode } => {
                tab.row_selection.select(row, mode);
                if mode == RowSelectMode::Replace {
                    tab.select_cell(row, column);
                }
            }
            TableAction::SelectRow { row, mode } => {
                tab.row_selection.select(row, mode);
                tab.selected_cell = None;
                tab.is_inspector_open = false;
            }
            TableAction::OpenInspector { row, column } => {
                tab.row_selection.select(row, RowSelectMode::Replace);
                tab.select_cell(row, column);
                tab.is_inspector_open = true;
            }
            TableAction::CopyValue { row, column } => self.copy_cell_value(ctx, row, column),
            TableAction::CopyRows(format) => self.copy_selected_rows(ctx, format),
            TableAction::SelectAll => self.select_all_rows(),
        }
    }

    pub(super) fn handle_table_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input()
            || self.modal.is_some()
            || self.sql_preview_modal.is_some()
            || self.settings_modal.is_some()
        {
            return;
        }
        let (is_select_all, is_copy, is_escape) = ctx.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                input.events.iter().any(|event| matches!(event, egui::Event::Copy)),
                input.key_pressed(egui::Key::Escape) && input.modifiers.is_none(),
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
        let dialect = self.slot_dialect(self.tabs[self.active_tab_idx].slot);
        let tab = &mut self.tabs[self.active_tab_idx];
        let Some(page) = &tab.page else {
            return;
        };
        let rows = tab.row_selection.sorted_rows_below(page.row_count);
        if rows.is_empty() {
            tab.last_status_msg = "Select one or more rows to copy".into();
            return;
        }
        let mut buffer = Vec::new();
        let target_table = tab.insert_target_table();
        let target = SqlInsertTarget {
            dialect,
            table_name: &target_table,
        };
        if let Err(err) = export_rows(page, &rows, format, &target, &mut buffer) {
            self.toast_banner = Some((format!("Copy failed: {err}"), true));
            return;
        }
        ctx.copy_text(String::from_utf8_lossy(&buffer).into_owned());
        let row_noun = if rows.len() == 1 { "row" } else { "rows" };
        tab.last_status_msg = format!("Copied {} {row_noun} as {}", rows.len(), format.short_label());
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
