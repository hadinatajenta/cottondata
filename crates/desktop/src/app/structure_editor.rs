use super::CottonDataApp;
use crate::theme::{highlight_mysql_editor_layout, Palette, RADIUS_CONTROL};
use crate::ui_kit::{
    banner, caption, icon, inset_frame, modal_shell, pill, primary_button, secondary_button, segmented_tabs,
    styled_text_input, tinted_ghost_button, ModalConfig,
};
use crate::worker::SlotCmd;
use cottondata_core::{EnvironmentTag, ServerSlotId};
use cottondata_sql::{build_alter_table_statements, ColumnDraft, ForeignKeyDraft, IndexDraft, TableStructureEdit};
use eframe::egui::{self, CornerRadius, Margin, RichText};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StructureTab {
    Columns,
    Indexes,
    ForeignKeys,
    PreviewSql,
}

pub(super) struct StructureEditorState {
    pub(super) slot: ServerSlotId,
    pub(super) schema: String,
    pub(super) table: String,
    pub(super) active_tab: StructureTab,
    pub(super) original_columns: Vec<ColumnDraft>,
    pub(super) columns: Vec<ColumnDraft>,
    pub(super) original_indexes: Vec<IndexDraft>,
    pub(super) indexes: Vec<IndexDraft>,
    pub(super) original_fks: Vec<ForeignKeyDraft>,
    pub(super) fks: Vec<ForeignKeyDraft>,
    pub(super) error_msg: Option<String>,
}

impl StructureEditorState {
    fn to_edit(&self) -> TableStructureEdit {
        TableStructureEdit {
            schema: self.schema.clone(),
            table: self.table.clone(),
            original_columns: self.original_columns.clone(),
            columns: self.columns.clone(),
            original_indexes: self.original_indexes.clone(),
            indexes: self.indexes.clone(),
            original_fks: self.original_fks.clone(),
            fks: self.fks.clone(),
        }
    }
}

