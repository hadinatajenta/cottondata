use super::row_selection::RowSelectMode;
use super::CottonDataApp;
use crate::column_glyph::column_type_glyph;
use crate::theme::{Palette, RADIUS_CONTROL};
use crate::ui_kit::{
    busy_indicator, caption, ghost_button, icon, icon_button, icon_text, inset_frame, pill, secondary_button,
};
use cottondata_grid::{CellValue, Editability};
use eframe::egui::{self, CornerRadius, FontFamily, FontId, Margin, RichText};

struct RecordField {
    col_idx: usize,
    name: String,
    type_name: String,
    is_pk: bool,
    is_nullable: bool,
    value: CellValue,
    is_modified: bool,
    truncated: Option<usize>,
}

impl CottonDataApp {
    pub(super) fn render_record_view_drawer(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        available_h: f32,
        editability: &Editability,
    ) -> Option<(usize, usize)> {
        let mut load_full_req = None;
        let mut copy_toast: Option<String> = None;
        let mut close_drawer = false;
        let mut copy_json_req = false;

        inset_frame(p).inner_margin(Margin::same(10)).show(ui, |ui| {
            ui.set_min_height((available_h - 4.0).max(160.0));
            let tab = &mut self.tabs[self.active_tab_idx];
            let total_rows = tab.total_grid_rows();
            let Some(mut state) = tab.record_view.clone() else {
                return;
            };
            if total_rows == 0 {
                close_drawer = true;
                return;
            }
            if state.row >= total_rows {
                state.row = total_rows - 1;
            }
            let row = state.row;

            ui.horizontal(|ui| {
                ui.label(icon_text(
                    icon::ROWS,
                    p.accent,
                    &format!("Row {} / {total_rows}", row + 1),
                    p.text_primary,
                    13.5,
                ));
                if ui
                    .add_enabled_ui(row > 0, |ui| icon_button(ui, p, icon::CARET_LEFT, "Previous row"))
                    .inner
                    .clicked()
                {
                    state.row -= 1;
                    tab.row_selection.select(state.row, RowSelectMode::Replace);
                    tab.select_cell(state.row, 0);
                }
                if ui
                    .add_enabled_ui(row + 1 < total_rows, |ui| icon_button(ui, p, icon::CARET_RIGHT, "Next row"))
                    .inner
                    .clicked()
                {
                    state.row += 1;
                    tab.row_selection.select(state.row, RowSelectMode::Replace);
                    tab.select_cell(state.row, 0);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, p, icon::X, "Close record view").clicked() {
                        close_drawer = true;
                    }
                    if icon_button(ui, p, icon::COPY, "Copy row as JSON").clicked() {
                        copy_json_req = true;
                    }
                });
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.filter)
                        .hint_text("Filter columns…")
                        .desired_width(165.0),
                );
                ui.toggle_value(&mut state.hide_nulls, RichText::new("Hide NULL").size(11.0));
                ui.toggle_value(&mut state.pretty_json, RichText::new("JSON").size(11.0))
                    .on_hover_text("Pretty-print JSON values");
            });
            ui.separator();

            let filter_lower = state.filter.trim().to_ascii_lowercase();
            let row_for_fields = state.row;
            let fields: Vec<RecordField> = if let Some(page) = &tab.page {
                let is_existing = row_for_fields < page.row_count;
                page.columns
                    .iter()
                    .enumerate()
                    .filter_map(|(col_idx, col)| {
                        let val = tab.cell_value(row_for_fields, col_idx)?;
                        if state.hide_nulls && val.is_null() {
                            return None;
                        }
                        if !filter_lower.is_empty()
                            && !col.name.to_ascii_lowercase().contains(&filter_lower)
                            && !val.as_display_str().to_ascii_lowercase().contains(&filter_lower)
                        {
                            return None;
                        }
                        let is_modified = is_existing && tab.mutator.get_modified(row_for_fields, col_idx).is_some();
                        let truncated = if is_existing && !is_modified {
                            page.cell_truncated_bytes(row_for_fields, col_idx)
                        } else {
                            None
                        };
                        Some(RecordField {
                            col_idx,
                            name: col.name.clone(),
                            type_name: col.type_name.clone(),
                            is_pk: col.is_primary_key,
                            is_nullable: col.is_nullable,
                            value: val,
                            is_modified,
                            truncated,
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };

            let mut staged_updates: Vec<(usize, CellValue)> = Vec::new();
            let mut revert_cols: Vec<usize> = Vec::new();
            let is_loading_full = tab.is_loading_full_cell;
            let tab_id = tab.id;

            egui::ScrollArea::vertical()
                .id_salt(("record_view_scroll", tab_id))
                .max_height((available_h - 102.0).max(100.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if fields.is_empty() {
                        caption(ui, p, "No matching columns for current filter.");
                        return;
                    }
                    for field in &fields {
                        let col_idx = field.col_idx;
                        let col_name = &field.name;
                        let col_type = &field.type_name;
                        let is_pk = field.is_pk;
                        let is_nullable = field.is_nullable;
                        let val = &field.value;
                        let is_modified = field.is_modified;
                        let truncated = field.truncated;
                        let can_edit = editability.is_column_editable(col_idx) && truncated.is_none();
                        let bg = if is_modified {
                            p.cell_modified.gamma_multiply(0.45)
                        } else {
                            p.bg_card
                        };
                        egui::Frame::new()
                            .fill(bg)
                            .stroke(egui::Stroke::new(1.0, p.divider))
                            .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                            .inner_margin(Margin::symmetric(8, 5))
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let (glyph, gcolor) = if is_pk {
                                        (icon::KEY, p.warning)
                                    } else {
                                        (column_type_glyph(col_type), p.text_muted)
                                    };
                                    ui.label(icon_text(glyph, gcolor, col_name, p.text_primary, 12.5));
                                    ui.label(
                                        RichText::new(col_type)
                                            .font(FontId::new(10.5, FontFamily::Monospace))
                                            .color(p.text_muted),
                                    );
                                    if is_modified {
                                        pill(ui, "EDITED", p.warning);
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if icon_button(ui, p, icon::COPY, "Copy value").clicked() {
                                            let txt = if val.is_null() {
                                                "NULL".to_string()
                                            } else {
                                                val.as_display_str().to_string()
                                            };
                                            ui.ctx().copy_text(txt);
                                            copy_toast = Some(format!("Copied `{col_name}`"));
                                        }
                                        if is_modified && icon_button(ui, p, icon::ARROW_U_UP_LEFT, "Revert").clicked()
                                        {
                                            revert_cols.push(col_idx);
                                        }
                                        if can_edit
                                            && is_nullable
                                            && !val.is_null()
                                            && icon_button(ui, p, icon::X_CIRCLE, "Set NULL").clicked()
                                        {
                                            staged_updates.push((col_idx, CellValue::Null));
                                        }
                                    });
                                });

                                if let Some(tot) = truncated {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("Truncated preview (total {tot} B)"))
                                                .size(10.5)
                                                .color(p.warning),
                                        );
                                        if is_loading_full {
                                            busy_indicator(ui, p, "Loading…");
                                        } else if editability.is_editable()
                                            && ghost_button(ui, p, icon::ARROW_CLOCKWISE, "Load full", true).clicked()
                                        {
                                            load_full_req = Some((row_for_fields, col_idx));
                                        }
                                    });
                                }

                                let raw_display = if val.is_null() {
                                    String::new()
                                } else if state.pretty_json {
                                    try_pretty_json(val.as_display_str())
                                        .unwrap_or_else(|| val.as_display_str().to_string())
                                } else {
                                    val.as_display_str().to_string()
                                };
                                let mut buf = raw_display.clone();
                                let is_multiline = buf.contains('\n') || buf.len() > 80;
                                let desired_rows = buf.lines().count().clamp(2, 8);
                                let resp = if is_multiline {
                                    ui.add_enabled(
                                        can_edit,
                                        egui::TextEdit::multiline(&mut buf)
                                            .id(egui::Id::new(("rec_field", tab_id, row_for_fields, col_idx)))
                                            .font(FontId::new(12.0, FontFamily::Monospace))
                                            .hint_text(if val.is_null() { "NULL" } else { "" })
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(desired_rows),
                                    )
                                } else {
                                    ui.add_enabled(
                                        can_edit,
                                        egui::TextEdit::singleline(&mut buf)
                                            .id(egui::Id::new(("rec_field", tab_id, row_for_fields, col_idx)))
                                            .font(FontId::new(12.0, FontFamily::Monospace))
                                            .hint_text(if val.is_null() { "NULL" } else { "" })
                                            .desired_width(f32::INFINITY),
                                    )
                                };
                                if can_edit && resp.changed() && buf != raw_display {
                                    staged_updates.push((col_idx, CellValue::Text(buf)));
                                }
                            });
                        ui.add_space(4.0);
                    }
                });

            for (col_idx, new_val) in staged_updates {
                tab.stage_cell_value(row_for_fields, col_idx, new_val);
            }
            for col_idx in revert_cols {
                tab.mutator.revert_cell(row_for_fields, col_idx);
            }
            if close_drawer {
                tab.record_view = None;
            } else {
                tab.record_view = Some(state);
            }
        });

        if copy_json_req {
            self.copy_record_row_as_json(ui.ctx());
        }
        if let Some(msg) = copy_toast {
            self.tabs[self.active_tab_idx].last_status_msg = msg;
        }
        load_full_req
    }

    fn copy_record_row_as_json(&mut self, ctx: &egui::Context) {
        let tab = &mut self.tabs[self.active_tab_idx];
        let (Some(state), Some(page)) = (&tab.record_view, &tab.page) else {
            return;
        };
        let mut map = serde_json::Map::new();
        for (col_idx, col) in page.columns.iter().enumerate() {
            let val = tab.cell_value(state.row, col_idx).unwrap_or(CellValue::Null);
            let json_val = if val.is_null() {
                serde_json::Value::Null
            } else if col.is_numeric {
                val.as_display_str()
                    .trim()
                    .parse::<i64>()
                    .map(serde_json::Value::from)
                    .unwrap_or_else(|_| serde_json::Value::String(val.as_display_str().to_string()))
            } else {
                serde_json::Value::String(val.as_display_str().to_string())
            };
            map.insert(col.name.clone(), json_val);
        }
        if let Ok(pretty) = serde_json::to_string_pretty(&serde_json::Value::Object(map)) {
            ctx.copy_text(pretty);
            tab.last_status_msg = format!("Copied Row {} as JSON", state.row + 1);
        }
    }

    pub(super) fn render_row_diff_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some((row_a, row_b, mut only_diffs)) = self.tabs[self.active_tab_idx].diff_rows else {
            return;
        };
        let mut open = true;
        let mut close_clicked = false;
        let rows_data: Vec<(String, String, String, String, bool)> = {
            let tab = &self.tabs[self.active_tab_idx];
            if let Some(page) = &tab.page {
                page.columns
                    .iter()
                    .enumerate()
                    .map(|(col_idx, col)| {
                        let va = tab
                            .cell_value(row_a, col_idx)
                            .map(|v| v.as_display_str().to_string())
                            .unwrap_or_else(|| "NULL".into());
                        let vb = tab
                            .cell_value(row_b, col_idx)
                            .map(|v| v.as_display_str().to_string())
                            .unwrap_or_else(|| "NULL".into());
                        let differs = va != vb;
                        (col.name.clone(), col.type_name.clone(), va, vb, differs)
                    })
                    .collect()
            } else {
                Vec::new()
            }
        };
        let diff_count = rows_data.iter().filter(|(_, _, _, _, d)| *d).count();

        let title = format!("Compare Rows: #{} vs #{}", row_a + 1, row_b + 1);
        let subtitle = format!("{diff_count} of {} columns differ", rows_data.len());
        let filter_only_diffs = only_diffs;
        let outcome = crate::ui_kit::modal_shell(
            ctx,
            p,
            crate::ui_kit::ModalConfig {
                id: "row_diff_modal",
                icon_glyph: icon::ROWS,
                icon_color: p.accent,
                title: &title,
                subtitle: Some(&subtitle),
                accent_bar: None,
                width: 760.0,
                max_height: Some(460.0),
                dismiss_on_backdrop: true,
            },
            |ui| {
                egui::Grid::new("row_diff_grid")
                    .striped(true)
                    .num_columns(3)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        ui.strong("Column");
                        ui.strong(format!("Row #{}", row_a + 1));
                        ui.strong(format!("Row #{}", row_b + 1));
                        ui.end_row();
                        for (name, ctype, va, vb, differs) in &rows_data {
                            if filter_only_diffs && !*differs {
                                continue;
                            }
                            ui.label(
                                RichText::new(format!("{name} ({ctype})"))
                                    .font(FontId::new(12.0, FontFamily::Monospace))
                                    .color(if *differs { p.accent } else { p.text_muted }),
                            );
                            let color_a = if *differs { p.danger } else { p.text_primary };
                            let color_b = if *differs { p.success } else { p.text_primary };
                            ui.add(
                                egui::Label::new(
                                    RichText::new(va)
                                        .font(FontId::new(12.0, FontFamily::Monospace))
                                        .color(color_a),
                                )
                                .wrap(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(vb)
                                        .font(FontId::new(12.0, FontFamily::Monospace))
                                        .color(color_b),
                                )
                                .wrap(),
                            );
                            ui.end_row();
                        }
                    });
            },
            |ui| {
                ui.checkbox(&mut only_diffs, "Show only differing columns");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if secondary_button(ui, p, icon::X, "Close", true).clicked() {
                        close_clicked = true;
                    }
                });
            },
        );
        if outcome.closed {
            open = false;
        }

        if open && !close_clicked {
            self.tabs[self.active_tab_idx].diff_rows = Some((row_a, row_b, only_diffs));
        } else {
            self.tabs[self.active_tab_idx].diff_rows = None;
        }
    }
}

fn try_pretty_json(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    serde_json::to_string_pretty(&parsed).ok()
}
