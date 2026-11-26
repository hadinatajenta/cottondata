use super::CottonDataApp;
use crate::theme::Palette;
use crate::ui_kit::{
    banner, busy_indicator, caption, field_label, icon, inset_frame, modal_shell, pill, primary_button,
    secondary_button, segmented_tabs, tinted_ghost_button, ModalConfig,
};
use cottondata_core::ServerSlotId;
use cottondata_grid::{
    auto_map_columns, dry_run_import, inspect_import_file, ColumnMapping, DryRunReport, ImportFormat, ImportPreview,
    TargetColumnSpec,
};
use eframe::egui::{self, FontFamily, FontId, RichText};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(super) struct ImportModalState {
    pub(super) slot: ServerSlotId,
    pub(super) schema: String,
    pub(super) table: String,
    pub(super) target_columns: Vec<TargetColumnSpec>,
    pub(super) file_path: Option<PathBuf>,
    pub(super) format: ImportFormat,
    pub(super) preview: Option<ImportPreview>,
    pub(super) mappings: Vec<ColumnMapping>,
    pub(super) batch_size: usize,
    pub(super) dry_run: Option<DryRunReport>,
    pub(super) is_running: bool,
    pub(super) imported_rows: u64,
    pub(super) cancel_flag: Option<Arc<AtomicBool>>,
    pub(super) error_msg: Option<String>,
}

impl ImportModalState {
    pub(super) fn new(
        slot: ServerSlotId,
        schema: String,
        table: String,
        target_columns: Vec<TargetColumnSpec>,
    ) -> Self {
        Self {
            slot,
            schema,
            table,
            target_columns,
            file_path: None,
            format: ImportFormat::Csv,
            preview: None,
            mappings: Vec::new(),
            batch_size: 500,
            dry_run: None,
            is_running: false,
            imported_rows: 0,
            cancel_flag: None,
            error_msg: None,
        }
    }

    fn load_file(&mut self, path: PathBuf, format_override: Option<ImportFormat>) {
        let fmt = format_override.unwrap_or_else(|| ImportFormat::detect_from_path(&path));
        match inspect_import_file(&path, fmt, 5) {
            Ok(preview) => {
                self.mappings = auto_map_columns(&preview.headers, &self.target_columns);
                self.format = fmt;
                self.file_path = Some(path);
                self.preview = Some(preview);
                self.dry_run = None;
                self.error_msg = None;
            }
            Err(err) => {
                self.error_msg = Some(err);
            }
        }
    }
}

fn is_numeric_data_type(data_type: &str) -> bool {
    let lower = data_type.to_ascii_lowercase();
    lower.contains("int")
        || lower.contains("decimal")
        || lower.contains("numeric")
        || lower.contains("float")
        || lower.contains("double")
        || lower.contains("real")
        || lower.contains("money")
        || lower.contains("bit")
}

impl CottonDataApp {
    pub(super) fn open_import_modal(&mut self, slot: ServerSlotId, schema: String, table: String) {
        let target_columns: Vec<TargetColumnSpec> = self.catalogs[slot.index()]
            .as_ref()
            .and_then(|cat| {
                cat.schemas()
                    .iter()
                    .find(|s| cat.resolve(s.name).eq_ignore_ascii_case(&schema))
                    .and_then(|s| {
                        s.tables
                            .iter()
                            .find(|t| cat.resolve(t.name).eq_ignore_ascii_case(&table))
                            .map(|t| {
                                t.columns
                                    .iter()
                                    .map(|c| {
                                        let dtype = cat.resolve(c.data_type).to_string();
                                        TargetColumnSpec {
                                            name: cat.resolve(c.name).to_string(),
                                            is_numeric: is_numeric_data_type(&dtype),
                                            data_type: dtype,
                                            is_nullable: c.is_nullable,
                                            has_default: c.default_expr.is_some() || c.is_auto_increment,
                                        }
                                    })
                                    .collect()
                            })
                    })
            })
            .unwrap_or_default();

        self.import_modal = Some(ImportModalState::new(slot, schema, table, target_columns));
    }

    pub(super) fn render_import_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(mut state) = self.import_modal.take() else {
            return;
        };
        let mut keep_open = true;
        let mut run_dry = false;
        let mut run_import = false;
        let mut cancel_import = false;

