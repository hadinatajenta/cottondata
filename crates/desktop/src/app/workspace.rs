use super::query_tab::BottomDeckTab;
use super::CottonDataApp;
use crate::theme::{Palette, GUTTER, RADIUS_CARD};
use crate::ui_kit::{banner, caption, card_frame, document_tab, icon, icon_button, icon_text, underline_tab};
use eframe::egui::{self, CornerRadius, Margin, Shape, Stroke};

const RESULT_TABS: [(BottomDeckTab, &str, &str); 5] = [
    (BottomDeckTab::DataGrid, icon::ROWS, "Data Grid"),
    (BottomDeckTab::DdlInspector, icon::CODE, "DDL & Indexes"),
    (BottomDeckTab::ExplainPlan, icon::CHART_BAR, "Explain Plan"),
    (BottomDeckTab::Monitor, icon::GAUGE, "Processlist & Health"),
    (BottomDeckTab::History, icon::CLOCK_COUNTER_CLOCKWISE, "History"),
];

impl CottonDataApp {
    pub(super) fn render_workspace(&mut self, ctx: &egui::Context, palette: Palette) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette.bg_backdrop)
                    .inner_margin(Margin::same(GUTTER)),
            )
            .show(ctx, |ui| {
                if let Some((message, is_error)) = self.toast_banner.clone() {
                    let (tone, glyph) = if is_error {
                        (palette.danger, icon::WARNING)
                    } else {
                        (palette.success, icon::CHECK_CIRCLE)
                    };
                    if banner(ui, palette, tone, glyph, &message, true) {
                        self.toast_banner = None;
                    }
                    ui.add_space(f32::from(GUTTER));
                }

                card_frame(palette).inner_margin(Margin::ZERO).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    self.render_document_tabs(ui, palette);
                    egui::Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
                        self.render_sql_editor_section(ui, palette);
                    });
                });
                ui.add_space(f32::from(GUTTER));
                card_frame(palette).inner_margin(Margin::ZERO).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    self.render_result_tabs(ui, palette);
                    egui::Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.render_result_content(ui, palette);
                    });
                });
            });
    }

    fn render_document_tabs(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let strip_background = ui.painter().add(Shape::Noop);
        let mut clicked_tab = None;
        let mut closed_tab = None;
        let mut is_new_console_requested = false;

        let strip = egui::Frame::new()
            .inner_margin(Margin {
                left: 8,
                right: 8,
                top: 6,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    egui::ScrollArea::horizontal()
                        .id_salt("document_tab_strip")
                        .max_width(ui.available_width() - 32.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let is_closable = self.tabs.len() > 1;
                                for (idx, tab) in self.tabs.iter().enumerate() {
                                    let glyph = if tab.bound_table.is_some() {
                                        icon::TABLE
                                    } else {
                                        icon::TERMINAL_WINDOW
                                    };
                                    let label = match tab.slot {
                                        Some(slot) => format!("{slot} {}", tab.title),
                                        None => tab.title.clone(),
                                    };
                                    let response = document_tab(
                                        ui,
                                        palette,
                                        glyph,
                                        &label,
                                        idx == self.active_tab_idx,
                                        is_closable,
                                    );
                                    if response.is_close_clicked {
                                        closed_tab = Some(idx);
                                    } else if response.is_clicked {
                                        clicked_tab = Some(idx);
                                    }
                                }
                            });
                        });
                    is_new_console_requested = icon_button(ui, palette, icon::PLUS, "New console").clicked();
                });
            });

        let strip_rect = strip.response.rect;
        let card_radius = CornerRadius {
            nw: RADIUS_CARD,
            ne: RADIUS_CARD,
            sw: 0,
            se: 0,
        };
        ui.painter().set(
            strip_background,
            Shape::Vec(vec![
                Shape::rect_filled(strip_rect.shrink(1.0), card_radius, palette.bg_surface),
                Shape::hline(
                    strip_rect.x_range(),
                    strip_rect.bottom() - 0.5,
                    Stroke::new(1.0, palette.border),
                ),
            ]),
        );

        if let Some(idx) = clicked_tab {
            self.activate_tab(idx);
        }
        if let Some(idx) = closed_tab {
            self.close_tab(idx);
        }
        if is_new_console_requested {
            self.open_console_tab();
        }
    }

    fn render_result_tabs(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let tabs_row = egui::Frame::new().inner_margin(Margin::symmetric(8, 0)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (deck_tab, glyph, title) in RESULT_TABS {
                    let label = match deck_tab {
                        BottomDeckTab::History => format!("{title} ({})", self.history.len()),
                        _ => title.to_string(),
                    };
                    if underline_tab(ui, palette, glyph, &label, self.bottom_tab == deck_tab).clicked() {
                        self.select_result_tab(deck_tab);
                    }
                }
            });
        });
        let row_rect = tabs_row.response.rect;
        ui.painter()
            .hline(row_rect.x_range(), row_rect.bottom(), Stroke::new(1.0, palette.divider));
    }

    fn select_result_tab(&mut self, deck_tab: BottomDeckTab) {
        self.bottom_tab = deck_tab;
        if deck_tab != BottomDeckTab::Monitor {
            return;
        }
        if let Some(slot) = self.active_slot {
            self.request_monitor_refresh(slot);
        }
    }

    fn render_result_content(&mut self, ui: &mut egui::Ui, palette: Palette) {
        match self.bottom_tab {
            BottomDeckTab::DataGrid => self.render_data_grid(ui, palette),
            BottomDeckTab::DdlInspector => self.render_ddl_view(ui, palette),
            BottomDeckTab::ExplainPlan => self.render_explain_view(ui, palette),
            BottomDeckTab::Monitor => self.render_monitor_view(ui, palette),
            BottomDeckTab::History => self.render_history_view(ui, palette),
        }
    }

    pub(super) fn render_status_bar(&self, ctx: &egui::Context, palette: Palette) {
        egui::TopBottomPanel::bottom("status_bar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(palette.bg_backdrop).inner_margin(Margin {
                left: 14,
                right: 14,
                top: 0,
                bottom: 6,
            }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let runtime = self
                        .active_slot
                        .and_then(|slot_id| self.registry.get(slot_id))
                        .and_then(|slot| slot.runtime.as_ref());
                    match runtime {
                        Some(runtime) => {
                            ui.label(icon_text(
                                icon::PLUGS_CONNECTED,
                                palette.success,
                                &format!("MySQL {}", runtime.version),
                                palette.text_muted,
                                11.5,
                            ));
                            caption(ui, palette, format!("connection #{}", runtime.connection_id));
                            if runtime.in_transaction {
                                caption(ui, palette, "transaction open");
                            }
                        }
                        None => {
                            ui.label(icon_text(
                                icon::PLUG,
                                palette.text_muted,
                                "Not connected",
                                palette.text_muted,
                                11.5,
                            ));
                        }
                    }
                    let session_name = &self.sessions_store.active_session().name;
                    let save_state = if self.session_dirty_since.is_some() {
                        "saving…"
                    } else {
                        "saved"
                    };
                    caption(
                        ui,
                        palette,
                        format!("· Session: {session_name} ({} tabs, {save_state})", self.tabs.len()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let stats = self.memory_budget.stats();
                        let used_mebibytes = stats.used_bytes as f64 / (1024.0 * 1024.0);
                        let limit_mebibytes = stats.limit_bytes / (1024 * 1024);
                        ui.label(icon_text(
                            icon::MEMORY,
                            palette.text_muted,
                            &format!("Grid memory {used_mebibytes:.2} / {limit_mebibytes} MiB"),
                            palette.text_muted,
                            11.5,
                        ));
                    });
                });
            });
    }
}
