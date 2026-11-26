use super::{truncate_display, CottonDataApp, HistoryStatusFilter};
use crate::theme::{highlight_mysql_layout, Palette};
use crate::ui_kit::{
    banner, busy_indicator, caption, empty_state, ghost_button, icon, icon_button, icon_text, inset_frame,
    loading_state, pill, secondary_button, segmented_tabs, status_dot, styled_text_input, tinted_ghost_button,
};
use crate::worker::SlotCmd;
use cottondata_core::ServerSlotId;
use eframe::egui::{self, FontFamily, FontId, RichText};

impl CottonDataApp {
    pub(super) fn render_ddl_view(&mut self, ui: &mut egui::Ui, p: Palette) {
        let pending_detail = self
            .active_slot
            .and_then(|slot| self.slot_activity[slot.index()].loading_table_detail.as_ref());
        if let Some((schema, table)) = pending_detail {
            loading_state(ui, p, &format!("Loading DDL for {schema}.{table}…"));
            return;
        }
        let mut edit_structure_req: Option<(String, String)> = None;
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if secondary_button(ui, p, icon::PENCIL_SIMPLE, "Edit Structure…", self.active_slot.is_some())
                    .clicked()
                {
                    edit_structure_req = Some((schema.clone(), table.clone()));
                }
            });
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
            if !detail.inbound_foreign_keys.is_empty() {
                ui.add_space(6.0);
                ui.label(icon_text(
                    icon::ARROW_U_UP_LEFT,
                    p.icon_table,
                    &format!("Referenced by ({})", detail.inbound_foreign_keys.len()),
                    p.text_primary,
                    13.0,
                ));
                for fk in &detail.inbound_foreign_keys {
                    ui.label(
                        RichText::new(format!(
                            "  {} : {}.{}.{} -> {} (ON UPDATE {}, ON DELETE {})",
                            fk.constraint_name,
                            fk.source_schema,
                            fk.source_table,
                            fk.source_column,
                            fk.target_column,
                            fk.update_rule,
                            fk.delete_rule
                        ))
                        .font(FontId::new(11.5, FontFamily::Monospace)),
                    );
                }
            }
        });
        if let (Some(slot), Some((s, t))) = (self.active_slot, edit_structure_req) {
            self.open_structure_editor(slot, s, t);
        }
    }

    pub(super) fn render_explain_view(&self, ui: &mut egui::Ui, p: Palette) {
        let active_tab = &self.tabs[self.active_tab_idx];
        if active_tab.is_running {
            loading_state(ui, p, "Waiting for the server to finish…");
            return;
        }
        let Some(exp) = &active_tab.explain else {
            empty_state(
                ui,
                p,
                icon::CHART_BAR,
                "Use Explain or Analyze in the editor toolbar to inspect the execution plan.",
            );
            return;
        };
        let has_full_scan = plan_has_full_scan(&exp.raw_output);
        ui.horizontal(|ui| {
            ui.label(icon_text(icon::CHART_BAR, p.accent, &exp.label, p.text_primary, 13.0));
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

    pub(super) fn request_monitor_refresh(&mut self, slot: ServerSlotId) {
        let activity = &mut self.slot_activity[slot.index()];
        if activity.is_refreshing_monitor {
            return;
        }
        activity.is_refreshing_monitor = true;
        self.hub.send(slot, SlotCmd::FetchProcesslist);
        self.hub.send(slot, SlotCmd::FetchHealth);
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
            let is_refreshing = self.slot_activity[slot.index()].is_refreshing_monitor;
            if ghost_button(ui, p, icon::ARROW_CLOCKWISE, "Refresh", !is_refreshing).clicked() {
                self.request_monitor_refresh(slot);
            }
            if is_refreshing {
                busy_indicator(ui, p, "Refreshing…");
            }
            if let Some(health) = &self.health[slot.index()] {
                for metric in &health.metrics {
                    caption(ui, p, format!("{}: {}", metric.label, metric.value));
                }
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
        if self.history.is_empty() {
            empty_state(
                ui,
                p,
                icon::CLOCK_COUNTER_CLOCKWISE,
                "Executed statements will appear here.",
            );
            return;
        }

        ui.horizontal_wrapped(|ui| {
            styled_text_input(
                ui,
                p,
                "history_search_input",
                &mut self.history_search,
                "Search SQL, server, or schema…",
                260.0,
            );
            segmented_tabs(
                ui,
                p,
                &mut self.history_status_filter,
                &[
                    (HistoryStatusFilter::All, "", "All"),
                    (HistoryStatusFilter::Succeeded, "", "OK"),
                    (HistoryStatusFilter::Failed, "", "Failed"),
                ],
            );
            if !self.history_search.is_empty() && icon_button(ui, p, icon::X, "Clear search").clicked() {
                self.history_search.clear();
            }
        });
        ui.add_space(6.0);

        let needle = self.history_search.trim().to_ascii_lowercase();
        let filtered_indices: Vec<usize> = self
            .history
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let status_ok = match self.history_status_filter {
                    HistoryStatusFilter::All => true,
                    HistoryStatusFilter::Succeeded => item.succeeded,
                    HistoryStatusFilter::Failed => !item.succeeded,
                };
                if !status_ok {
                    return false;
                }
                if needle.is_empty() {
                    return true;
                }
                item.sql.to_ascii_lowercase().contains(&needle)
                    || item.server_name.to_ascii_lowercase().contains(&needle)
                    || item
                        .schema
                        .as_deref()
                        .is_some_and(|s| s.to_ascii_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect();

        if filtered_indices.is_empty() {
            empty_state(ui, p, icon::MAGNIFYING_GLASS, "No history entries match your search.");
            return;
        }

        let mut open_new_tab_sql = None;
        let mut load_sql = None;
        let mut save_snippet_sql = None;
        let mut copy_sql = None;

        egui::ScrollArea::vertical()
            .id_salt("history_list_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for idx in filtered_indices {
                    let item = &self.history[idx];
                    ui.horizontal(|ui| {
                        status_dot(ui, if item.succeeded { p.success } else { p.danger });
                        if secondary_button(ui, p, icon::PLUS, "New Tab", true)
                            .on_hover_text("Open query in a new console tab")
                            .clicked()
                        {
                            open_new_tab_sql = Some(item.sql.clone());
                        }
                        if ghost_button(ui, p, icon::ARROW_U_UP_LEFT, "Load", true)
                            .on_hover_text("Replace active console SQL")
                            .clicked()
                        {
                            load_sql = Some(item.sql.clone());
                        }
                        if icon_button(ui, p, icon::BOOKMARK_SIMPLE, "Save as snippet").clicked() {
                            save_snippet_sql = Some(item.sql.clone());
                        }
                        if icon_button(ui, p, icon::COPY, "Copy SQL").clicked() {
                            copy_sql = Some(item.sql.clone());
                        }
                        let schema_part = item
                            .schema
                            .as_deref()
                            .map(|s| format!(" · {s}"))
                            .unwrap_or_default();
                        ui.label(icon_text(
                            icon::CLOCK,
                            p.text_muted,
                            &format!("{}{} · {} ms", item.server_name, schema_part, item.elapsed_ms),
                            p.text_muted,
                            11.5,
                        ));
                        ui.add(
                            egui::Label::new(
                                RichText::new(truncate_display(&item.sql.replace('\n', " "), 120))
                                    .font(FontId::new(11.5, FontFamily::Monospace))
                                    .color(p.text_primary),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&item.sql);
                    });
                    ui.add_space(2.0);
                }
            });

        if let Some(sql) = open_new_tab_sql {
            self.open_console_tab_for_slot(self.active_slot, Some(sql));
        } else if let Some(sql) = load_sql {
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.sql = sql;
            tab.show_sql_editor = true;
            self.persist_sessions_now();
        }
        if let Some(sql) = save_snippet_sql {
            self.open_save_snippet_modal(sql);
        }
        if let Some(sql) = copy_sql {
            ui.ctx().copy_text(sql);
            self.notify_success("Copied SQL from history");
        }
    }
}

fn plan_has_full_scan(raw_output: &str) -> bool {
    raw_output.contains("\"access_type\": \"ALL\"")
        || raw_output.contains("Table scan")
        || raw_output.contains("PhysicalOp=\"Table Scan\"")
        || raw_output.contains("Seq Scan on")
        || raw_output.contains("FULL SCAN")
        || raw_output.lines().any(|line| {
            let step = line.trim_start().trim_start_matches("└─ ");
            step.starts_with("SCAN ") && !step.starts_with("SCAN CONSTANT ROW")
        })
}

#[cfg(test)]
mod tests {
    use super::plan_has_full_scan;

    #[test]
    fn detects_full_scans_across_engine_plan_formats() {
        assert!(plan_has_full_scan("Seq Scan on orders  (cost=0.00..35.50 rows=2550 width=4)"));
        assert!(plan_has_full_scan("  • scan\n    spans: FULL SCAN"));
        assert!(plan_has_full_scan("SCAN orders\n  └─ SEARCH customers USING INTEGER PRIMARY KEY"));
        assert!(!plan_has_full_scan("SEARCH orders USING INTEGER PRIMARY KEY (rowid=?)"));
        assert!(!plan_has_full_scan("SCAN CONSTANT ROW"));
        assert!(!plan_has_full_scan("Index Scan using orders_pkey on orders"));
    }
}
