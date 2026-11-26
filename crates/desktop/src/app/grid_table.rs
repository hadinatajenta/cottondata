use super::result_table::{
    compute_column_widths, compute_single_column_width, render_result_table, ResultTableView,
};
use super::{CottonDataApp, PendingDiscardAction};
use crate::column_glyph::column_type_glyph;
use crate::theme::Palette;
use crate::ui_kit::{
    busy_indicator, empty_state, ghost_button, icon, icon_button, icon_text, inset_frame, loading_state,
    primary_button,
};
use cottondata_grid::{CellValue, Editability};
use eframe::egui::{self, RichText};

impl CottonDataApp {
    pub(super) fn render_grid_body(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        bound_table: Option<(String, String)>,
        editability: Editability,
    ) {
        ui.add_space(4.0);

        let mut clicked_sort_col: Option<String> = None;
        let mut load_full_cell_req: Option<(usize, usize)> = None;
        let available_h = ui.available_height().max(160.0);

        self.handle_table_shortcuts(ui.ctx(), &editability);
        let mut pending_action = None;
        let active_tab = &self.tabs[self.active_tab_idx];
        let active_slot_for_fk = active_tab.slot.or(self.active_slot);
        let fk_target = bound_table.clone().or_else(|| {
            let page = active_tab.page.as_ref()?;
            let first = page.columns.iter().find(|c| c.table.is_some())?;
            let table = first.table.clone()?;
            let schema = first.schema.clone().or_else(|| {
                active_slot_for_fk
                    .and_then(|s| self.registry.get(s))
                    .and_then(|sl| sl.runtime.as_ref())
                    .and_then(|r| r.current_schema.clone())
            })?;
            Some((schema, table))
        });
        let cached_detail = active_slot_for_fk
            .zip(fk_target.as_ref())
            .and_then(|(slot, (schema, table))| self.cached_table_detail(slot, schema, table))
            .cloned();
        let has_record_view = active_tab.record_view.is_some() && active_tab.page.is_some();
        let has_selected = !has_record_view && active_tab.selected_cell.is_some() && active_tab.is_inspector_open;
        let side_w = if has_record_view {
            420.0
        } else if has_selected {
            300.0
        } else {
            0.0
        };
        let grid_w = (ui.available_width() - side_w - if side_w > 0.0 { 8.0 } else { 0.0 }).max(260.0);

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(grid_w, available_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let tab = &mut self.tabs[self.active_tab_idx];
                    let Some(page) = &tab.page else {
                        if tab.is_running {
                            loading_state(ui, p, "Running query…");
                        } else {
                            empty_state(ui, p, icon::ROWS, "Run a query or open a table from the explorer.");
                        }
                        return;
                    };

                    if tab.column_widths.len() != page.columns.len() {
                        tab.column_widths = compute_column_widths(ui, page);
                    }
                    let is_sortable = bound_table.is_some() && !tab.is_running;
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
                        is_sortable,
                        editability: &editability,
                        table_detail: cached_detail.as_ref(),
                    };
                    let interaction = render_result_table(ui, &view, tab.inline_editor.as_mut());
                    if let Some((col_idx, new_w)) = interaction.resized_column {
                        if let Some(slot_w) = tab.column_widths.get_mut(col_idx) {
                            *slot_w = new_w;
                        }
                    }
                    if let Some(col_idx) = interaction.autofit_column {
                        let fit_w = compute_single_column_width(ui, page, col_idx);
                        if let Some(slot_w) = tab.column_widths.get_mut(col_idx) {
                            *slot_w = fit_w;
                        }
                    }
                    if let Some(column_index) = interaction.clicked_header.filter(|_| is_sortable) {
                        clicked_sort_col = Some(page.columns[column_index].name.clone());
                    }
                    pending_action = interaction.action;
                },
            );

            if has_record_view {
                ui.allocate_ui_with_layout(
                    egui::vec2(side_w, available_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        if let Some(req) = self.render_record_view_drawer(ui, p, available_h, &editability) {
                            load_full_cell_req = Some(req);
                        }
                    },
                );
            } else if has_selected {
                ui.allocate_ui_with_layout(
                    egui::vec2(side_w, available_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        inset_frame(p).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                            let tab = &mut self.tabs[self.active_tab_idx];
                            let cell_meta = tab.selected_cell.and_then(|(r, c)| {
                                let page = tab.page.as_ref()?;
                                let col = page.columns.get(c)?;
                                let truncated_bytes = if r < page.row_count {
                                    page.cell_truncated_bytes(r, c)
                                } else {
                                    None
                                };
                                Some((
                                    r,
                                    c,
                                    page.row_count,
                                    col.name.clone(),
                                    col.type_name.clone(),
                                    col.is_nullable,
                                    truncated_bytes,
                                ))
                            });
                            if let Some((r, c, page_row_count, col_name, col_type, col_nullable, truncated_bytes)) =
                                cell_meta
                            {
                                let can_edit_col = editability.is_column_editable(c);
                                let is_truncated_unloaded = truncated_bytes.is_some()
                                    && tab.mutator.get_modified(r, c).is_none()
                                    && tab.cell_edit_buf.len() <= cottondata_core::MAX_INLINE_CELL_BYTES;
                                ui.horizontal(|ui| {
                                    ui.label(icon_text(
                                        column_type_glyph(&col_type),
                                        p.accent,
                                        &format!("Row {} · {}", r + 1, col_name),
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
                                    RichText::new(format!("Type: {col_type}"))
                                        .size(11.0)
                                        .color(p.text_muted),
                                );
                                if let Some(tot) = truncated_bytes {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(format!("Preview capped at 4 KiB (total {tot} B)"))
                                                .size(10.5)
                                                .color(p.warning),
                                        );
                                        if tab.is_loading_full_cell {
                                            busy_indicator(ui, p, "Loading…");
                                        } else if editability.is_editable()
                                            && ghost_button(ui, p, icon::ARROW_CLOCKWISE, "Load full", true).clicked()
                                        {
                                            load_full_cell_req = Some((r, c));
                                        }
                                    });
                                }
                                ui.separator();
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            can_edit_col && col_nullable,
                                            egui::Checkbox::new(&mut tab.cell_edit_is_null, "SQL NULL"),
                                        )
                                        .changed()
                                        && tab.cell_edit_is_null
                                    {
                                        tab.cell_edit_is_default = false;
                                    }
                                    if ui
                                        .add_enabled(
                                            can_edit_col,
                                            egui::Checkbox::new(&mut tab.cell_edit_is_default, "DEFAULT"),
                                        )
                                        .changed()
                                        && tab.cell_edit_is_default
                                    {
                                        tab.cell_edit_is_null = false;
                                    }
                                });
                                let editor_enabled = can_edit_col
                                    && !tab.cell_edit_is_null
                                    && !tab.cell_edit_is_default
                                    && !is_truncated_unloaded;
                                egui::ScrollArea::vertical()
                                    .max_height((available_h - 165.0).max(80.0))
                                    .show(ui, |ui| {
                                        ui.add_enabled(
                                            editor_enabled,
                                            egui::TextEdit::multiline(&mut tab.cell_edit_buf)
                                                .code_editor()
                                                .desired_width(f32::INFINITY)
                                                .desired_rows(6),
                                        );
                                    });
                                ui.add_space(6.0);
                                if can_edit_col {
                                    ui.horizontal(|ui| {
                                        let can_stage = !is_truncated_unloaded
                                            || tab.cell_edit_is_null
                                            || tab.cell_edit_is_default;
                                        if primary_button(ui, p, icon::CHECK, "Apply edit", can_stage).clicked() {
                                            let new_val = if tab.cell_edit_is_null {
                                                CellValue::Null
                                            } else if tab.cell_edit_is_default {
                                                CellValue::DefaultExpr
                                            } else {
                                                CellValue::Text(tab.cell_edit_buf.clone())
                                            };
                                            tab.stage_cell_value(r, c, new_val);
                                        }
                                        if r < page_row_count && ui.small_button("Revert").clicked() {
                                            tab.mutator.revert_cell(r, c);
                                            tab.select_cell(r, c);
                                        }
                                    });
                                }
                            }
                        });
                    },
                );
            }
        });

        if let Some((row, col)) = load_full_cell_req {
            self.request_load_full_cell(row, col);
        }

        if let Some(action) = pending_action {
            self.apply_table_action(ui.ctx(), action, &editability);
        }

        if let Some(col_name) = clicked_sort_col {
            if self.tabs[self.active_tab_idx].mutator.has_changes() {
                self.unsaved_guard_modal = Some(PendingDiscardAction::SortColumn(col_name));
            } else {
                self.apply_sort_column(col_name);
            }
        }
    }
}
