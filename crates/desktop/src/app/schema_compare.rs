use super::CottonDataApp;
use crate::theme::{highlight_mysql_editor_layout, Palette, RADIUS_CONTROL};
use crate::ui_kit::{
    banner, busy_indicator, caption, field_label, icon, inset_frame, modal_shell, pill, primary_button,
    secondary_button, segmented_tabs, ModalConfig,
};
use cottondata_core::ServerSlotId;
use cottondata_introspect::{generate_migration_sql, SchemaDiff};
use eframe::egui::{self, CornerRadius, FontFamily, FontId, Margin, RichText};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CompareViewTab {
    Summary,
    MigrationSql,
}

pub(super) struct SchemaCompareState {
    pub(super) source_slot: Option<ServerSlotId>,
    pub(super) source_schema: String,
    pub(super) target_slot: Option<ServerSlotId>,
    pub(super) target_schema: String,
    pub(super) include_drops: bool,
    pub(super) view_tab: CompareViewTab,
    pub(super) is_comparing: bool,
    pub(super) diff: Option<SchemaDiff>,
    pub(super) error_msg: Option<String>,
}

impl SchemaCompareState {
    pub(super) fn new(
        source_slot: Option<ServerSlotId>,
        source_schema: String,
        target_slot: Option<ServerSlotId>,
        target_schema: String,
    ) -> Self {
        Self {
            source_slot,
            source_schema,
            target_slot,
            target_schema,
            include_drops: false,
            view_tab: CompareViewTab::Summary,
            is_comparing: false,
            diff: None,
            error_msg: None,
        }
    }
}

impl CottonDataApp {
    pub(super) fn open_schema_compare(&mut self, source_slot: Option<ServerSlotId>, source_schema: String) {
        let other_slot = self
            .registry
            .iter()
            .map(|s| s.slot_id)
            .find(|&id| Some(id) != source_slot)
            .or(source_slot);
        let target_schema = other_slot
            .and_then(|slot| {
                self.registry
                    .get(slot)
                    .and_then(|s| s.runtime.as_ref())
                    .and_then(|r| r.current_schema.clone())
                    .or_else(|| self.registry.get(slot).and_then(|s| s.profile.default_schema.clone()))
            })
            .unwrap_or_else(|| source_schema.clone());
        self.schema_compare = Some(SchemaCompareState::new(
            source_slot,
            source_schema,
            other_slot,
            target_schema,
        ));
    }

    pub(super) fn render_schema_compare_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(mut state) = self.schema_compare.take() else {
            return;
        };
        let mut keep_open = true;
        let mut run_compare = false;
        let mut open_console_sql: Option<(Option<ServerSlotId>, String)> = None;
        let mut copy_sql: Option<String> = None;