        let server_label = self.slot_display_name(state.slot);
        let title = format!("Import Data → {}.{}", state.schema, state.table);
        let subtitle = format!("{server_label} · Streaming batched INSERT inside a single transaction");
        let is_running = state.is_running;
        let imported_rows = state.imported_rows;
        let has_file = state.file_path.is_some();
        let mapped_count = state.mappings.iter().filter(|m| m.target_column.is_some()).count();

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "table_import_modal",
                icon_glyph: icon::UPLOAD_SIMPLE,
                icon_color: p.accent,
                title: &title,
                subtitle: Some(&subtitle),
                accent_bar: Some(p.accent),
                width: 780.0,
                max_height: Some(560.0),
                dismiss_on_backdrop: false,
            },
            |ui| {
                if let Some(err) = &state.error_msg {
                    banner(ui, p, p.danger, icon::WARNING, err, false);
                    ui.add_space(10.0);
                }

                ui.horizontal_wrapped(|ui| {
                    if secondary_button(ui, p, icon::FOLDER_OPEN, "Choose CSV / JSON file…", !is_running).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Data files", &["csv", "tsv", "json", "jsonl", "ndjson"])
                            .pick_file()
                        {
                            state.load_file(path, None);
                        }
                    }
                    if let Some(path) = &state.file_path {
                        let fname = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("file")
                            .to_string();
                        pill(ui, &fname, p.accent);
                        let mut fmt = state.format;
                        segmented_tabs(
                            ui,
                            p,
                            &mut fmt,
                            &[
                                (ImportFormat::Csv, "", "CSV"),
                                (ImportFormat::Tsv, "", "TSV"),
                                (ImportFormat::JsonArray, "", "JSON"),
                                (ImportFormat::JsonLines, "", "JSONL"),
                            ],
                        );
                        if fmt != state.format {
                            let cloned = path.clone();
                            state.load_file(cloned, Some(fmt));
                        }
                    }
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    field_label(ui, p, "Batch Size");
                    ui.add_space(8.0);
                    segmented_tabs(
                        ui,
                        p,
                        &mut state.batch_size,
                        &[(100usize, "", "100"), (500, "", "500"), (1000, "", "1,000")],
                    );
                    caption(ui, p, "rows per INSERT batch");
                });

                if !state.mappings.is_empty() {
                    ui.add_space(12.0);
                    field_label(ui, p, "Column Mapping");
                    inset_frame(p).show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("import_mapping_scroll")
                            .max_height(210.0)
                            .show(ui, |ui| {
                                egui::Grid::new("import_mapping_grid")
                                    .striped(true)
                                    .num_columns(3)
                                    .spacing([12.0, 6.0])
                                    .show(ui, |ui| {
                                        ui.strong("Source Column");
                                        ui.strong("Target Table Column");
                                        ui.strong("Empty String");
                                        ui.end_row();
                                        for mapping in &mut state.mappings {
                                            ui.label(
                                                RichText::new(&mapping.source_name)
                                                    .font(FontId::new(12.0, FontFamily::Monospace))
                                                    .color(p.text_primary),
                                            );
                                            let selected_text = mapping
                                                .target_column
                                                .as_deref()
                                                .unwrap_or("(Skip column)")
                                                .to_string();
                                            egui::ComboBox::from_id_salt(("imp_map", mapping.source_index))
                                                .truncate()
                                                .selected_text(&selected_text)
                                                .width(230.0)
                                                .show_ui(ui, |ui| {
                                                    if ui
                                                        .selectable_label(mapping.target_column.is_none(), "(Skip column)")
                                                        .clicked()
                                                    {
                                                        mapping.target_column = None;
                                                    }
                                                    for tc in &state.target_columns {
                                                        let label = format!("{} ({})", tc.name, tc.data_type);
                                                        let sel = mapping
                                                            .target_column
                                                            .as_deref()
                                                            .is_some_and(|n| n.eq_ignore_ascii_case(&tc.name));
                                                        if ui.selectable_label(sel, label).clicked() {
                                                            mapping.target_column = Some(tc.name.clone());
                                                        }
                                                    }
                                                });
                                            ui.checkbox(&mut mapping.empty_as_null, "Treat as NULL");
                                            ui.end_row();
                                        }
                                    });
                            });
                    });
                }

                if let Some(preview) = &state.preview {
                    if !preview.sample_rows.is_empty() {
                        ui.add_space(10.0);
                        caption(ui, p, format!("Sample preview ({} rows):", preview.sample_rows.len()));
                        inset_frame(p).show(ui, |ui| {
                            egui::ScrollArea::horizontal()
                                .id_salt("import_sample_scroll")
                                .max_height(110.0)
                                .show(ui, |ui| {
                                    egui::Grid::new("import_sample_grid")
                                        .striped(true)
                                        .spacing([10.0, 3.0])
                                        .show(ui, |ui| {
                                            for h in &preview.headers {
                                                ui.strong(super::truncate_display(h, 24));
                                            }
                                            ui.end_row();
                                            for row in &preview.sample_rows {
                                                for cell in row {
                                                    let txt = cell.as_deref().unwrap_or("NULL");
                                                    ui.label(
                                                        RichText::new(super::truncate_display(txt, 28))
                                                            .font(FontId::new(11.0, FontFamily::Monospace))
                                                            .color(if cell.is_none() {
                                                                p.text_muted
                                                            } else {
                                                                p.text_primary
                                                            }),
                                                    );
                                                }
                                                ui.end_row();
                                            }
                                        });
                                });
                        });
                    }
                }

                if let Some(report) = &state.dry_run {
                    ui.add_space(10.0);
                    if report.error_count == 0 {
                        banner(
                            ui,
                            p,
                            p.success,
                            icon::CHECK_CIRCLE,
                            &format!(
                                "Dry-run passed: {} / {} rows valid and ready to import.",
                                report.valid_rows, report.total_rows
                            ),
                            false,
                        );
                    } else {
                        banner(
                            ui,
                            p,
                            p.warning,
                            icon::WARNING,
                            &format!(
                                "Dry-run found {} error(s) across {} rows ({} valid).",
                                report.error_count, report.total_rows, report.valid_rows
                            ),
                            false,
                        );
                        for err in report.sample_errors.iter().take(6) {
                            ui.label(
                                RichText::new(format!("  • {err}"))
                                    .font(FontId::new(11.0, FontFamily::Monospace))
                                    .color(p.danger),
                            );
                        }
                    }
                }
            },
            |ui| {
                if is_running {
                    busy_indicator(ui, p, &format!("Importing {imported_rows} rows…"));
                    if tinted_ghost_button(ui, p.danger, icon::STOP, "Cancel", true).clicked() {
                        cancel_import = true;
                    }
                } else if secondary_button(
                    ui,
                    p,
                    icon::CHECK_CIRCLE,
                    "Dry-Run Validate",
                    has_file && mapped_count > 0,
                )
                .clicked()
                {
                    run_dry = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(
                        ui,
                        p,
                        icon::UPLOAD_SIMPLE,
                        "Import in Transaction",
                        !is_running && has_file && mapped_count > 0,
                    )
                    .clicked()
                    {
                        run_import = true;
                    }
                    if secondary_button(ui, p, icon::X, "Close", !is_running).clicked() {
                        keep_open = false;
                    }
                });
            },
        );

        if outcome.closed && !is_running {
            keep_open = false;
        }
        if cancel_import {
            if let Some(flag) = &state.cancel_flag {
                flag.store(true, Ordering::Relaxed);
            }
        }
        if run_dry {
            if let Some(path) = &state.file_path {
                match dry_run_import(path, state.format, &state.mappings, &state.target_columns) {
                    Ok(report) => {
                        state.dry_run = Some(report);
                        state.error_msg = None;
                    }
                    Err(err) => {
                        state.error_msg = Some(err);
                    }
                }
            }
        }
        if run_import {
            if let (Some(path), Some(slot_entry)) = (&state.file_path, self.registry.get(state.slot)) {
                let profile = slot_entry.profile.clone();
                let pw = self.storage.get_password(&profile.id);
                let cancel = Arc::new(AtomicBool::new(false));
                state.is_running = true;
                state.imported_rows = 0;
                state.error_msg = None;
                state.cancel_flag = Some(Arc::clone(&cancel));
                self.hub.import_file_async(
                    state.slot,
                    profile,
                    pw,
                    state.schema.clone(),
                    state.table.clone(),
                    path.clone(),
                    state.format,
                    state.mappings.clone(),
                    state.target_columns.clone(),
                    state.batch_size,
                    cancel,
                );
            }
        }
        if keep_open {
            self.import_modal = Some(state);
        }
    }
}
