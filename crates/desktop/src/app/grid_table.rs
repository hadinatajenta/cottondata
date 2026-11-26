use super::result_table::{compute_column_widths, render_result_table, ResultTableView};
use super::CottonDataApp;
use crate::column_glyph::column_type_glyph;
use crate::theme::Palette;
use crate::ui_kit::{empty_state, icon, icon_button, icon_text, inset_frame, primary_button};
use crate::worker::SlotCmd;
use cottondata_core::ServerSlotId;
use cottondata_grid::{CellValue, SortDirection};
use eframe::egui::{self, RichText};

impl CottonDataApp {
    pub(super) fn render_grid_body(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        active_slot: Option<ServerSlotId>,
        bound_table: Option<(String, String)>,
        is_read_only: bool,
    ) {
        ui.add_space(4.0);

        let mut clicked_sort_col: Option<String> = None;
        let available_h = ui.available_height().max(160.0);

        self.handle_table_shortcuts(ui.ctx());
        let mut pending_action = None;
        let active_tab = &self.tabs[self.active_tab_idx];
        let has_selected = active_tab.selected_cell.is_some() && active_tab.is_inspector_open;
        let inspector_w = if has_selected { 290.0 } else { 0.0 };
        let grid_w = (ui.available_width() - inspector_w - 8.0).max(300.0);

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(grid_w, available_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let tab = &mut self.tabs[self.active_tab_idx];
                    let Some(page) = &tab.page else {
                        empty_state(ui, p, icon::ROWS, "Run a query or open a table from the navigator.");
                        return;
                    };

                    if tab.column_widths.len() != page.columns.len() {
                        tab.column_widths = compute_column_widths(ui, page);
                    }
                    let view = ResultTableView {
                        palette: p,
                        tab_id: tab.id,
                        page,
                        mutator: &tab.mutator,
                        focused_cell: tab.selected_cell,
                        row_selection: &tab.row_selection,
                        selected_row_count: tab.row_selection.sorted_rows_below(page.row_count).len(),
                        column_widths: &tab.column_widths,
                        order_by: tab.filter_sort.order_by.as_ref(),
                        is_sortable: bound_table.is_some(),
                    };
                    let interaction = render_result_table(ui, &view);
                    if let Some(column_index) = interaction.clicked_header.filter(|_| view.is_sortable) {
                        clicked_sort_col = Some(page.columns[column_index].name.clone());
                    }
                    pending_action = interaction.action;
                },
            );

            if has_selected {
                ui.allocate_ui_with_layout(
                    egui::vec2(inspector_w, available_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        inset_frame(p).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                            let tab = &mut self.tabs[self.active_tab_idx];
                            if let (Some((r, c)), Some(page)) = (tab.selected_cell, &tab.page) {
                                let col = &page.columns[c];
                                ui.horizontal(|ui| {
                                    ui.label(icon_text(
                                        column_type_glyph(&col.type_name),
                                        p.accent,
                                        &format!("Row {} · {}", r + 1, col.name),
                                        p.text_primary,
                                        13.0,
                                    ));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if icon_button(ui, p, icon::X, "Close inspector").clicked() {
                                            tab.is_inspector_open = false;
                                        }
                                    });
                                });
                                ui.label(
                                    RichText::new(format!("Type: {}", col.type_name))
                                        .size(11.0)
                                        .color(p.text_muted),
                                );
                                if r < page.row_count {
                                    if let Some(tot) = page.cell_truncated_bytes(r, c) {
                                        ui.label(
                                            RichText::new(format!("Preview capped at 4 KiB (total {tot} B)"))
                                                .size(10.5)
                                                .color(p.warning),
                                        );
                                    }
                                }
                                ui.separator();
                                ui.checkbox(&mut tab.cell_edit_is_null, "SQL NULL");
                                egui::ScrollArea::vertical()
                                    .max_height((available_h - 140.0).max(80.0))
                                    .show(ui, |ui| {
                                        ui.add_enabled(
                                            !tab.cell_edit_is_null,
                                            egui::TextEdit::multiline(&mut tab.cell_edit_buf)
                                                .code_editor()
                                                .desired_width(f32::INFINITY)
                                                .desired_rows(6),
                                        );
                                    });
                                ui.add_space(6.0);
                                if bound_table.is_some() && !is_read_only {
                                    ui.horizontal(|ui| {
                                        if primary_button(ui, p, icon::PENCIL_SIMPLE, "Stage edit", true).clicked() {
                                            let new_val = if tab.cell_edit_is_null {
                                                CellValue::Null
                                            } else {
                                                CellValue::Text(tab.cell_edit_buf.clone())
                                            };
                                            if r >= page.row_count {
                                                tab.mutator.set_inserted_cell(r - page.row_count, c, new_val);
                                            } else {
                                                tab.mutator.set_cell(page, r, c, new_val);
                                            }
                                        }
                                        if r < page.row_count && ui.small_button("Revert").clicked() {
                                            tab.mutator.revert_cell(r, c);
                                        }
                                    });
                                }
                            }
                        });
                    },
                );
            }
        });

        if let Some(action) = pending_action {
            self.apply_table_action(ui.ctx(), action);
        }

        if let Some(col_name) = clicked_sort_col {
            if let (Some(slot), Some((schema, table))) = (active_slot, bound_table) {
                let tab = &mut self.tabs[self.active_tab_idx];
                let next_dir = match &tab.filter_sort.order_by {
                    Some((c, SortDirection::Asc)) if c == &col_name => Some(SortDirection::Desc),
                    Some((c, SortDirection::Desc)) if c == &col_name => None,
                    _ => Some(SortDirection::Asc),
                };
                tab.filter_sort.order_by = next_dir.map(|d| (col_name, d));
                tab.filter_sort.page_index = 0;
                tab.sql = tab.filter_sort.build_select_sql(&schema, &table);
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
    }
}
