use super::CottonDataApp;
use crate::theme::Palette;
use crate::ui_kit::{
    busy_indicator, caption, ghost_button, icon, icon_button, icon_text, primary_button, tinted_ghost_button,
    toolbar_divider,
};
use crate::worker::SlotCmd;
use cottondata_grid::{export_rows, ExportFormat, SqlInsertTarget};
use eframe::egui;
use std::fs::File;
use std::io::BufWriter;

impl CottonDataApp {
    pub(super) fn render_data_grid(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut reload_page = false;
        let mut count_rows_req = false;
        let mut export_req = None;
        let mut preview_req = false;

        let active_slot = self.tabs[self.active_tab_idx].slot.or(self.active_slot);
        let is_read_only = active_slot
            .and_then(|s| self.registry.get(s))
            .map(|sl| sl.profile.read_only)
            .unwrap_or(false);

        let bound_table = self.tabs[self.active_tab_idx].bound_table.clone();

        ui.horizontal_wrapped(|ui| {
            let tab = &mut self.tabs[self.active_tab_idx];
            let is_busy = tab.is_running;
            if is_busy && tab.page.is_some() {
                busy_indicator(ui, p, "Loading…");
                toolbar_divider(ui, p);
            }
            if bound_table.is_some() {
                ui.label(icon_text(icon::FUNNEL, p.accent, "WHERE", p.text_muted, 11.5));
                let filter_resp = ui.add(
                    egui::TextEdit::singleline(&mut tab.filter_sort.where_clause)
                        .hint_text("status = 'active' AND id > 100")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(240.0),
                );
                let is_submitted = filter_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (is_submitted && !is_busy) || ghost_button(ui, p, icon::CHECK, "Apply", !is_busy).clicked() {
                    tab.filter_sort.page_index = 0;
                    reload_page = true;
                }
                toolbar_divider(ui, p);
                let has_previous = tab.filter_sort.page_index > 0 && !is_busy;
                if ui
                    .add_enabled_ui(has_previous, |ui| icon_button(ui, p, icon::CARET_LEFT, "Previous page"))
                    .inner
                    .clicked()
                {
                    tab.filter_sort.page_index -= 1;
                    reload_page = true;
                }
                caption(ui, p, format!("Page {}", tab.filter_sort.page_index + 1));
                let has_more = !is_busy && tab.page.as_ref().map(|pg| pg.has_more_rows).unwrap_or(false);
                if ui
                    .add_enabled_ui(has_more, |ui| icon_button(ui, p, icon::CARET_RIGHT, "Next page"))
                    .inner
                    .clicked()
                {
                    tab.filter_sort.page_index += 1;
                    reload_page = true;
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
            if bound_table.is_some() && !is_read_only {
                toolbar_divider(ui, p);
                if ghost_button(ui, p, icon::PLUS, "Row", true)
                    .on_hover_text("Insert a blank row")
                    .clicked()
                {
                    if let Some(pg) = &tab.page {
                        tab.mutator.insert_blank_row(pg);
                    }
                }
                if let Some((sel_r, _)) = tab.selected_cell {
                    if ghost_button(ui, p, icon::COPY, "Clone", true)
                        .on_hover_text("Clone the selected row")
                        .clicked()
                    {
                        if let Some(pg) = &tab.page {
                            tab.mutator.clone_row(pg, sel_r);
                        }
                    }
                    if tinted_ghost_button(ui, p.danger, icon::TRASH, "Delete", true)
                        .on_hover_text("Toggle delete on the selected row")
                        .clicked()
                    {
                        tab.mutator.toggle_delete_row(sel_r);
                    }
                }
                if tab.mutator.has_changes() {
                    let (deletes, updates, inserts) = tab.mutator.pending_summary();
                    let review_label = format!("Review changes  D{deletes} U{updates} I{inserts}");
                    let can_review = !is_busy && !tab.is_submitting_mutations;
                    if tab.is_submitting_mutations {
                        busy_indicator(ui, p, "Committing…");
                    } else if primary_button(ui, p, icon::FLOPPY_DISK, &review_label, can_review).clicked() {
                        preview_req = true;
                    }
                    if ghost_button(ui, p, icon::ARROW_U_UP_LEFT, "Revert all", can_review).clicked() {
                        tab.mutator.revert_all();
                    }
                }
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

        if reload_page {
            if let (Some(slot), Some((schema, table))) = (active_slot, bound_table.clone()) {
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

        if preview_req {
            if let (Some(slot), Some((schema, table))) = (active_slot, bound_table.clone()) {
                let dialect = self.slot_dialect(Some(slot));
                let pks = self.catalogs[slot.index()]
                    .as_ref()
                    .map(|cat| cat.primary_keys_for(&schema, &table))
                    .unwrap_or_default();
                let tab = &self.tabs[self.active_tab_idx];
                if let Some(page) = &tab.page {
                    let fallback_pks: Vec<String> = if pks.is_empty() {
                        page.columns
                            .iter()
                            .filter(|c| c.is_primary_key)
                            .map(|c| c.name.clone())
                            .collect()
                    } else {
                        pks
                    };
                    match tab.mutator.build_pending_sql(dialect, page, &schema, &table, &fallback_pks) {
                        Ok(batch) if !batch.is_empty() => {
                            self.sql_preview_modal = Some((tab.id, batch.as_script(), batch.statements));
                        }
                        Ok(_) => {}
                        Err(err) => self.toast_banner = Some((err, true)),
                    }
                }
            }
        }

        self.render_grid_body(ui, p, active_slot, bound_table, is_read_only);
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