        let server_choices: Vec<(ServerSlotId, String)> = self
            .registry
            .iter()
            .map(|s| (s.slot_id, format!("{} {}", s.slot_id, s.profile.name)))
            .collect();
        let schemas_for = |slot: Option<ServerSlotId>| -> Vec<String> {
            let Some(sid) = slot else {
                return Vec::new();
            };
            self.catalogs[sid.index()]
                .as_ref()
                .map(|cat| {
                    cat.schemas()
                        .iter()
                        .filter(|s| !s.is_system || self.show_system_schemas)
                        .map(|s| cat.resolve(s.name).to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        let source_schemas = schemas_for(state.source_slot);
        let target_schemas = schemas_for(state.target_slot);
        let target_dialect = self.slot_dialect(state.target_slot);

        let migration_sql = state.diff.as_ref().map(|diff| {
            generate_migration_sql(diff, &state.target_schema, target_dialect, state.include_drops)
        });
        let has_migration = migration_sql.as_ref().is_some_and(|s| !s.trim().is_empty());
        let open_target_slot = state.target_slot;

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "schema_compare_modal",
                icon_glyph: icon::FLOW_ARROW,
                icon_color: p.accent,
                title: "Schema Compare",
                subtitle: Some("Diff tables, columns, indexes, and foreign keys between two servers or schemas"),
                accent_bar: Some(p.accent),
                width: 860.0,
                max_height: Some(580.0),
                dismiss_on_backdrop: false,
            },
            |ui| {
                if let Some(err) = &state.error_msg {
                    banner(ui, p, p.danger, icon::WARNING, err, false);
                    ui.add_space(10.0);
                }

                let full_w = ui.available_width();
                let side_w = (full_w - 130.0) / 2.0;
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(side_w);
                        field_label(ui, p, "Source (Reference · e.g. DEV)");
                        let src_srv_label = state
                            .source_slot
                            .and_then(|id| server_choices.iter().find(|(s, _)| *s == id))
                            .map(|(_, l)| l.as_str())
                            .unwrap_or("Choose server");
                        egui::ComboBox::from_id_salt("cmp_src_server")
                            .truncate()
                            .selected_text(src_srv_label)
                            .width(side_w)
                            .show_ui(ui, |ui| {
                                for (sid, label) in &server_choices {
                                    ui.selectable_value(&mut state.source_slot, Some(*sid), label);
                                }
                            });
                        ui.add_space(4.0);
                        render_schema_selector(ui, "cmp_src_schema", &mut state.source_schema, &source_schemas, side_w);
                    });

                    ui.vertical_centered(|ui| {
                        ui.set_width(114.0);
                        ui.add_space(22.0);
                        let can_cmp = state.source_slot.is_some()
                            && state.target_slot.is_some()
                            && !state.source_schema.trim().is_empty()
                            && !state.target_schema.trim().is_empty()
                            && !state.is_comparing;
                        if primary_button(ui, p, icon::ARROWS_CLOCKWISE, "Compare", can_cmp).clicked() {
                            run_compare = true;
                        }
                        if state.is_comparing {
                            ui.add_space(4.0);
                            busy_indicator(ui, p, "Diffing…");
                        }
                    });

                    ui.vertical(|ui| {
                        ui.set_width(side_w);
                        field_label(ui, p, "Target (Migrate · e.g. STAGING)");
                        let dst_srv_label = state
                            .target_slot
                            .and_then(|id| server_choices.iter().find(|(s, _)| *s == id))
                            .map(|(_, l)| l.as_str())
                            .unwrap_or("Choose server");
                        egui::ComboBox::from_id_salt("cmp_dst_server")
                            .truncate()
                            .selected_text(dst_srv_label)
                            .width(side_w)
                            .show_ui(ui, |ui| {
                                for (sid, label) in &server_choices {
                                    ui.selectable_value(&mut state.target_slot, Some(*sid), label);
                                }
                            });
                        ui.add_space(4.0);
                        render_schema_selector(ui, "cmp_dst_schema", &mut state.target_schema, &target_schemas, side_w);
                    });
                });

                ui.add_space(12.0);

