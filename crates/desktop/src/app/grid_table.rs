use super::query_tab::{MAX_APPENDED_ROWS, MAX_AUTO_SCROLL_ROWS};
use super::result_table::{
    compute_column_widths, compute_single_column_width, render_result_table, ResultTableView,
};
use super::{CottonDataApp, PendingDiscardAction};
use crate::column_glyph::column_type_glyph;
use crate::theme::Palette;
use crate::ui_kit::{
    busy_indicator, caption, empty_state, ghost_button, icon, icon_button, icon_text, inset_frame, loading_state,
    pill, primary_button,
};
use cottondata_grid::{CellValue, Editability, SortDirection};
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

        let mut clicked_sort_col: Option<(usize, String)> = None;
        let mut header_sort_override: Option<(usize, String, Option<SortDirection>)> = None;
        let mut load_full_cell_req: Option<(usize, usize)> = None;
        let mut load_more_req = false;
        let mut rerun_unlimited_req = false;
        let can_load_more = self.can_load_more_for_tab(self.active_tab_idx);
        let next_batch_size = self.effective_tab_batch_size(self.active_tab_idx);
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
                    let Some(page) = tab.page.clone() else {
                        if tab.is_running {
                            loading_state(ui, p, "Running query…");
                        } else {
                            empty_state(ui, p, icon::ROWS, "Run a query or open a table from the explorer.");
                        }
                        return;
                    };

                    if tab.column_widths.len() != page.columns.len() {
                        tab.column_widths = compute_column_widths(ui, &page);
                    }
                    let has_filter_query = !tab.grid_filter.query.trim().is_empty();
                    if has_filter_query {
                        let gen = tab.page_generation;
                        let mutator = &tab.mutator;
                        tab.grid_filter.ensure_synced(gen, &page, |r, c| {
                            mutator.get_modified(r, c).map(|v| v.as_display_str())
                        });
                    }
                    let combined_rows: Option<Vec<usize>> = match (&tab.console_sorted_indices, has_filter_query) {
                        (Some(sorted), true) => {
                            let mut in_filter = vec![false; page.row_count];
                            for &r in &tab.grid_filter.matched_rows {
                                if let Some(slot) = in_filter.get_mut(r) {
                                    *slot = true;
                                }
                            }
                            Some(
                                sorted
                                    .iter()
                                    .copied()
                                    .filter(|&r| in_filter.get(r).copied().unwrap_or(false))
                                    .collect(),
                            )
                        }
                        _ => None,
                    };
                    let visible_rows: Option<&[usize]> = if let Some(combined) = combined_rows.as_deref() {
                        Some(combined)
                    } else if has_filter_query {
                        Some(tab.grid_filter.matched_rows.as_slice())
                    } else {
                        tab.console_sorted_indices.as_deref()
                    };

                    let active_order_by: Option<(String, SortDirection)> = if bound_table.is_some() {
                        tab.filter_sort.order_by.clone()
                    } else {
                        tab.console_sort
                            .as_ref()
                            .map(|(_, col, dir)| (col.clone(), *dir))
                    };

                    let show_footer = page.has_more_rows || tab.is_loading_more;
                    let footer_h = if show_footer { 34.0 } else { 0.0 };
                    let table_h = (available_h - footer_h).max(120.0);
                    let is_sortable = !page.columns.is_empty() && !tab.is_running && !tab.is_loading_more;
                    let interaction = ui
                        .allocate_ui_with_layout(
                            egui::vec2(grid_w, table_h),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                let view = ResultTableView {
                                    palette: p,
                                    tab_id: tab.id,
                                    page: &page,
                                    visible_rows,
                                    mutator: &tab.mutator,
                                    focused_cell: tab.selected_cell,
                                    row_selection: &tab.row_selection,
                                    selected_row_count: tab.row_selection.sorted_rows_below(page.row_count).len(),
                                    column_widths: &tab.column_widths,
                                    order_by: active_order_by.as_ref(),
                                    is_sortable,
                                    editability: &editability,
                                    table_detail: cached_detail.as_ref(),
                                };
                                render_result_table(ui, &view, tab.inline_editor.as_mut())
                            },
                        )
                        .inner;
                    if let Some((col_idx, new_w)) = interaction.resized_column {
                        if let Some(slot_w) = tab.column_widths.get_mut(col_idx) {
                            *slot_w = new_w;
                        }
                    }
                    if let Some(col_idx) = interaction.autofit_column {
                        let fit_w = compute_single_column_width(ui, &page, col_idx);
                        if let Some(slot_w) = tab.column_widths.get_mut(col_idx) {
                            *slot_w = fit_w;
                        }
                    }
                    if interaction.autofit_all_columns {
                        tab.column_widths = compute_column_widths(ui, &page);
                    }
                    if interaction.reset_column_widths {
                        tab.column_widths.clear();
                    }
                    if let Some((col_idx, dir)) = interaction.header_sort_override.filter(|_| is_sortable) {
                        if let Some(col) = page.columns.get(col_idx) {
                            header_sort_override = Some((col_idx, col.name.clone(), dir));
                        }
                    } else if let Some(column_index) = interaction.clicked_header.filter(|_| is_sortable) {
                        if let Some(col) = page.columns.get(column_index) {
                            clicked_sort_col = Some((column_index, col.name.clone()));
                        }
                    }
                    pending_action = interaction.action;

                    let has_inserted_rows = !tab.mutator.inserted_rows().is_empty();
                    let row_count = page.row_count;
                    let has_more_rows = page.has_more_rows;
                    if interaction.reached_scroll_bottom
                        && tab.auto_fetch_on_scroll
                        && !tab.is_running
                        && !tab.is_loading_more
                        && !tab.scroll_fetch_failed
                        && !has_inserted_rows
                        && has_more_rows
                        && row_count < MAX_AUTO_SCROLL_ROWS
                        && can_load_more
                    {
                        load_more_req = true;
                    }

                    if show_footer {
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            if tab.is_loading_more {
                                busy_indicator(ui, p, &format!("Loading next {next_batch_size} rows…"));
                            } else if has_more_rows {
                                let badge_label = if bound_table.is_none() {
                                    format!("{row_count}+ rows · auto-limited")
                                } else if tab.loaded_keyset_order.is_some() {
                                    format!("{row_count}+ rows · keyset seek")
                                } else {
                                    format!("{row_count}+ rows")
                                };
                                pill(ui, &badge_label, p.accent);
                                if row_count >= MAX_APPENDED_ROWS {
                                    caption(
                                        ui,
                                        p,
                                        format!(
                                            "In-memory cap reached ({MAX_APPENDED_ROWS} rows) — use Export → Stream all"
                                        ),
                                    );
                                } else if has_inserted_rows {
                                    caption(ui, p, "Save or revert inserted rows to load more");
                                } else if can_load_more {
                                    if ghost_button(
                                        ui,
                                        p,
                                        icon::ARROW_DOWN,
                                        &format!("Load +{next_batch_size}"),
                                        !tab.is_running,
                                    )
                                    .clicked()
                                    {
                                        load_more_req = true;
                                    }
                                    ui.checkbox(&mut tab.auto_fetch_on_scroll, "Auto-fetch on scroll")
                                        .on_hover_text(format!(
                                            "Automatically load the next batch when scrolling near the bottom (pauses at {MAX_AUTO_SCROLL_ROWS} rows)"
                                        ));
                                    if row_count >= MAX_AUTO_SCROLL_ROWS && tab.auto_fetch_on_scroll {
                                        caption(ui, p, "Auto-scroll paused");
                                    }
                                }
                                if bound_table.is_none()
                                    && ghost_button(ui, p, icon::FAST_FORWARD, "Run without limit", !tab.is_running)
                                        .on_hover_text("Re-run this query once without server-side auto-limit")
                                        .clicked()
                                {
                                    rerun_unlimited_req = true;
                                }
                            }
                        });
                    }
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

        if load_more_req {
            self.request_load_more_active_tab();
        }

        if rerun_unlimited_req {
            self.request_rerun_active_query_unlimited();
        }

        if let Some((row, col)) = load_full_cell_req {
            self.request_load_full_cell(row, col);
        }

        if let Some(action) = pending_action {
            self.apply_table_action(ui.ctx(), action, &editability);
        }

        if let Some((col_idx, col_name, dir)) = header_sort_override {
            let needs_reload = bound_table.is_some()
                || self.tabs[self.active_tab_idx]
                    .page
                    .as_ref()
                    .is_some_and(|pg| pg.has_more_rows);
            if needs_reload && self.tabs[self.active_tab_idx].mutator.has_changes() {
                self.unsaved_guard_modal = Some(PendingDiscardAction::SortColumnExplicit {
                    col_idx,
                    col_name,
                    direction: dir,
                });
            } else {
                self.apply_explicit_sort(col_idx, col_name, dir);
            }
        } else if let Some((col_idx, col_name)) = clicked_sort_col {
            let next_dir = self.next_sort_direction_for_active_tab(&col_name);
            let needs_reload = bound_table.is_some()
                || self.tabs[self.active_tab_idx]
                    .page
                    .as_ref()
                    .is_some_and(|pg| pg.has_more_rows);
            if needs_reload && self.tabs[self.active_tab_idx].mutator.has_changes() {
                self.unsaved_guard_modal = Some(PendingDiscardAction::SortColumnExplicit {
                    col_idx,
                    col_name,
                    direction: next_dir,
                });
            } else {
                self.apply_explicit_sort(col_idx, col_name, next_dir);
            }
        }
    }
}