impl CottonDataApp {
    pub(super) fn open_structure_editor(&mut self, slot: ServerSlotId, schema: String, table: String) {
        let original_columns: Vec<ColumnDraft> = self.catalogs[slot.index()]
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
                                        let col_name = cat.resolve(c.name).to_string();
                                        ColumnDraft {
                                            original_name: Some(col_name.clone()),
                                            name: col_name,
                                            data_type: cat.resolve(c.column_type).to_string(),
                                            nullable: c.is_nullable,
                                            default_expr: c.default_expr.clone().unwrap_or_default(),
                                            auto_increment: c.is_auto_increment,
                                            comment: c.comment.clone().unwrap_or_default(),
                                            is_dropped: false,
                                        }
                                    })
                                    .collect()
                            })
                    })
            })
            .unwrap_or_default();

        let detail = self.cached_table_detail(slot, &schema, &table).cloned();
        if detail.is_none() {
            self.ensure_table_detail_loaded(slot, &schema, &table);
        }
        let original_indexes: Vec<IndexDraft> = detail
            .as_ref()
            .map(|d| {
                d.indexes
                    .iter()
                    .filter(|i| !i.name.eq_ignore_ascii_case("PRIMARY"))
                    .map(|i| IndexDraft {
                        original_name: Some(i.name.clone()),
                        name: i.name.clone(),
                        columns: i.columns.clone(),
                        unique: !i.non_unique,
                        is_dropped: false,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let original_fks: Vec<ForeignKeyDraft> = detail
            .as_ref()
            .map(|d| {
                d.foreign_keys
                    .iter()
                    .map(|fk| ForeignKeyDraft {
                        original_name: Some(fk.constraint_name.clone()),
                        name: fk.constraint_name.clone(),
                        column: fk.column_name.clone(),
                        ref_schema: fk.ref_schema.clone(),
                        ref_table: fk.ref_table.clone(),
                        ref_column: fk.ref_column.clone(),
                        on_update: fk.update_rule.clone(),
                        on_delete: fk.delete_rule.clone(),
                        is_dropped: false,
                    })
                    .collect()
            })
            .unwrap_or_default();

        self.structure_editor = Some(StructureEditorState {
            slot,
            schema,
            table,
            active_tab: StructureTab::Columns,
            columns: original_columns.clone(),
            original_columns,
            indexes: original_indexes.clone(),
            original_indexes,
            fks: original_fks.clone(),
            original_fks,
            error_msg: None,
        });
    }

    pub(super) fn render_structure_editor_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(mut state) = self.structure_editor.take() else {
            return;
        };
        let dialect = self.slot_dialect(Some(state.slot));
        let is_prod = self
            .registry
            .get(state.slot)
            .is_some_and(|s| s.profile.environment == EnvironmentTag::Production);
        let server_label = self.slot_display_name(state.slot);
        let title = format!("Table Structure · {}.{}", state.schema, state.table);
        let subtitle = format!("{server_label} · Preview ALTER TABLE SQL before applying");

        let preview_result = build_alter_table_statements(dialect, &state.to_edit());
        let stmt_count = preview_result.as_ref().map(|v| v.len()).unwrap_or(0);
        let preview_script = match &preview_result {
            Ok(stmts) if stmts.is_empty() => "-- No structural changes yet.".to_string(),
            Ok(stmts) => stmts.join("\n"),
            Err(err) => format!("-- Validation error: {err}"),
        };

        let mut keep_open = true;
        let mut apply_clicked = false;
        let mut open_console_clicked = false;

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "table_structure_modal",
                icon_glyph: icon::TABLE,
                icon_color: if is_prod { p.danger } else { p.accent },
                title: &title,
                subtitle: Some(&subtitle),
                accent_bar: Some(if is_prod { p.danger } else { p.accent }),
                width: 820.0,
                max_height: Some(560.0),
                dismiss_on_backdrop: false,
            },
            |ui| {
                if is_prod {
                    banner(
                        ui,
                        p,
                        p.danger,
                        icon::WARNING,
                        "Production server target — carefully review ALTER TABLE SQL before executing.",
                        false,
                    );
                    ui.add_space(8.0);
                }
                if let Some(err) = &state.error_msg {
                    banner(ui, p, p.danger, icon::WARNING, err, false);
                    ui.add_space(8.0);
                }

                ui.horizontal(|ui| {
                    segmented_tabs(
                        ui,
                        p,
                        &mut state.active_tab,
                        &[
                            (StructureTab::Columns, icon::ROWS, "Columns"),
                            (StructureTab::Indexes, icon::KEY, "Indexes"),
                            (StructureTab::ForeignKeys, icon::FLOW_ARROW, "Foreign Keys"),
                            (StructureTab::PreviewSql, icon::CODE, "Preview SQL"),
                        ],
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if stmt_count > 0 {
                            pill(ui, &format!("{stmt_count} ALTER statement(s)"), p.warning);
                        }
                    });
                });
                ui.add_space(10.0);

                match state.active_tab {
                    StructureTab::Columns => render_columns_tab(ui, p, &mut state),
                    StructureTab::Indexes => render_indexes_tab(ui, p, &mut state),
                    StructureTab::ForeignKeys => render_fks_tab(ui, p, &mut state),
                    StructureTab::PreviewSql => {
                        let mut buf = preview_script.clone();
                        let palette_copy = p;
                        let mut layouter = move |ui: &egui::Ui, string: &str, _wrap_width: f32| {
                            highlight_mysql_editor_layout(ui, string, palette_copy)
                        };
                        inset_frame(p).show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("structure_sql_preview")
                                .max_height(340.0)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut buf)
                                            .code_editor()
                                            .frame(false)
                                            .layouter(&mut layouter)
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(14),
                                    );
                                });
                        });
                    }
                }
            },
            |ui| {
                if secondary_button(ui, p, icon::TERMINAL_WINDOW, "Open SQL in Console", stmt_count > 0).clicked() {
                    open_console_clicked = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_apply = preview_result.as_ref().is_ok_and(|v| !v.is_empty());
                    if primary_button(ui, p, icon::CHECK_CIRCLE, "Apply ALTER TABLE", can_apply).clicked() {
                        apply_clicked = true;
                    }
                    if secondary_button(ui, p, icon::X, "Close", true).clicked() {
                        keep_open = false;
                    }
                });
            },
        );

        if outcome.closed {
            keep_open = false;
        }

        if open_console_clicked {
            if let Ok(stmts) = &preview_result {
                self.open_console_tab_for_slot(Some(state.slot), Some(stmts.join("\n")));
                keep_open = false;
            }
        }

        if apply_clicked {
            match preview_result {
                Ok(stmts) if !stmts.is_empty() => {
                    let tab_id = self.tabs[self.active_tab_idx].id;
                    if is_prod && state.active_tab != StructureTab::PreviewSql {
                        state.active_tab = StructureTab::PreviewSql;
                        state.error_msg = Some("Review the generated ALTER TABLE SQL on PROD, then click Apply again.".into());
                    } else {
                        self.hub.send(
                            state.slot,
                            SlotCmd::SubmitMutations {
                                tab_id,
                                statements: stmts,
                            },
                        );
                        self.hub.send(
                            state.slot,
                            SlotCmd::LoadSchemaL1 {
                                schema: state.schema.clone(),
                            },
                        );
                        let key = (
                            state.slot.index(),
                            state.schema.to_ascii_lowercase(),
                            state.table.to_ascii_lowercase(),
                        );
                        self.table_details.remove(&key);
                        self.table_details_loading.remove(&key);
                        self.notify_info(format!("Applying ALTER TABLE to {}.{}…", state.schema, state.table));
                        keep_open = false;
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    state.error_msg = Some(err);
                }
            }
        }

        if keep_open {
            self.structure_editor = Some(state);
        }
    }
}