                if let Some(diff) = &state.diff {
                    ui.horizontal_wrapped(|ui| {
                        pill(ui, &format!("+{} new", diff.only_in_source.len()), p.success);
                        pill(ui, &format!("~{} modified", diff.modified_tables.len()), p.warning);
                        pill(ui, &format!("-{} extra in target", diff.only_in_target.len()), p.danger);
                        pill(ui, &format!("={} identical", diff.identical_count), p.text_muted);

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.checkbox(&mut state.include_drops, "Include DROP statements");
                            segmented_tabs(
                                ui,
                                p,
                                &mut state.view_tab,
                                &[
                                    (CompareViewTab::Summary, icon::ROWS, "Diff Summary"),
                                    (CompareViewTab::MigrationSql, icon::CODE, "Migration SQL"),
                                ],
                            );
                        });
                    });
                    ui.add_space(8.0);

                    match state.view_tab {
                        CompareViewTab::Summary => {
                            if !diff.has_differences() {
                                banner(
                                    ui,
                                    p,
                                    p.success,
                                    icon::CHECK_CIRCLE,
                                    "Both schemas are structurally identical.",
                                    false,
                                );
                            } else {
                                inset_frame(p).show(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .id_salt("schema_diff_summary_scroll")
                                        .max_height(330.0)
                                        .show(ui, |ui| {
                                            for t in &diff.only_in_source {
                                                render_diff_card(
                                                    ui,
                                                    p,
                                                    p.success,
                                                    "NEW TABLE",
                                                    &t.name,
                                                    &format!("{} columns (present in source only)", t.columns.len()),
                                                );
                                            }
                                            for td in &diff.modified_tables {
                                                egui::Frame::new()
                                                    .fill(p.bg_card)
                                                    .stroke(egui::Stroke::new(1.0, p.border))
                                                    .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                                                    .inner_margin(Margin::symmetric(10, 7))
                                                    .show(ui, |ui| {
                                                        ui.set_min_width(ui.available_width());
                                                        ui.horizontal(|ui| {
                                                            pill(ui, "MODIFIED", p.warning);
                                                            ui.label(
                                                                RichText::new(&td.table_name)
                                                                    .strong()
                                                                    .color(p.text_primary),
                                                            );
                                                            caption(ui, p, format!("{} change(s)", td.change_count()));
                                                        });
                                                        for c in &td.added_columns {
                                                            ui.label(
                                                                RichText::new(format!("  + COLUMN {} {}", c.name, c.column_type))
                                                                    .font(FontId::new(11.5, FontFamily::Monospace))
                                                                    .color(p.success),
                                                            );
                                                        }
                                                        for (src_c, dst_c) in &td.altered_columns {
                                                            ui.label(
                                                                RichText::new(format!(
                                                                    "  ~ COLUMN {} : {} → {}",
                                                                    src_c.name, dst_c.column_type, src_c.column_type
                                                                ))
                                                                .font(FontId::new(11.5, FontFamily::Monospace))
                                                                .color(p.warning),
                                                            );
                                                        }
                                                        for c in &td.removed_columns {
                                                            ui.label(
                                                                RichText::new(format!("  - COLUMN {} {}", c.name, c.column_type))
                                                                    .font(FontId::new(11.5, FontFamily::Monospace))
                                                                    .color(p.danger),
                                                            );
                                                        }
                                                        for idx in &td.added_indexes {
                                                            ui.label(
                                                                RichText::new(format!(
                                                                    "  + INDEX {} ({})",
                                                                    idx.name,
                                                                    idx.columns.join(", ")
                                                                ))
                                                                .font(FontId::new(11.5, FontFamily::Monospace))
                                                                .color(p.success),
                                                            );
                                                        }
                                                        for idx in &td.removed_indexes {
                                                            ui.label(
                                                                RichText::new(format!("  - INDEX {}", idx.name))
                                                                    .font(FontId::new(11.5, FontFamily::Monospace))
                                                                    .color(p.danger),
                                                            );
                                                        }
                                                        for fk in &td.added_fks {
                                                            ui.label(
                                                                RichText::new(format!(
                                                                    "  + FK {} ({} → {}.{})",
                                                                    fk.constraint_name,
                                                                    fk.column_name,
                                                                    fk.ref_table,
                                                                    fk.ref_column
                                                                ))
                                                                .font(FontId::new(11.5, FontFamily::Monospace))
                                                                .color(p.success),
                                                            );
                                                        }
                                                        for fk in &td.removed_fks {
                                                            ui.label(
                                                                RichText::new(format!("  - FK {}", fk.constraint_name))
                                                                    .font(FontId::new(11.5, FontFamily::Monospace))
                                                                    .color(p.danger),
                                                            );
                                                        }
                                                    });
                                                ui.add_space(4.0);
                                            }
                                            for t in &diff.only_in_target {
                                                render_diff_card(
                                                    ui,
                                                    p,
                                                    p.danger,
                                                    "EXTRA IN TARGET",
                                                    &t.name,
                                                    &format!("{} columns (not in source)", t.columns.len()),
                                                );
                                            }
                                        });
                                });
                            }
                        }
                        CompareViewTab::MigrationSql => {
                            let mut buf = migration_sql.clone().unwrap_or_default();
                            let palette_copy = p;
                            let mut layouter = move |ui: &egui::Ui, string: &str, _wrap_width: f32| {
                                highlight_mysql_editor_layout(ui, string, palette_copy)
                            };
                            inset_frame(p).show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("schema_diff_sql_scroll")
                                    .max_height(330.0)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::multiline(&mut buf)
                                                .code_editor()
                                                .frame(false)
                                                .layouter(&mut layouter)
                                                .desired_width(f32::INFINITY)
                                                .desired_rows(15),
                                        );
                                    });
                            });
                        }
                    }
                } else if !state.is_comparing {
                    caption(
                        ui,
                        p,
                        "Select a Source (reference) and Target server + schema above, then click Compare.",
                    );
                }
            },
            |ui| {
                if secondary_button(ui, p, icon::COPY, "Copy Migration SQL", has_migration).clicked() {
                    copy_sql = migration_sql.clone();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, p, icon::TERMINAL_WINDOW, "Open in SQL Console", has_migration).clicked() {
                        if let Some(sql) = migration_sql.clone() {
                            open_console_sql = Some((open_target_slot, sql));
                        }
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
        if let Some(sql) = copy_sql {
            ctx.copy_text(sql);
            self.notify_success("Copied migration SQL");
        }
        if let Some((slot, sql)) = open_console_sql {
            self.open_console_tab_for_slot(slot, Some(sql));
            keep_open = false;
        }
        if run_compare {
            if let (Some(src_slot), Some(dst_slot)) = (state.source_slot, state.target_slot) {
                if let (Some(src_s), Some(dst_s)) = (self.registry.get(src_slot), self.registry.get(dst_slot)) {
                    let src_profile = src_s.profile.clone();
                    let dst_profile = dst_s.profile.clone();
                    let src_pw = self.storage.get_password(&src_profile.id);
                    let dst_pw = self.storage.get_password(&dst_profile.id);
                    state.is_comparing = true;
                    state.error_msg = None;
                    state.diff = None;
                    self.hub.compare_schemas_async(
                        src_profile,
                        src_pw,
                        state.source_schema.trim().to_string(),
                        dst_profile,
                        dst_pw,
                        state.target_schema.trim().to_string(),
                    );
                }
            }
        }
        if keep_open {
            self.schema_compare = Some(state);
        }
    }
}

fn render_schema_selector(ui: &mut egui::Ui, id: &str, value: &mut String, options: &[String], width: f32) {
    if options.is_empty() {
        ui.add(
            egui::TextEdit::singleline(value)
                .id(egui::Id::new(id))
                .hint_text("Schema / database name")
                .desired_width(width),
        );
    } else {
        let selected = if value.is_empty() {
            "Choose schema…"
        } else {
            value.as_str()
        };
        egui::ComboBox::from_id_salt(id)
            .truncate()
            .selected_text(selected)
            .width(width)
            .show_ui(ui, |ui| {
                for opt in options {
                    ui.selectable_value(value, opt.clone(), opt);
                }
            });
    }
}

fn render_diff_card(
    ui: &mut egui::Ui,
    p: Palette,
    tone: egui::Color32,
    badge: &str,
    table_name: &str,
    summary: &str,
) {
    egui::Frame::new()
        .fill(p.bg_card)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                pill(ui, badge, tone);
                ui.label(RichText::new(table_name).strong().color(p.text_primary));
                caption(ui, p, summary);
            });
        });
    ui.add_space(4.0);
}
