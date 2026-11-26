use super::{truncate_display, CottonDataApp};
use crate::theme::{highlight_mysql_layout, Palette};
use crate::ui_kit::{
    banner, caption, empty_state, ghost_button, icon, icon_text, inset_frame, pill, tinted_ghost_button,
};
use crate::worker::SlotCmd;
use eframe::egui::{self, FontFamily, FontId, RichText};

impl CottonDataApp {
    pub(super) fn render_ddl_view(&self, ui: &mut egui::Ui, p: Palette) {
        let Some((schema, table, detail)) = &self.ddl_view else {
            empty_state(
                ui,
                p,
                icon::CODE,
                "Use the code icon next to a table in the navigator to inspect its DDL, indexes, and foreign keys.",
            );
            return;
        };
        ui.horizontal(|ui| {
            ui.label(icon_text(
                icon::TABLE,
                p.icon_table,
                &format!("{schema}.{table}"),
                p.text_primary,
                13.5,
            ));
            if let Some(eng) = &detail.engine {
                pill(ui, eng, p.accent);
            }
            if let Some(col) = &detail.collation {
                caption(ui, p, col.as_str());
            }
            if let Some(rows) = detail.row_estimate {
                caption(ui, p, format!("~{rows} rows"));
            }
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut ddl_str = detail.ddl.clone();
            let mut layouter =
                move |ui: &egui::Ui, text: &str, wrap_width: f32| highlight_mysql_layout(ui, text, wrap_width, p);
            inset_frame(p).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut ddl_str)
                        .code_editor()
                        .frame(false)
                        .layouter(&mut layouter)
                        .desired_width(f32::INFINITY)
                        .desired_rows(10),
                );
            });
            ui.add_space(6.0);
            ui.label(icon_text(
                icon::KEY,
                p.warning,
                &format!("Indexes ({})", detail.indexes.len()),
                p.text_primary,
                13.0,
            ));
            for idx in &detail.indexes {
                let uniq = if idx.non_unique { "INDEX" } else { "UNIQUE" };
                ui.label(
                    RichText::new(format!(
                        "  {} [{} · {}] ({})",
                        idx.name,
                        uniq,
                        idx.index_type,
                        idx.columns.join(", ")
                    ))
                    .font(FontId::new(11.5, FontFamily::Monospace)),
                );
            }
            if !detail.foreign_keys.is_empty() {
                ui.add_space(6.0);
                ui.label(icon_text(
                    icon::FLOW_ARROW,
                    p.icon_view,
                    &format!("Foreign keys ({})", detail.foreign_keys.len()),
                    p.text_primary,
                    13.0,
                ));
                for fk in &detail.foreign_keys {
                    ui.label(
                        RichText::new(format!(
                            "  {} : {} -> {}.{}.{} (ON UPDATE {}, ON DELETE {})",
                            fk.constraint_name,
                            fk.column_name,
                            fk.ref_schema,
                            fk.ref_table,
                            fk.ref_column,
                            fk.update_rule,
                            fk.delete_rule
                        ))
                        .font(FontId::new(11.5, FontFamily::Monospace)),
                    );
                }
            }
        });
    }

    pub(super) fn render_explain_view(&self, ui: &mut egui::Ui, p: Palette) {
        let Some(exp) = &self.tabs[self.active_tab_idx].explain else {
            empty_state(
                ui,
                p,
                icon::CHART_BAR,
                "Use Explain or Analyze in the editor toolbar to inspect the execution plan.",
            );
            return;
        };
        let has_full_scan =
            exp.raw_output.contains("\"access_type\": \"ALL\"") || exp.raw_output.contains("Table scan");
        ui.horizontal(|ui| {
            let kind = if exp.is_analyze {
                "EXPLAIN ANALYZE"
            } else {
                "EXPLAIN FORMAT=JSON"
            };
            ui.label(icon_text(icon::CHART_BAR, p.accent, kind, p.text_primary, 13.0));
            caption(ui, p, format!("{} ms", exp.elapsed_ms));
            if has_full_scan {
                banner(
                    ui,
                    p,
                    p.warning,
                    icon::WARNING,
                    "Full table scan detected. Consider adding an index.",
                    false,
                );
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut text = exp.raw_output.clone();
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(14),
            );
        });
    }

    pub(super) fn render_monitor_view(&mut self, ui: &mut egui::Ui, p: Palette) {
        let Some(slot) = self.active_slot else {
            empty_state(
                ui,
                p,
                icon::GAUGE,
                "Select a connected server to see its processlist and health.",
            );
            return;
        };
        ui.horizontal(|ui| {
            if ghost_button(ui, p, icon::ARROW_CLOCKWISE, "Refresh", true).clicked() {
                self.hub.send(slot, SlotCmd::FetchProcesslist);
                self.hub.send(slot, SlotCmd::FetchHealth);
            }
            if let Some(m) = &self.health[slot.index()] {
                let hit = m
                    .innodb_hit_ratio_pct
                    .map(|v| format!("{v:.2}%"))
                    .unwrap_or_else(|| "n/a".into());
                ui.label(
                    RichText::new(format!(
                        "Uptime: {}s · Threads: {}/{} running · Questions: {} · InnoDB Hit: {}",
                        m.uptime_secs, m.threads_running, m.threads_connected, m.questions, hit
                    ))
                    .size(11.5)
                    .color(p.text_muted),
                );
            }
        });
        ui.add_space(4.0);

        let mut kill_pid = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for proc in &self.processlists[slot.index()] {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "#{} {}@{} [{}] {}s · {} · {}",
                            proc.id,
                            proc.user,
                            proc.host,
                            proc.db.as_deref().unwrap_or("-"),
                            proc.time_secs,
                            proc.command,
                            proc.info.as_deref().unwrap_or("").chars().take(90).collect::<String>()
                        ))
                        .font(FontId::new(11.0, FontFamily::Monospace)),
                    );
                    if tinted_ghost_button(ui, p.danger, icon::X_CIRCLE, "Kill", true).clicked() {
                        kill_pid = Some(proc.id);
                    }
                });
            }
        });
        if let Some(pid) = kill_pid {
            self.hub.send(slot, SlotCmd::KillProcess(pid));
        }
    }

    pub(super) fn render_history_view(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut load_sql = None;
        if self.history.is_empty() {
            empty_state(
                ui,
                p,
                icon::CLOCK_COUNTER_CLOCKWISE,
                "Executed statements will appear here.",
            );
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in &self.history {
                ui.horizontal(|ui| {
                    if ghost_button(ui, p, icon::ARROW_U_UP_LEFT, "Load", true).clicked() {
                        load_sql = Some(item.sql.clone());
                    }
                    ui.label(
                        RichText::new(format!(
                            "[{} · {} ms] {}",
                            item.server_name,
                            item.elapsed_ms,
                            truncate_display(&item.sql.replace('\n', " "), 110)
                        ))
                        .font(FontId::new(11.5, FontFamily::Monospace))
                        .color(p.text_primary),
                    );
                });
            }
        });
        if let Some(sql) = load_sql {
            self.tabs[self.active_tab_idx].sql = sql;
        }
    }
}