fn render_columns_tab(ui: &mut egui::Ui, p: Palette, state: &mut StructureEditorState) {
    if secondary_button(ui, p, icon::PLUS, "Add Column", true).clicked() {
        state.columns.push(ColumnDraft {
            original_name: None,
            name: String::new(),
            data_type: "varchar(255)".into(),
            nullable: true,
            default_expr: String::new(),
            auto_increment: false,
            comment: String::new(),
            is_dropped: false,
        });
    }
    ui.add_space(8.0);

    let mut remove_new_idx: Option<usize> = None;
    inset_frame(p).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("struct_cols_scroll")
            .max_height(330.0)
            .show(ui, |ui| {
                for (idx, col) in state.columns.iter_mut().enumerate() {
                    let row_bg = if col.is_dropped {
                        p.cell_deleted
                    } else if col.original_name.is_none() {
                        p.cell_inserted
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    egui::Frame::new()
                        .fill(row_bg)
                        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                        .inner_margin(Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(!col.is_dropped, |ui| {
                                    styled_text_input(ui, p, ("sc_name", idx), &mut col.name, "column_name", 155.0);
                                    styled_text_input(ui, p, ("sc_type", idx), &mut col.data_type, "varchar(100)", 140.0);
                                    styled_text_input(ui, p, ("sc_def", idx), &mut col.default_expr, "default", 110.0);
                                    ui.checkbox(&mut col.nullable, "NULL");
                                    ui.checkbox(&mut col.auto_increment, "AI");
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if col.original_name.is_none() {
                                        if tinted_ghost_button(ui, p.danger, icon::TRASH, "", true)
                                            .on_hover_text("Remove new column")
                                            .clicked()
                                        {
                                            remove_new_idx = Some(idx);
                                        }
                                    } else {
                                        let (glyph, tip, color) = if col.is_dropped {
                                            (icon::ARROW_U_UP_LEFT, "Undo drop column", p.warning)
                                        } else {
                                            (icon::TRASH, "Drop column", p.danger)
                                        };
                                        if tinted_ghost_button(ui, color, glyph, "", true)
                                            .on_hover_text(tip)
                                            .clicked()
                                        {
                                            col.is_dropped = !col.is_dropped;
                                        }
                                    }
                                });
                            });
                        });
                    ui.add_space(3.0);
                }
            });
    });
    if let Some(idx) = remove_new_idx {
        state.columns.remove(idx);
    }
}

