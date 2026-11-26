use super::{CottonDataApp, PendingDiscardAction};
use crate::theme::Palette;
use crate::ui_kit::{
    busy_indicator, caption, ghost_button, icon, icon_button, icon_text, pill, primary_button, tinted_ghost_button,
    toolbar_divider,
};
use crate::worker::SlotCmd;
use cottondata_core::EnvironmentTag;
use cottondata_grid::{export_rows, ExportFormat, SqlInsertTarget};
use eframe::egui;
use std::fs::File;
use std::io::BufWriter;

impl CottonDataApp {
    pub(super) fn render_data_grid(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut reload_page_idx: Option<u64> = None;
        let mut count_rows_req = false;
        let mut export_req = None;
        let mut save_req: Option<bool> = None;

        let active_slot = self.tabs[self.active_tab_idx].slot.or(self.active_slot);
        let is_production = active_slot
            .and_then(|s| self.registry.get(s))
            .is_some_and(|sl| sl.profile.environment == EnvironmentTag::Production);
        let bound_table = self.tabs[self.active_tab_idx].bound_table.clone();
        let editability = self.resolve_active_tab_editability();

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
                let total_label = tab
                    .page
                    .as_ref()
                    .and_then(|pg| pg.total_rows_known)
                    .map(|n| format!("{n} rows"))
                    .unwrap_or_else(|| "Count rows".into());
                if tab.is_counting_rows {
                    busy_indicator(ui, p, "Counting…");
                } else if ghost_button(ui, p, icon::HASH, &total_label, !is_busy).clicked() {
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
                            } else {
                                tab.mutator.remove_inserted_row(sel_r - pg.row_count);
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
                        if let Some((r, c)) = tab.selected_cell {
                            tab.select_cell(r, c);
                        }
                    }
                }
            } else if let Some(reason) = editability.reason() {
                caption(ui, p, "Read-only").on_hover_text(reason.message());
            }

            toolbar_divider(ui, p);
            egui::ComboBox::from_id_salt("export_menu")
                .selected_text(icon_text(icon::EXPORT, p.accent, "Export", p.text_primary, 13.0))
                .width(110.0)
                .show_ui(ui, |ui| {
                    for fmt in ExportFormat::ALL {
                        if ui.selectable_label(false, fmt.label()).clicked() {
                            export_req = Some(fmt);
                        }
                    }
                });
        });

        if let Some(page_index) = reload_page_idx {
            if self.tabs[self.active_tab_idx].mutator.has_changes() {
                self.unsaved_guard_modal = Some(PendingDiscardAction::ReloadTablePage { page_index });
            } else {
                self.tabs[self.active_tab_idx].filter_sort.page_index = page_index;
                self.send_reload_active_table_page();
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

        if let Some(fmt) = export_req {
            self.export_active_page(fmt);
        }

        if let Some(force_preview) = save_req {
            self.trigger_save_active_tab(force_preview);
        }

        self.render_grid_body(ui, p, bound_table, editability);
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
                self.toast_banner = Some((format!("Exported to {}", path.display()), false));
            }
            Err(err) => {
                self.toast_banner = Some((format!("Export failed: {err}"), true));
            }
        }
    }
}
