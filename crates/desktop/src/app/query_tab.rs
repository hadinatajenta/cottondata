use super::row_selection::RowSelection;
use cottondata_core::{ServerSlotId, SqlDialect};
use cottondata_grid::{CellValue, ColumnarPage, PageFilterSort, SortDirection, StagedMutator};
use cottondata_driver::ExplainResult;
use cottondata_storage::{SessionSortOrder, SessionTabState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BottomDeckTab {
    DataGrid,
    DdlInspector,
    ExplainPlan,
    Monitor,
    History,
    Snippets,
}

#[derive(Clone, Default)]
pub(super) struct FindReplaceState {
    pub(super) open: bool,
    pub(super) show_replace: bool,
    pub(super) query: String,
    pub(super) replace_with: String,
    pub(super) case_sensitive: bool,
    pub(super) whole_word: bool,
    pub(super) active_match: usize,
    pub(super) focus_find: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InlineCommitMove {
    Stay,
    Down,
    Right,
    Left,
}

#[derive(Clone)]
pub(super) struct InlineCellEditor {
    pub(super) row: usize,
    pub(super) column: usize,
    pub(super) buffer: String,
    pub(super) needs_focus: bool,
    pub(super) select_all: bool,
}

#[derive(Clone)]
pub(super) struct RecordViewState {
    pub(super) row: usize,
    pub(super) filter: String,
    pub(super) hide_nulls: bool,
    pub(super) pretty_json: bool,
}

#[derive(Clone)]
pub(super) struct StoredResultSet {
    pub(super) label: String,
    pub(super) page: ColumnarPage,
    pub(super) status_msg: String,
    pub(super) mutator: StagedMutator,
    pub(super) column_widths: Vec<f32>,
    pub(super) selected_cell: Option<(usize, usize)>,
    pub(super) row_selection: RowSelection,
}

#[derive(Clone)]
pub(super) struct QueryTab {
    pub(super) id: u64,
    pub(super) title: String,
    pub(super) slot: Option<ServerSlotId>,
    pub(super) sql: String,
    pub(super) cursor_byte: usize,
    pub(super) selection_byte_range: Option<(usize, usize)>,
    pub(super) bound_table: Option<(String, String)>,
    pub(super) filter_sort: PageFilterSort,
    pub(super) page: Option<ColumnarPage>,
    pub(super) result_sets: Vec<StoredResultSet>,
    pub(super) active_result_idx: usize,
    pub(super) last_status_msg: String,
    pub(super) is_running: bool,
    pub(super) mutator: StagedMutator,
    pub(super) selected_cell: Option<(usize, usize)>,
    pub(super) inline_editor: Option<InlineCellEditor>,
    pub(super) cell_edit_buf: String,
    pub(super) cell_edit_is_null: bool,
    pub(super) cell_edit_is_default: bool,
    pub(super) explain: Option<ExplainResult>,
    pub(super) column_widths: Vec<f32>,
    pub(super) row_selection: RowSelection,
    pub(super) is_inspector_open: bool,
    pub(super) record_view: Option<RecordViewState>,
    pub(super) diff_rows: Option<(usize, usize, bool)>,
    pub(super) is_counting_rows: bool,
    pub(super) is_submitting_mutations: bool,
    pub(super) is_loading_full_cell: bool,
    pub(super) show_sql_editor: bool,
    pub(super) find_replace: FindReplaceState,
    pub(super) pending_cursor_range: Option<(usize, usize)>,
}

impl QueryTab {
    pub(super) fn new_console(id: u64, slot: Option<ServerSlotId>) -> Self {
        Self {
            id,
            title: format!("Console {id}"),
            slot,
            sql: "SELECT VERSION() AS mysql_version, DATABASE() AS current_db, NOW() AS server_time;".to_string(),
            cursor_byte: 0,
            selection_byte_range: None,
            bound_table: None,
            filter_sort: PageFilterSort {
                page_size: 500,
                ..Default::default()
            },
            page: None,
            result_sets: Vec::new(),
            active_result_idx: 0,
            last_status_msg: "Ready. Press Cmd/Ctrl+Enter to run, Cmd/Ctrl+Shift+Enter to run all".into(),
            is_running: false,
            mutator: StagedMutator::new(),
            selected_cell: None,
            inline_editor: None,
            cell_edit_buf: String::new(),
            cell_edit_is_null: false,
            cell_edit_is_default: false,
            explain: None,
            column_widths: Vec::new(),
            row_selection: RowSelection::default(),
            is_inspector_open: false,
            record_view: None,
            diff_rows: None,
            is_counting_rows: false,
            is_submitting_mutations: false,
            is_loading_full_cell: false,
            show_sql_editor: true,
            find_replace: FindReplaceState::default(),
            pending_cursor_range: None,
        }
    }

    pub(super) fn new_table(
        id: u64,
        slot: ServerSlotId,
        dialect: SqlDialect,
        schema: String,
        table: String,
        page_size: u32,
    ) -> Self {
        let filter_sort = PageFilterSort {
            page_size,
            ..Default::default()
        };
        let sql = filter_sort.build_select_sql(dialect, &schema, &table);
        Self {
            id,
            title: format!("{schema}.{table}"),
            slot: Some(slot),
            sql,
            cursor_byte: 0,
            selection_byte_range: None,
            bound_table: Some((schema, table)),
            filter_sort,
            page: None,
            result_sets: Vec::new(),
            active_result_idx: 0,
            last_status_msg: "Loading table page…".into(),
            is_running: true,
            mutator: StagedMutator::new(),
            selected_cell: None,
            inline_editor: None,
            cell_edit_buf: String::new(),
            cell_edit_is_null: false,
            cell_edit_is_default: false,
            explain: None,
            column_widths: Vec::new(),
            row_selection: RowSelection::default(),
            is_inspector_open: false,
            record_view: None,
            diff_rows: None,
            is_counting_rows: false,
            is_submitting_mutations: false,
            is_loading_full_cell: false,
            show_sql_editor: false,
            find_replace: FindReplaceState::default(),
            pending_cursor_range: None,
        }
    }

    pub(super) fn from_session_state(state: &SessionTabState, slot: Option<ServerSlotId>) -> Self {
        let mut cursor_byte = state.cursor_byte.min(state.sql.len());
        while cursor_byte > 0 && !state.sql.is_char_boundary(cursor_byte) {
            cursor_byte -= 1;
        }
        let order_by = state.order_by.as_ref().map(|(col, dir)| {
            let mapped = match dir {
                SessionSortOrder::Asc => SortDirection::Asc,
                SessionSortOrder::Desc => SortDirection::Desc,
            };
            (col.clone(), mapped)
        });
        let filter_sort = PageFilterSort {
            where_clause: state.where_clause.clone(),
            order_by,
            page_index: state.page_index,
            page_size: state.page_size.max(50),
        };
        let last_status_msg = if state.bound_table.is_some() {
            "Restored from session. Reconnecting to reload table…".into()
        } else {
            "Restored from session. Press Cmd/Ctrl+Enter to run".into()
        };
        Self {
            id: state.id,
            title: state.title.clone(),
            slot,
            sql: state.sql.clone(),
            cursor_byte,
            selection_byte_range: None,
            bound_table: state.bound_table.clone(),
            filter_sort,
            page: None,
            result_sets: Vec::new(),
            active_result_idx: 0,
            last_status_msg,
            is_running: false,
            mutator: StagedMutator::new(),
            selected_cell: None,
            inline_editor: None,
            cell_edit_buf: String::new(),
            cell_edit_is_null: false,
            cell_edit_is_default: false,
            explain: None,
            column_widths: Vec::new(),
            row_selection: RowSelection::default(),
            is_inspector_open: false,
            record_view: None,
            diff_rows: None,
            is_counting_rows: false,
            is_submitting_mutations: false,
            is_loading_full_cell: false,
            show_sql_editor: state.show_sql_editor.unwrap_or_else(|| state.bound_table.is_none()),
            find_replace: FindReplaceState::default(),
            pending_cursor_range: None,
        }
    }

    pub(super) fn to_session_state(&self, server_id: Option<String>) -> SessionTabState {
        let order_by = self.filter_sort.order_by.as_ref().map(|(col, dir)| {
            let mapped = match dir {
                SortDirection::Asc => SessionSortOrder::Asc,
                SortDirection::Desc => SessionSortOrder::Desc,
            };
            (col.clone(), mapped)
        });
        SessionTabState {
            id: self.id,
            title: self.title.clone(),
            server_id,
            sql: self.sql.clone(),
            cursor_byte: self.cursor_byte,
            bound_table: self.bound_table.clone(),
            where_clause: self.filter_sort.where_clause.clone(),
            order_by,
            page_index: self.filter_sort.page_index,
            page_size: self.filter_sort.page_size,
            show_sql_editor: Some(self.show_sql_editor),
        }
    }

    pub(super) fn insert_target_table(&self) -> String {
        self.bound_table
            .as_ref()
            .map(|(_, table)| table.clone())
            .or_else(|| {
                self.page
                    .as_ref()
                    .and_then(|page| page.columns.iter().find_map(|column| column.table.clone()))
                    .filter(|table| !table.is_empty())
            })
            .unwrap_or_else(|| "exported_rows".into())
    }

    pub(super) fn cell_value(&self, row: usize, column: usize) -> Option<CellValue> {
        let page = self.page.as_ref()?;
        if row >= page.row_count {
            return self
                .mutator
                .inserted_rows()
                .get(row - page.row_count)?
                .get(column)
                .cloned();
        }
        Some(
            self.mutator
                .get_modified(row, column)
                .cloned()
                .unwrap_or_else(|| page.cell_value(row, column)),
        )
    }

    pub(super) fn select_cell(&mut self, row: usize, column: usize) {
        let Some(current_value) = self.cell_value(row, column) else {
            return;
        };
        self.selected_cell = Some((row, column));
        self.cell_edit_is_null = current_value.is_null();
        self.cell_edit_is_default = current_value.is_default();
        self.cell_edit_buf = if current_value.is_null() || current_value.is_default() {
            String::new()
        } else {
            current_value.as_display_str().to_string()
        };
    }

    pub(super) fn total_grid_rows(&self) -> usize {
        self.page
            .as_ref()
            .map(|page| page.row_count + self.mutator.inserted_rows().len())
            .unwrap_or(0)
    }

    pub(super) fn stage_cell_value(&mut self, row: usize, column: usize, new_val: CellValue) {
        let Some(page) = &self.page else {
            return;
        };
        if row >= page.row_count {
            self.mutator.set_inserted_cell(row - page.row_count, column, new_val);
        } else {
            self.mutator.set_cell(page, row, column, new_val);
        }
        if self.selected_cell == Some((row, column)) {
            self.select_cell(row, column);
        }
    }

    pub(super) fn start_inline_edit(&mut self, row: usize, column: usize, initial_text: Option<String>) {
        let Some(current_value) = self.cell_value(row, column) else {
            return;
        };
        self.select_cell(row, column);
        if initial_text.is_none()
            && (current_value.is_truncated()
                || (!current_value.is_null() && current_value.as_display_str().contains('\n')))
        {
            self.inline_editor = None;
            self.is_inspector_open = true;
            return;
        }
        let (buffer, select_all) = match initial_text {
            Some(typed) => (typed, false),
            None => {
                let text = if current_value.is_null() || current_value.is_default() {
                    String::new()
                } else {
                    current_value.as_display_str().to_string()
                };
                (text, true)
            }
        };
        self.inline_editor = Some(InlineCellEditor {
            row,
            column,
            buffer,
            needs_focus: true,
            select_all,
        });
    }

    pub(super) fn commit_inline_edit(&mut self, movement: InlineCommitMove) {
        let Some(editor) = self.inline_editor.take() else {
            return;
        };
        let original = self.cell_value(editor.row, editor.column);
        let unchanged_special = matches!(original, Some(CellValue::Null | CellValue::DefaultExpr))
            && editor.buffer.is_empty();
        if !unchanged_special {
            self.stage_cell_value(editor.row, editor.column, CellValue::Text(editor.buffer));
        }

        let total_rows = self.total_grid_rows();
        let col_count = self.page.as_ref().map(|p| p.columns.len()).unwrap_or(0);
        if total_rows == 0 || col_count == 0 {
            return;
        }
        let (next_row, next_col) = match movement {
            InlineCommitMove::Stay => (editor.row, editor.column),
            InlineCommitMove::Down => ((editor.row + 1).min(total_rows - 1), editor.column),
            InlineCommitMove::Right => {
                if editor.column + 1 < col_count {
                    (editor.row, editor.column + 1)
                } else if editor.row + 1 < total_rows {
                    (editor.row + 1, 0)
                } else {
                    (editor.row, editor.column)
                }
            }
            InlineCommitMove::Left => {
                if editor.column > 0 {
                    (editor.row, editor.column - 1)
                } else if editor.row > 0 {
                    (editor.row - 1, col_count - 1)
                } else {
                    (editor.row, editor.column)
                }
            }
        };
        self.select_cell(next_row, next_col);
    }

    pub(super) fn cancel_inline_edit(&mut self) {
        if let Some(editor) = self.inline_editor.take() {
            self.select_cell(editor.row, editor.column);
        }
    }

    pub(super) fn save_active_result_snapshot(&mut self) {
        if let (Some(slot), Some(page)) = (self.result_sets.get_mut(self.active_result_idx), self.page.as_ref()) {
            slot.page = page.clone();
            slot.status_msg = self.last_status_msg.clone();
            slot.mutator = self.mutator.clone();
            slot.column_widths = self.column_widths.clone();
            slot.selected_cell = self.selected_cell;
            slot.row_selection = self.row_selection.clone();
        }
    }

    pub(super) fn set_result_sets(&mut self, sets: Vec<StoredResultSet>) {
        self.result_sets = sets;
        self.active_result_idx = 0;
        self.inline_editor = None;
        self.record_view = None;
        self.diff_rows = None;
        if let Some(first) = self.result_sets.first() {
            self.page = Some(first.page.clone());
            self.last_status_msg = first.status_msg.clone();
            self.mutator = first.mutator.clone();
            self.column_widths = first.column_widths.clone();
            self.selected_cell = first.selected_cell;
            self.row_selection = first.row_selection.clone();
        }
    }

    pub(super) fn switch_result_set(&mut self, idx: usize) {
        if idx >= self.result_sets.len() || idx == self.active_result_idx {
            return;
        }
        self.cancel_inline_edit();
        self.save_active_result_snapshot();
        self.active_result_idx = idx;
        let next = &self.result_sets[idx];
        self.page = Some(next.page.clone());
        self.last_status_msg = next.status_msg.clone();
        self.mutator = next.mutator.clone();
        self.column_widths = next.column_widths.clone();
        self.selected_cell = next.selected_cell;
        self.row_selection = next.row_selection.clone();
        self.record_view = None;
        self.diff_rows = None;
    }

    pub(super) fn close_result_set(&mut self, idx: usize) {
        if idx >= self.result_sets.len() {
            return;
        }
        self.cancel_inline_edit();
        self.save_active_result_snapshot();
        self.result_sets.remove(idx);
        if self.result_sets.is_empty() {
            self.active_result_idx = 0;
            self.page = None;
            self.mutator.revert_all();
            self.column_widths.clear();
            self.selected_cell = None;
            self.row_selection.clear();
            self.record_view = None;
            self.diff_rows = None;
            return;
        }
        if idx < self.active_result_idx || self.active_result_idx >= self.result_sets.len() {
            self.active_result_idx = self.active_result_idx.saturating_sub(1);
        }
        let next = &self.result_sets[self.active_result_idx];
        self.page = Some(next.page.clone());
        self.last_status_msg = next.status_msg.clone();
        self.mutator = next.mutator.clone();
        self.column_widths = next.column_widths.clone();
        self.selected_cell = next.selected_cell;
        self.row_selection = next.row_selection.clone();
        self.record_view = None;
        self.diff_rows = None;
    }

    pub(super) fn has_any_unsaved_changes(&self) -> bool {
        self.mutator.has_changes()
            || self
                .result_sets
                .iter()
                .enumerate()
                .any(|(i, rs)| i != self.active_result_idx && rs.mutator.has_changes())
    }
}

pub(super) fn result_set_label(page: &ColumnarPage, idx: usize) -> String {
    let base = page
        .columns
        .iter()
        .find_map(|c| c.table.as_deref().filter(|t| !t.is_empty()))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Result {}", idx + 1));
    let more = if page.has_more_rows { "+" } else { "" };
    format!("{base} ({}{more})", page.row_count)
}
