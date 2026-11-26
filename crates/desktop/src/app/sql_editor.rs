use super::{insert_completion, CottonDataApp};
use crate::theme::{highlight_mysql_editor_layout, Palette};
use crate::ui_kit::{
    banner, ghost_button, icon, icon_button, icon_text, inset_frame, primary_button, tinted_ghost_button,
    toolbar_divider,
};
use crate::worker::SlotCmd;
use cottondata_sql::{analyze_statement, format_mysql, statement_at_offset, CompletionProvider, WriteClass};
use eframe::egui::{self, text::LayoutJob, FontFamily, FontId, RichText, Stroke, TextFormat};

impl CottonDataApp {
    pub(super) fn render_sql_editor_section(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut run_current = false;
        let mut run_all = false;
        let mut stop_query = false;
        let mut do_explain = None;
        let mut do_format = false;
        let mut toggle_expand_editor = false;

        let (is_running, safety_warning, current_sql, cursor_byte, has_selection, slot_for_tab) = {
            let tab = &self.tabs[self.active_tab_idx];
            let stmt = statement_at_offset(&tab.sql, tab.cursor_byte)
                .map(|s| s.sql)
                .unwrap_or(&tab.sql);
            let safety = analyze_statement(stmt);
            (
                tab.is_running,
                if matches!(safety.class, WriteClass::Destructive) {
                    safety.warning
                } else {
                    None
                },
                tab.sql.clone(),
                tab.cursor_byte,
                tab.selection_byte_range.is_some(),
                tab.slot.or(self.active_slot),
            )
        };

        ui.horizontal(|ui| {
            let run_label = if has_selection { "Run selected" } else { "Run" };
            run_current = primary_button(ui, p, icon::PLAY, run_label, !is_running)
                .on_hover_text("Run selection or statement at cursor (Cmd/Ctrl+Enter)")
                .clicked();
            run_all = ghost_button(ui, p, icon::FAST_FORWARD, "Run all", !is_running)
                .on_hover_text("Run all statements in editor as separate result tabs (Cmd/Ctrl+Shift+Enter)")
                .clicked();
            stop_query = tinted_ghost_button(ui, p.danger, icon::STOP, "Stop", is_running)
                .on_hover_text("Cancel the running statement")
                .clicked();
            toolbar_divider(ui, p);
            if ghost_button(ui, p, icon::CHART_BAR, "Explain", !is_running).clicked() {
                do_explain = Some(false);
            }
            if ghost_button(ui, p, icon::GAUGE, "Analyze", !is_running)
                .on_hover_text("EXPLAIN ANALYZE")
                .clicked()
            {
                do_explain = Some(true);
            }
            toolbar_divider(ui, p);
            do_format = ghost_button(ui, p, icon::MAGIC_WAND, "Format", true).clicked();
            let is_expanded = self.editor_height >= 340.0;
            let expand_glyph = if is_expanded { icon::CARET_UP } else { icon::CARET_DOWN };
            let expand_tip = if is_expanded {
                "Compact SQL editor"
            } else {
                "Expand SQL editor"
            };
            if icon_button(ui, p, expand_glyph, expand_tip).clicked() {
                toggle_expand_editor = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(&self.tabs[self.active_tab_idx].last_status_msg)
                            .size(11.5)
                            .color(p.text_muted),
                    )
                    .truncate(),
                );
                if is_running {
                    ui.spinner();
                }
            });
        });
        ui.add_space(4.0);

        if let Some(warn) = safety_warning {
            banner(ui, p, p.warning, icon::WARNING, &format!("Safety guard: {warn}"), false);
            ui.add_space(4.0);
        }

        let palette_copy = p;
        let mut layouter = move |ui: &egui::Ui, string: &str, _wrap_width: f32| {
            highlight_mysql_editor_layout(ui, string, palette_copy)
        };

        let mut sql_edited = false;
        let editor_h = self.editor_height;
        let font_id = FontId::new(13.0, FontFamily::Monospace);
        let row_height = ui.fonts(|f| f.row_height(&font_id)).max(16.0);
        let min_visible_lines = ((editor_h - 10.0) / row_height).floor().max(6.0) as usize;

        inset_frame(p).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt(("sql_editor_scroll", self.tabs[self.active_tab_idx].id))
                .max_height(editor_h)
                .min_scrolled_height(editor_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let tab = &mut self.tabs[self.active_tab_idx];
                    let line_count = tab.sql.split('\n').count().max(1);
                    let display_lines = line_count.max(min_visible_lines);
                    let safe_cursor = tab.cursor_byte.min(tab.sql.len());
                    let active_line = tab.sql[..safe_cursor].bytes().filter(|&b| b == b'\n').count() + 1;
                    let digit_width = display_lines.to_string().len().max(2);

                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let gutter_job = build_line_numbers_job(display_lines, active_line, digit_width, &font_id, p);
                        let gutter_resp = egui::Frame::new()
                            .inner_margin(egui::Margin {
                                left: 4,
                                right: 10,
                                top: 2,
                                bottom: 2,
                            })
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(gutter_job).selectable(false));
                            })
                            .response;
                        let divider_x = gutter_resp.rect.right();
                        ui.painter().vline(
                            divider_x,
                            gutter_resp.rect.y_range(),
                            Stroke::new(1.0, p.divider),
                        );
                        ui.add_space(8.0);

                        let editor_min_w = (ui.available_width() - 4.0).max(260.0);
                        let editor_min_h = (editor_h - 8.0).max(100.0);
                        let out = egui::TextEdit::multiline(&mut tab.sql)
                            .min_size(egui::vec2(editor_min_w, editor_min_h))
                            .desired_width(f32::INFINITY)
                            .code_editor()
                            .frame(false)
                            .layouter(&mut layouter)
                            .show(ui);
                        if out.response.changed() {
                            sql_edited = true;
                        }
                        if let Some(range) = out.cursor_range {
                            let primary_idx = range.primary.ccursor.index;
                            let secondary_idx = range.secondary.ccursor.index;
                            tab.cursor_byte = char_index_to_byte(&tab.sql, primary_idx);
                            if primary_idx != secondary_idx {
                                let lo = char_index_to_byte(&tab.sql, primary_idx.min(secondary_idx));
                                let hi = char_index_to_byte(&tab.sql, primary_idx.max(secondary_idx));
                                tab.selection_byte_range = (lo < hi).then_some((lo, hi));
                            } else {
                                tab.selection_byte_range = None;
                            }
                        }
                    });
                });
        });

        let hints = slot_for_tab
            .and_then(|s| {
                let active_db = self
                    .registry
                    .get(s)
                    .and_then(|sl| sl.runtime.as_ref())
                    .and_then(|r| r.current_schema.as_deref());
                self.catalogs[s.index()]
                    .as_ref()
                    .map(|cat| cat.completion_hints(active_db))
            })
            .unwrap_or_default();

        let suggestions = CompletionProvider::suggest(&current_sql, cursor_byte, &hints, 6);
        if !suggestions.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(icon_text(icon::SPARKLE, p.accent, "Suggestions", p.text_muted, 11.5));
                for item in &suggestions {
                    if ui.small_button(format!("{} ({})", item.label, item.detail)).clicked() {
                        let tab = &mut self.tabs[self.active_tab_idx];
                        insert_completion(&mut tab.sql, tab.cursor_byte, &item.label);
                        sql_edited = true;
                    }
                }
            });
        }

        if toggle_expand_editor {
            self.editor_height = if self.editor_height >= 340.0 { 220.0 } else { 420.0 };
        }
        if run_current {
            self.request_execute_active_tab(false);
        }
        if run_all {
            self.request_execute_active_tab(true);
        }
        if stop_query {
            if let Some(slot_id) = slot_for_tab {
                if let Some(slot) = self.registry.get(slot_id) {
                    if let Some(rt) = &slot.runtime {
                        let pw = self.storage.get_password(&slot.profile.id);
                        self.hub
                            .cancel_query_async(slot_id, slot.profile.clone(), pw, rt.connection_id);
                    }
                }
            }
        }
        if let Some(analyze) = do_explain {
            if let Some(slot_id) = slot_for_tab {
                let tab = &mut self.tabs[self.active_tab_idx];
                let sql = statement_at_offset(&tab.sql, tab.cursor_byte)
                    .map(|s| s.sql.to_string())
                    .unwrap_or_else(|| tab.sql.trim().to_string());
                if !sql.is_empty() {
                    tab.is_running = true;
                    let tid = tab.id;
                    self.hub.send(
                        slot_id,
                        SlotCmd::ExplainQuery {
                            tab_id: tid,
                            sql,
                            analyze,
                        },
                    );
                }
            }
        }
        if do_format {
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.sql = format_mysql(&tab.sql);
            sql_edited = true;
        }
        if sql_edited {
            self.mark_session_dirty();
        }
    }
}

fn char_index_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(text.len())
}

fn build_line_numbers_job(
    display_lines: usize,
    active_line: usize,
    digit_width: usize,
    font_id: &FontId,
    p: Palette,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    for line in 1..=display_lines {
        let is_active = line == active_line;
        let color = if is_active { p.accent } else { p.text_muted };
        let text = if line < display_lines {
            format!("{line:>digit_width$}\n")
        } else {
            format!("{line:>digit_width$}")
        };
        job.append(
            &text,
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color,
                ..Default::default()
            },
        );
    }
    job
}