fn render_indexes_tab(ui: &mut egui::Ui, p: Palette, state: &mut StructureEditorState) {
    if secondary_button(ui, p, icon::PLUS, "Add Index", true).clicked() {
        state.indexes.push(IndexDraft {
            original_name: None,
            name: format!("idx_{}_", state.table),
            columns: Vec::new(),
            unique: false,
            is_dropped: false,
        });
    }
    ui.add_space(8.0);
    if state.indexes.is_empty() {
        caption(ui, p, "No secondary indexes on this table yet.");
        return;
    }

    let mut remove_new_idx: Option<usize> = None;
    inset_frame(p).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("struct_idx_scroll")
            .max_height(320.0)
            .show(ui, |ui| {
                for (i, index) in state.indexes.iter_mut().enumerate() {
                    let row_bg = if index.is_dropped {
                        p.cell_deleted
                    } else if index.original_name.is_none() {
                        p.cell_inserted
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    egui::Frame::new()
                        .fill(row_bg)
                        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                        .inner_margin(Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(!index.is_dropped, |ui| {
                                    styled_text_input(ui, p, ("si_name", i), &mut index.name, "idx_name", 200.0);
                                    let mut cols_csv = index.columns.join(", ");
                                    if styled_text_input(
                                        ui,
                                        p,
                                        ("si_cols", i),
                                        &mut cols_csv,
                                        "col1, col2",
                                        250.0,
                                    )
                                    .changed()
                                    {
                                        index.columns = cols_csv
                                            .split(',')
                                            .map(|s| s.trim().to_string())
                                            .filter(|s| !s.is_empty())
                                            .collect();
                                    }
                                    ui.checkbox(&mut index.unique, "UNIQUE");
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if index.original_name.is_none() {
                                        if tinted_ghost_button(ui, p.danger, icon::TRASH, "", true).clicked() {
                                            remove_new_idx = Some(i);
                                        }
                                    } else if tinted_ghost_button(
                                        ui,
                                        if index.is_dropped { p.warning } else { p.danger },
                                        if index.is_dropped { icon::ARROW_U_UP_LEFT } else { icon::TRASH },
                                        "",
                                        true,
                                    )
                                    .clicked()
                                    {
                                        index.is_dropped = !index.is_dropped;
                                    }
                                });
                            });
                        });
                    ui.add_space(3.0);
                }
            });
    });
    if let Some(idx) = remove_new_idx {
        state.indexes.remove(idx);
    }
}

fn render_fks_tab(ui: &mut egui::Ui, p: Palette, state: &mut StructureEditorState) {
    if secondary_button(ui, p, icon::PLUS, "Add Foreign Key", true).clicked() {
        state.fks.push(ForeignKeyDraft {
            original_name: None,
            name: format!("fk_{}_", state.table),
            column: String::new(),
            ref_schema: state.schema.clone(),
            ref_table: String::new(),
            ref_column: "id".into(),
            on_update: "RESTRICT".into(),
            on_delete: "RESTRICT".into(),
            is_dropped: false,
        });
    }
    ui.add_space(8.0);
    if state.fks.is_empty() {
        caption(ui, p, "No foreign keys defined on this table.");
        return;
    }

    let mut remove_new_idx: Option<usize> = None;
    inset_frame(p).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("struct_fk_scroll")
            .max_height(320.0)
            .show(ui, |ui| {
                for (i, fk) in state.fks.iter_mut().enumerate() {
                    let row_bg = if fk.is_dropped {
                        p.cell_deleted
                    } else if fk.original_name.is_none() {
                        p.cell_inserted
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    egui::Frame::new()
                        .fill(row_bg)
                        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                        .inner_margin(Margin::symmetric(6, 5))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(!fk.is_dropped, |ui| {
                                    styled_text_input(ui, p, ("sfk_name", i), &mut fk.name, "fk_name", 150.0);
                                    styled_text_input(ui, p, ("sfk_col", i), &mut fk.column, "column", 120.0);
                                    ui.label(RichText::new("→").color(p.text_muted));
                                    styled_text_input(ui, p, ("sfk_rtbl", i), &mut fk.ref_table, "ref_table", 140.0);
                                    styled_text_input(ui, p, ("sfk_rcol", i), &mut fk.ref_column, "ref_col", 110.0);
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if fk.original_name.is_none() {
                                        if tinted_ghost_button(ui, p.danger, icon::TRASH, "", true).clicked() {
                                            remove_new_idx = Some(i);
                                        }
                                    } else if tinted_ghost_button(
                                        ui,
                                        if fk.is_dropped { p.warning } else { p.danger },
                                        if fk.is_dropped { icon::ARROW_U_UP_LEFT } else { icon::TRASH },
                                        "",
                                        true,
                                    )
                                    .clicked()
                                    {
                                        fk.is_dropped = !fk.is_dropped;
                                    }
                                });
                            });
                        });
                    ui.add_space(3.0);
                }
            });
    });
    if let Some(idx) = remove_new_idx {
        state.fks.remove(idx);
    }
}
