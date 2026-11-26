use super::row_selection::RowSelection;
use cottondata_core::ServerSlotId;
use cottondata_grid::{CellValue, ColumnarPage, PageFilterSort, SortDirection, StagedMutator};
use cottondata_mysql::ExplainResult;
use cottondata_storage::{SessionSortOrder, SessionTabState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BottomDeckTab {
    DataGrid,
    DdlInspector,
    ExplainPlan,
    Monitor,
    History,
}

#[derive(Clone)]
pub(super) struct QueryTab {
    pub(super) id: u64,
    pub(super) title: String,
    pub(super) slot: Option<ServerSlotId>,
    pub(super) sql: String,
    pub(super) cursor_byte: usize,
    pub(super) bound_table: Option<(String, String)>,
    pub(super) filter_sort: PageFilterSort,
    pub(super) page: Option<ColumnarPage>,
    pub(super) last_status_msg: String,
    pub(super) is_running: bool,
    pub(super) mutator: StagedMutator,
    pub(super) selected_cell: Option<(usize, usize)>,
    pub(super) cell_edit_buf: String,
    pub(super) cell_edit_is_null: bool,
    pub(super) explain: Option<ExplainResult>,
    pub(super) column_widths: Vec<f32>,
    pub(super) row_selection: RowSelection,
    pub(super) is_inspector_open: bool,
    pub(super) is_counting_rows: bool,
    pub(super) is_submitting_mutations: bool,
}

impl QueryTab {
    pub(super) fn new_console(id: u64, slot: Option<ServerSlotId>) -> Self {
        Self {
            id,
            title: format!("Console {id}"),
            slot,
            sql: "SELECT VERSION() AS mysql_version, DATABASE() AS current_db, NOW() AS server_time;".to_string(),
            cursor_byte: 0,
            bound_table: None,
            filter_sort: PageFilterSort {
                page_size: 500,
                ..Default::default()
            },
            page: None,
            last_status_msg: "Ready. Press Cmd/Ctrl+Enter to run the current statement".into(),
            is_running: false,
            mutator: StagedMutator::new(),
            selected_cell: None,
            cell_edit_buf: String::new(),
            cell_edit_is_null: false,
            explain: None,
            column_widths: Vec::new(),
            row_selection: RowSelection::default(),
            is_inspector_open: false,
            is_counting_rows: false,
            is_submitting_mutations: false,
        }
    }

    pub(super) fn new_table(id: u64, slot: ServerSlotId, schema: String, table: String, page_size: u32) -> Self {
        let filter_sort = PageFilterSort {
            page_size,
            ..Default::default()
        };
        let sql = filter_sort.build_select_sql(&schema, &table);
        Self {
            id,
            title: format!("{schema}.{table}"),
            slot: Some(slot),
            sql,
            cursor_byte: 0,
            bound_table: Some((schema, table)),
            filter_sort,
            page: None,
            last_status_msg: "Loading table page…".into(),
            is_running: true,
            mutator: StagedMutator::new(),
            selected_cell: None,
            cell_edit_buf: String::new(),
            cell_edit_is_null: false,
            explain: None,
            column_widths: Vec::new(),
            row_selection: RowSelection::default(),
            is_inspector_open: false,
            is_counting_rows: false,
            is_submitting_mutations: false,
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
            bound_table: state.bound_table.clone(),
            filter_sort,
            page: None,
            last_status_msg,
            is_running: false,
            mutator: StagedMutator::new(),
            selected_cell: None,
            cell_edit_buf: String::new(),
            cell_edit_is_null: false,
            explain: None,
            column_widths: Vec::new(),
            row_selection: RowSelection::default(),
            is_inspector_open: false,
            is_counting_rows: false,
            is_submitting_mutations: false,
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
        self.cell_edit_buf = if current_value.is_null() {
            String::new()
        } else {
            current_value.as_display_str().to_string()
        };
    }
}
