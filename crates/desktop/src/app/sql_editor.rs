use super::{insert_completion, CottonDataApp};
use crate::theme::{highlight_mysql_layout, Palette};
use crate::ui_kit::{
    banner, ghost_button, icon, icon_text, inset_frame, primary_button, tinted_ghost_button, toolbar_divider,
};
use crate::worker::SlotCmd;
use cottondata_sql::{analyze_statement, format_mysql, statement_at_offset, CompletionProvider, WriteClass};
use eframe::egui::{self, RichText};

impl CottonDataApp {
    pub(super) fn render_sql_editor_section(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut run_current = false;
        let mut run_all = false;
        let mut stop_query = false;
        let mut do_explain = None;
        let mut do_format = false;

        let (is_running, safety_warning, current_sql, cursor_byte, slot_for_tab) = {
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
                tab.slot.or(self.active_slot),
            )
        };

        ui.horizontal(|ui| {
            run_current = primary_button(ui, p, icon::PLAY, "Run", !is_running)
                .on_hover_text("Run the statement at the cursor (Cmd/Ctrl+Enter)")
                .clicked();
            run_all = ghost_button(ui, p, icon::FAST_FORWARD, "Run last", !is_running)
                .on_hover_text("Run the last statement in the editor")
                .clicked();
            stop_query = tinted_ghost_button(ui, p.danger, icon::STOP, "Stop", is_running)
                .on_hover_text("KILL QUERY on the running statement")
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
        let mut layouter = move |ui: &egui::Ui, string: &str, wrap_width: f32| {
            highlight_mysql_layout(ui, string, wrap_width, palette_copy)
        };

        let mut sql_edited = false;
        inset_frame(p).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("sql_editor_scroll")
                .max_height(190.0)
                .show(ui, |ui| {
                    let tab = &mut self.tabs[self.active_tab_idx];
                    let out = egui::TextEdit::multiline(&mut tab.sql)
                        .desired_rows(7)
                        .desired_width(f32::INFINITY)
                        .code_editor()
                        .frame(false)
                        .layouter(&mut layouter)
                        .show(ui);
                    if out.response.changed() {
                        sql_edited = true;
                    }
                    if let Some(range) = out.cursor_range {
                        let char_idx = range.primary.ccursor.index;
                        tab.cursor_byte = tab
                            .sql
                            .char_indices()
                            .nth(char_idx)
                            .map(|(b, _)| b)
                            .unwrap_or(tab.sql.len());
                    }
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

        if run_current {
            self.execute_active_tab(false);
        }
        if run_all {
            self.execute_active_tab(true);
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
