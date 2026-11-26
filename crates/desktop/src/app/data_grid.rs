use super::query_tab::{limit_label, RecordViewState, TABLE_PAGE_SIZE_PRESETS};
use super::{ActiveStreamExport, CottonDataApp, PendingDiscardAction};
use crate::theme::Palette;
use crate::ui_kit::{
    busy_indicator, caption, document_tab, ghost_button, icon, icon_button, icon_text, inset_frame, pill,
    primary_button, tinted_ghost_button, toolbar_divider,
};
use crate::worker::SlotCmd;
use cottondata_grid::{export_rows, ExportFormat, SortDirection, SqlInsertTarget};
use eframe::egui::{self, RichText};
use std::fs::File;
use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

impl CottonDataApp {
    pub(super) fn render_data_grid(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut reload_page_idx: Option<u64> = None;
        let mut new_table_page_size: Option<u32> = None;
        let mut count_rows_req = false;
        let mut export_page_req = None;
        let mut export_stream_req = None;
        let mut cancel_export = false;
        let mut save_req: Option<bool> = None;

        let active_slot = self.tabs[self.active_tab_idx].slot.or(self.active_slot);
        let is_production = active_slot
            .and_then(|s| self.registry.get(s))
            .is_some_and(|sl| sl.profile.is_production());
        let bound_table = self.tabs[self.active_tab_idx].bound_table.clone();
        let row_estimate = active_slot
            .zip(bound_table.as_ref())
            .and_then(|(slot, (schema, table))| self.cached_table_detail(slot, schema, table))
            .and_then(|detail| detail.row_estimate);

        if self.tabs[self.active_tab_idx].result_sets.len() > 1 {
            let mut switch_to = None;
            let mut close_idx = None;
            egui::ScrollArea::horizontal()
                .id_salt(("result_subtabs", self.tabs[self.active_tab_idx].id))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let tab = &self.tabs[self.active_tab_idx];
                        for (idx, rs) in tab.result_sets.iter().enumerate() {
                            let is_active = idx == tab.active_result_idx;
                            let is_dirty = if is_active {
                                tab.mutator.has_changes()
                            } else {
                                rs.mutator.has_changes()
                            };
                            let label = if is_dirty {
                                format!("• {}", rs.label)
                            } else {
                                rs.label.clone()
                            };
                            let resp = document_tab(ui, p, icon::TABLE, &label, is_active, true, None);
                            if resp.is_close_clicked {
                                close_idx = Some(idx);
                            } else if resp.is_clicked {
                                switch_to = Some(idx);
                            }
                        }
                    });
                });
            ui.separator();
            if let Some(idx) = switch_to {
                self.tabs[self.active_tab_idx].switch_result_set(idx);
            }
            if let Some(idx) = close_idx {
                self.tabs[self.active_tab_idx].close_result_set(idx);
            }
        }

        let editability = self.resolve_active_tab_editability();
        let mut sql_editor_toggled = false;

        ui.horizontal_wrapped(|ui| {
            let tab = &mut self.tabs[self.active_tab_idx];
            let is_busy = tab.is_running;
            if is_busy && tab.page.is_some() {
                busy_indicator(ui, p, "Loading…");
                toolbar_divider(ui, p);
            }
            if bound_table.is_some() {
                let sql_toggle_label = if tab.show_sql_editor { "Hide SQL" } else { "Show SQL" };
                if ghost_button(ui, p, icon::CODE, sql_toggle_label, true)
                    .on_hover_text("Toggle SQL query editor for this table tab")
                    .clicked()
                {
                    tab.show_sql_editor = !tab.show_sql_editor;
                    sql_editor_toggled = true;
                }
                toolbar_divider(ui, p);
                ui.label(icon_text(icon::FUNNEL, p.accent, "WHERE", p.text_muted, 11.5));
                let filter_resp = ui.add(
                    egui::TextEdit::singleline(&mut tab.filter_sort.where_clause)
                        .hint_text("status = 'active' AND id > 100")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(220.0),
                );
                let is_submitted = filter_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (is_submitted && !is_busy) || ghost_button(ui, p, icon::CHECK, "Apply", !is_busy).clicked() {
                    reload_page_idx = Some(0);
                }
                toolbar_divider(ui, p);
                let has_previous = tab.filter_sort.page_index > 0 && !is_busy;
                if ui
                    .add_enabled_ui(has_previous, |ui| icon_button(ui, p, icon::CARET_LEFT, "Previous page"))
                    .inner
                    .clicked()
                {
                    reload_page_idx = Some(tab.filter_sort.page_index - 1);
                }
                caption(ui, p, format!("Page {}", tab.filter_sort.page_index + 1));
                let has_more = !is_busy && tab.page.as_ref().map(|pg| pg.has_more_rows).unwrap_or(false);
                if ui
                    .add_enabled_ui(has_more, |ui| icon_button(ui, p, icon::CARET_RIGHT, "Next page"))
                    .inner
                    .clicked()
                {
                    reload_page_idx = Some(tab.filter_sort.page_index + 1);
                }
                let cur_page_size = tab.filter_sort.page_size.max(20);
                egui::ComboBox::from_id_salt(("table_page_size", tab.id))
                    .selected_text(
                        RichText::new(format!("{} rows", limit_label(cur_page_size)))
                            .size(11.5)
                            .color(p.text_primary),
                    )
                    .width(78.0)
                    .show_ui(ui, |ui| {
                        for &(preset, label) in TABLE_PAGE_SIZE_PRESETS {
                            if ui.selectable_label(cur_page_size == preset, label).clicked() {
                                new_table_page_size = Some(preset);
                            }
                        }
                    })
                    .response
                    .on_hover_text("Rows fetched per page / scroll batch");
                let (total_label, total_tip) = match tab
                    .page
                    .as_ref()
                    .and_then(|pg| pg.total_rows_known)
                {
                    Some(n) => (format!("{n} rows"), "Exact row count — click to recount"),
                    None => match row_estimate {
                        Some(est) => (
                            format!("~{} rows · Exact", format_compact_count(est)),
                            "Estimated from table metadata — click to run exact COUNT(*)",
                        ),
                        None => ("Count rows".into(), "Run exact COUNT(*) for this table"),
                    },
                };
                if tab.is_counting_rows {
                    busy_indicator(ui, p, "Counting…");
                } else if ghost_button(ui, p, icon::HASH, &total_label, !is_busy)
                    .on_hover_text(total_tip)
                    .clicked()
                {
                    count_rows_req = true;
                }
            }

            if tab.page.is_none() {
                return;
            }

            toolbar_divider(ui, p);
            if let Some(target) = editability.target() {
                if bound_table.is_none() {
                    pill(ui, &format!("EDIT {}", target.table), p.success);
                }
                if ghost_button(ui, p, icon::PLUS, "Row", true)
                    .on_hover_text("Insert a blank row")
                    .clicked()
                {
                    if let Some(pg) = &tab.page {
                        let ins = tab.mutator.insert_blank_row(pg);
                        let new_row = pg.row_count + ins;
                        tab.select_cell(new_row, 0);
                    }
                }
                if let Some((sel_r, _)) = tab.selected_cell {
                    if ghost_button(ui, p, icon::COPY, "Clone", true)
                        .on_hover_text("Clone the selected row (Cmd+D)")
                        .clicked()
                    {
                        if let Some(pg) = &tab.page {
                            if sel_r < pg.row_count {
                                let ins = tab.mutator.clone_row(pg, sel_r);
                                tab.select_cell(pg.row_count + ins, 0);
                            }
                        }
                    }
                    if tinted_ghost_button(ui, p.danger, icon::TRASH, "Delete", true)
                        .on_hover_text("Toggle delete on the selected row")
                        .clicked()
                    {
                        if let Some(pg) = &tab.page {
                            if sel_r < pg.row_count {
                                tab.mutator.toggle_delete_row(sel_r);
                                tab.bump_page_generation();
                            } else {
                                tab.mutator.remove_inserted_row(sel_r - pg.row_count);
                                tab.bump_page_generation();
                                tab.selected_cell = None;
                            }
                        }
                    }
                }
                if tab.mutator.has_changes() {
                    let (deletes, updates, inserts) = tab.mutator.pending_summary();
                    let save_label = if is_production {
                        format!("Review & Save  D{deletes} U{updates} I{inserts}")
                    } else {
                        format!("Save  D{deletes} U{updates} I{inserts}")
                    };
                    let save_tooltip = if is_production {
                        "Production server: previews SQL before committing (Cmd+S)"
                    } else {
                        "Commit staged cell edits directly to database (Cmd+S)"
                    };
                    let can_save = !is_busy && !tab.is_submitting_mutations;
                    if tab.is_submitting_mutations {
                        busy_indicator(ui, p, "Saving…");
                    } else {
                        if primary_button(ui, p, icon::FLOPPY_DISK, &save_label, can_save)
                            .on_hover_text(save_tooltip)
                            .clicked()
                        {
                            save_req = Some(false);
                        }
                        if !is_production
                            && ghost_button(ui, p, icon::CODE, "Preview SQL", can_save)
                                .on_hover_text("Inspect generated SQL before committing")
                                .clicked()
                        {
                            save_req = Some(true);
                        }
                    }
                    if ghost_button(ui, p, icon::ARROW_U_UP_LEFT, "Revert all", can_save).clicked() {
                        tab.cancel_inline_edit();
                        tab.mutator.revert_all();
                        tab.bump_page_generation();
                        if let Some((r, c)) = tab.selected_cell {
                            tab.select_cell(r, c);
                        }
                    }
                }
            } else if let Some(reason) = editability.reason() {
                caption(ui, p, "Read-only").on_hover_text(reason.message());
            }

            toolbar_divider(ui, p);
            let filter_active = tab.grid_filter.open || !tab.grid_filter.query.trim().is_empty();
            let filter_resp = if filter_active {
                tinted_ghost_button(ui, p.accent, icon::FUNNEL, "Filter", true)
            } else {
                ghost_button(ui, p, icon::FUNNEL, "Filter", true)
            };
            if filter_resp
                .on_hover_text("Filter loaded rows in memory (Cmd/Ctrl+Shift+F)")
                .clicked()
            {
                if tab.grid_filter.open && tab.grid_filter.query.is_empty() {
                    tab.grid_filter.open = false;
                } else {
                    tab.grid_filter.open = true;
                    tab.grid_filter.focus_input = true;
                }
            }
            toolbar_divider(ui, p);
            let record_open = tab.record_view.is_some();
            let record_label = if record_open { "Hide list" } else { "Row list" };
            if ghost_button(ui, p, icon::ROWS, record_label, true)
                .on_hover_text("View selected row vertically as a list (Record view)")
                .clicked()
            {
                if record_open {
                    tab.record_view = None;
                } else {
                    let row = tab.selected_cell.map(|(r, _)| r).unwrap_or(0);
                    tab.is_inspector_open = false;
                    tab.record_view = Some(RecordViewState {
                        row,
                        filter: String::new(),
                        hide_nulls: false,
                        pretty_json: true,
                    });
                }
            }
            toolbar_divider(ui, p);
            let export_busy = self.active_export.is_some();
            if let Some(exp) = &self.active_export {
                busy_indicator(
                    ui,
                    p,
                    &format!("Exporting {} rows ({})…", exp.rows_written, exp.path_label),
                );
                if tinted_ghost_button(ui, p.danger, icon::STOP, "Cancel", true).clicked() {
                    cancel_export = true;
                }
            } else {
                let page_rows = self.tabs[self.active_tab_idx]
                    .page
                    .as_ref()
                    .map(|pg| pg.row_count)
                    .unwrap_or(0);
                let can_stream = active_slot.is_some()
                    && (bound_table.is_some()
                        || self.tabs[self.active_tab_idx]
                            .page
                            .as_ref()
                            .is_some_and(|pg| !pg.executed_sql.trim().is_empty()));
                egui::ComboBox::from_id_salt("export_menu")
                    .selected_text(icon_text(icon::EXPORT, p.accent, "Export", p.text_primary, 13.0))
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        ui.label(
                            RichText::new(format!("Current page ({page_rows} rows)"))
                                .size(11.0)
                                .strong()
                                .color(p.text_muted),
                        );
                        for fmt in ExportFormat::ALL {
                            if ui.selectable_label(false, fmt.label()).clicked() {
                                export_page_req = Some(fmt);
                            }
                        }
                        if can_stream && !export_busy {
                            ui.separator();
                            ui.label(
                                RichText::new("Full result (streaming, all rows)")
                                    .size(11.0)
                                    .strong()
                                    .color(p.accent),
                            );
                            for fmt in ExportFormat::ALL {
                                if ui
                                    .selectable_label(false, format!("Stream all → {}", fmt.short_label()))
                                    .clicked()
                                {
                                    export_stream_req = Some(fmt);
                                }
                            }
                        }
                    });
            }
        });

        if cancel_export {
            if let Some(exp) = &self.active_export {
                exp.cancel_flag.store(true, Ordering::Relaxed);
            }
        }

        if sql_editor_toggled {
            self.mark_session_dirty();
        }

        if let Some(page_size) = new_table_page_size {
            if self.tabs[self.active_tab_idx].mutator.has_changes() {
                self.unsaved_guard_modal = Some(PendingDiscardAction::ChangeTablePageSize(page_size));
            } else {
                self.tabs[self.active_tab_idx].filter_sort.page_size = page_size;
                self.tabs[self.active_tab_idx].filter_sort.page_index = 0;
                self.send_reload_active_table_page();
            }
        }

        if let Some(page_index) = reload_page_idx {
            if self.tabs[self.active_tab_idx].mutator.has_changes() {
                self.unsaved_guard_modal = Some(PendingDiscardAction::ReloadTablePage { page_index });
            } else {
                let prev_page = self.tabs[self.active_tab_idx].filter_sort.page_index;
                let sequential_next = page_index == prev_page.saturating_add(1);
                let dialect = self.slot_dialect(active_slot);
                let keyset_seek = if sequential_next {
                    self.build_active_tab_keyset_seek(dialect)
                } else {
                    None
                };
                self.tabs[self.active_tab_idx].filter_sort.page_index = page_index;
                self.send_reload_active_table_page_with_seek(keyset_seek);
            }
        }

        if count_rows_req {
            if let (Some(slot), Some((schema, table))) = (active_slot, bound_table.clone()) {
                let tab = &mut self.tabs[self.active_tab_idx];
                tab.is_counting_rows = true;
                self.hub.send(
                    slot,
                    SlotCmd::CountTableRows {
                        tab_id: tab.id,
                        schema,
                        table,
                        filter_sort: tab.filter_sort.clone(),
                    },
                );
            }
        }

        if let Some(fmt) = export_page_req {
            self.export_active_page(fmt);
        }

        if let Some(fmt) = export_stream_req {
            self.start_stream_export(fmt);
        }

        if let Some(force_preview) = save_req {
            self.trigger_save_active_tab(force_preview);
        }

        self.render_grid_filter_bar(ui, p);
        self.render_grid_body(ui, p, bound_table, editability);
    }

    fn render_grid_filter_bar(&mut self, ui: &mut egui::Ui, p: Palette) {
        let tab = &mut self.tabs[self.active_tab_idx];
        if !tab.grid_filter.open {
            return;
        }
        let Some(page) = tab.page.clone() else {
            return;
        };
        if !tab.grid_filter.query.trim().is_empty() {
            let gen = tab.page_generation;
            let mutator = &tab.mutator;
            tab.grid_filter.ensure_synced(gen, &page, |r, c| {
                mutator.get_modified(r, c).map(|v| v.as_display_str())
            });
        }
        ui.add_space(4.0);
        let mut close_bar = false;
        inset_frame(p).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::MAGNIFYING_GLASS).size(13.0).color(p.accent));
                let edit_resp = ui.add(
                    egui::TextEdit::singleline(&mut tab.grid_filter.query)
                        .id(egui::Id::new(("grid_filter_input", tab.id)))
                        .hint_text("Filter loaded rows…")
                        .desired_width(240.0),
                );
                if tab.grid_filter.focus_input {
                    edit_resp.request_focus();
                    tab.grid_filter.focus_input = false;
                }
                if edit_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close_bar = true;
                }
                if !tab.grid_filter.query.trim().is_empty() {
                    let matched = tab.grid_filter.matched_rows.len();
                    let total = page.row_count;
                    caption(ui, p, format!("{matched}/{total} rows"));
                } else {
                    caption(ui, p, format!("{} rows", page.row_count));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, p, icon::X, "Clear and close filter (Esc)").clicked() {
                        close_bar = true;
                    }
                    if !tab.grid_filter.query.is_empty()
                        && ghost_button(ui, p, icon::X, "Clear", true).clicked()
                    {
                        tab.grid_filter.query.clear();
                        tab.grid_filter.focus_input = true;
                    }
                });
            });
        });
        if close_bar {
            tab.grid_filter.query.clear();
            tab.grid_filter.open = false;
        }
    }

    fn export_active_page(&mut self, fmt: ExportFormat) {
        let Some(page) = &self.tabs[self.active_tab_idx].page else {
            return;
        };
        let default_name = format!(
            "{}.{}",
            self.tabs[self.active_tab_idx].title.replace(['.', ' ', '/'], "_"),
            fmt.extension()
        );
        let Some(path) = rfd::FileDialog::new().set_file_name(&default_name).save_file() else {
            return;
        };
        let tab = &self.tabs[self.active_tab_idx];
        let all_rows: Vec<usize> = (0..page.row_count).collect();
        let target_table = tab.insert_target_table();
        let target = SqlInsertTarget {
            dialect: self.slot_dialect(tab.slot),
            table_name: &target_table,
        };
        let result = File::create(&path).and_then(|file| {
            let mut writer = BufWriter::new(file);
            export_rows(page, &all_rows, fmt, &target, &mut writer)
        });
        match result {
            Ok(()) => {
                self.notify_success(format!("Exported to {}", path.display()));
            }
            Err(err) => {
                self.notify_error(format!("Export failed: {err}"));
            }
        }
    }

    fn start_stream_export(&mut self, fmt: ExportFormat) {
        if self.active_export.is_some() {
            self.notify_warning("Another streaming export is already in progress.");
            return;
        }
        let tab = &self.tabs[self.active_tab_idx];
        let Some(slot_id) = tab.slot.or(self.active_slot) else {
            self.notify_error("Connect to a server before exporting.");
            return;
        };
        let Some(slot) = self.registry.get(slot_id) else {
            return;
        };
        let profile = slot.profile.clone();
        let active_schema = slot
            .runtime
            .as_ref()
            .and_then(|r| r.current_schema.clone())
            .or_else(|| profile.initial_schema());
        let dialect = profile.engine.dialect();

        let sql = if let Some((schema, table)) = &tab.bound_table {
            let fq = dialect.qualified_table(schema, table);
            let mut query = format!("SELECT * FROM {fq}");
            if !tab.filter_sort.where_clause.trim().is_empty() {
                query.push_str(" WHERE ");
                query.push_str(tab.filter_sort.where_clause.trim());
            }
            if let Some((col, dir)) = &tab.filter_sort.order_by {
                let dir_kw = match dir {
                    SortDirection::Asc => "ASC",
                    SortDirection::Desc => "DESC",
                };
                query.push_str(&format!(" ORDER BY {} {dir_kw}", dialect.quote_ident(col)));
            }
            query
        } else if let Some(page) = &tab.page {
            page.executed_sql.trim().trim_end_matches(';').to_string()
        } else {
            return;
        };

        if sql.is_empty() {
            return;
        }

        let default_name = format!(
            "{}_full.{}",
            tab.title.replace(['.', ' ', '/'], "_"),
            fmt.extension()
        );
        let Some(path) = rfd::FileDialog::new().set_file_name(&default_name).save_file() else {
            return;
        };
        let table_name = tab.insert_target_table();
        let password = self.storage.get_password(&profile.id);
        self.export_seq += 1;
        let export_id = self.export_seq;
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let path_label = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("export")
            .to_string();

        self.active_export = Some(ActiveStreamExport {
            export_id,
            path_label,
            rows_written: 0,
            cancel_flag: Arc::clone(&cancel_flag),
        });
        self.hub.stream_export_async(
            export_id,
            profile,
            password,
            active_schema,
            sql,
            fmt,
            table_name,
            path,
            cancel_flag,
        );
    }
}

fn format_compact_count(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
