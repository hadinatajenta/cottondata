use super::{truncate_display, CottonDataApp};
use crate::theme::{highlight_mysql_editor_layout, Palette, RADIUS_CONTROL};
use crate::ui_kit::{
    banner, caption, empty_state, field_label, ghost_button, icon, icon_button, icon_text, inset_frame, modal_shell,
    pill, primary_button, secondary_button, segmented_tabs, styled_text_input, tinted_ghost_button, toggle_card,
    ModalConfig,
};
use cottondata_storage::{SavedQuery, SnippetScope, SnippetScopeFilter};
use eframe::egui::{self, CornerRadius, FontFamily, FontId, Margin, RichText};

pub(super) struct SnippetModalState {
    pub(super) editing_id: Option<String>,
    pub(super) name: String,
    pub(super) folder: String,
    pub(super) is_server_scoped: bool,
    pub(super) server_id: Option<String>,
    pub(super) pinned: bool,
    pub(super) sql: String,
    pub(super) error_msg: Option<String>,
}

impl SnippetModalState {
    pub(super) fn new_from_sql(sql: String, active_server_id: Option<String>) -> Self {
        Self {
            editing_id: None,
            name: String::new(),
            folder: String::new(),
            is_server_scoped: false,
            server_id: active_server_id,
            pinned: false,
            sql,
            error_msg: None,
        }
    }

    pub(super) fn from_existing(item: &SavedQuery, fallback_server_id: Option<String>) -> Self {
        let (is_server_scoped, server_id) = match &item.scope {
            SnippetScope::Global => (false, fallback_server_id),
            SnippetScope::Server(sid) => (true, Some(sid.clone())),
        };
        Self {
            editing_id: Some(item.id.clone()),
            name: item.name.clone(),
            folder: item.folder.clone(),
            is_server_scoped,
            server_id,
            pinned: item.pinned,
            sql: item.sql.clone(),
            error_msg: None,
        }
    }
}

enum SnippetAction {
    OpenNewTab(String),
    InsertInEditor(String),
    CopySql(String),
    TogglePin(String),
    Edit(String),
    Delete(String),
}

impl CottonDataApp {
    pub(super) fn open_save_snippet_modal(&mut self, sql: String) {
        let active_server_id = self.active_slot.and_then(|s| self.server_id_for_slot(s));
        self.snippet_modal = Some(SnippetModalState::new_from_sql(sql, active_server_id));
    }

    pub(super) fn render_snippets_view(&mut self, ui: &mut egui::Ui, p: Palette) {
        let active_server_id = self.active_slot.and_then(|s| self.server_id_for_slot(s));
        let folders = self.snippets_store.folders();
        let mut open_new_modal = false;

        ui.horizontal_wrapped(|ui| {
            let search_w = 220.0;
            styled_text_input(
                ui,
                p,
                "snippets_search_input",
                &mut self.snippet_search,
                "Search saved queries…",
                search_w,
            );
            segmented_tabs(
                ui,
                p,
                &mut self.snippet_scope_filter,
                &[
                    (SnippetScopeFilter::All, "", "All"),
                    (SnippetScopeFilter::ActiveServerOnly, "", "This Server"),
                    (SnippetScopeFilter::GlobalOnly, "", "Global"),
                ],
            );
            let folder_label = self
                .snippet_folder_filter
                .as_deref()
                .unwrap_or("All folders")
                .to_string();
            egui::ComboBox::from_id_salt("snippet_folder_filter_combo")
                .truncate()
                .selected_text(icon_text(icon::FOLDER, p.text_muted, &folder_label, p.text_primary, 12.5))
                .width(136.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.snippet_folder_filter.is_none(), "All folders")
                        .clicked()
                    {
                        self.snippet_folder_filter = None;
                    }
                    for folder in &folders {
                        let selected = self.snippet_folder_filter.as_deref() == Some(folder.as_str());
                        if ui.selectable_label(selected, folder).clicked() {
                            self.snippet_folder_filter = Some(folder.clone());
                        }
                    }
                });
            if (!self.snippet_search.is_empty() || self.snippet_folder_filter.is_some())
                && icon_button(ui, p, icon::X, "Clear filters").clicked()
            {
                self.snippet_search.clear();
                self.snippet_folder_filter = None;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary_button(ui, p, icon::BOOKMARK_SIMPLE, "Save current SQL", true)
                    .on_hover_text("Save active console SQL as a reusable snippet (Cmd/Ctrl+Shift+S)")
                    .clicked()
                {
                    open_new_modal = true;
                }
            });
        });

        ui.add_space(6.0);

        let items: Vec<SavedQuery> = self
            .snippets_store
            .filtered(
                &self.snippet_search,
                self.snippet_folder_filter.as_deref(),
                active_server_id.as_deref(),
                self.snippet_scope_filter,
            )
            .into_iter()
            .cloned()
            .collect();

        if items.is_empty() {
            let msg = if self.snippets_store.items.is_empty() {
                "No saved queries yet. Save frequent SQL queries with folders and pins."
            } else {
                "No saved queries match your current filter."
            };
            empty_state(ui, p, icon::BOOKMARK_SIMPLE, msg);
        } else {
            let mut action = None;
            egui::ScrollArea::vertical()
                .id_salt("snippets_list_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for item in &items {
                        let scope_label = match &item.scope {
                            SnippetScope::Global => "GLOBAL".to_string(),
                            SnippetScope::Server(sid) => self
                                .registry
                                .iter()
                                .find(|s| s.profile.id == *sid)
                                .map(|s| s.profile.name.clone())
                                .unwrap_or_else(|| "Server".into()),
                        };
                        let scope_color = match &item.scope {
                            SnippetScope::Global => p.accent,
                            SnippetScope::Server(_) => p.icon_table,
                        };
                        egui::Frame::new()
                            .fill(if item.pinned { p.accent_soft.gamma_multiply(0.55) } else { p.bg_surface })
                            .stroke(egui::Stroke::new(1.0, p.border))
                            .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                            .inner_margin(Margin::symmetric(10, 7))
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let pin_color = if item.pinned { p.warning } else { p.text_muted };
                                    if tinted_ghost_button(ui, pin_color, icon::PUSH_PIN, "", true)
                                        .on_hover_text(if item.pinned { "Unpin snippet" } else { "Pin to top" })
                                        .clicked()
                                    {
                                        action = Some(SnippetAction::TogglePin(item.id.clone()));
                                    }
                                    ui.label(RichText::new(&item.name).size(13.0).strong().color(p.text_primary));
                                    if !item.folder.trim().is_empty() {
                                        pill(ui, item.folder.trim(), p.text_muted);
                                    }
                                    pill(ui, &scope_label, scope_color);

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if tinted_ghost_button(ui, p.danger, icon::TRASH, "", true)
                                            .on_hover_text("Delete snippet")
                                            .clicked()
                                        {
                                            action = Some(SnippetAction::Delete(item.id.clone()));
                                        }
                                        if icon_button(ui, p, icon::PENCIL_SIMPLE, "Edit snippet").clicked() {
                                            action = Some(SnippetAction::Edit(item.id.clone()));
                                        }
                                        if icon_button(ui, p, icon::COPY, "Copy SQL").clicked() {
                                            action = Some(SnippetAction::CopySql(item.sql.clone()));
                                        }
                                        if ghost_button(ui, p, icon::ARROW_U_UP_LEFT, "Load", true)
                                            .on_hover_text("Load into current console tab")
                                            .clicked()
                                        {
                                            action = Some(SnippetAction::InsertInEditor(item.sql.clone()));
                                        }
                                        if secondary_button(ui, p, icon::PLUS, "New Tab", true)
                                            .on_hover_text("Open snippet in a new console tab")
                                            .clicked()
                                        {
                                            action = Some(SnippetAction::OpenNewTab(item.sql.clone()));
                                        }
                                    });
                                });
                                ui.add_space(2.0);
                                ui.label(
                                    RichText::new(truncate_display(&item.sql.replace('\n', " "), 140))
                                        .font(FontId::new(11.5, FontFamily::Monospace))
                                        .color(p.text_muted),
                                );
                            });
                        ui.add_space(4.0);
                    }
                });

            if let Some(act) = action {
                match act {
                    SnippetAction::OpenNewTab(sql) => {
                        self.open_console_tab_for_slot(self.active_slot, Some(sql));
                    }
                    SnippetAction::InsertInEditor(sql) => {
                        let tab = &mut self.tabs[self.active_tab_idx];
                        tab.sql = sql;
                        tab.show_sql_editor = true;
                        self.persist_sessions_now();
                    }
                    SnippetAction::CopySql(sql) => {
                        ui.ctx().copy_text(sql);
                        self.notify_success("Copied snippet SQL");
                    }
                    SnippetAction::TogglePin(id) => {
                        if self.snippets_store.toggle_pin(&id) {
                            self.persist_snippets_now();
                        }
                    }
                    SnippetAction::Edit(id) => {
                        if let Some(found) = self.snippets_store.items.iter().find(|s| s.id == id) {
                            self.snippet_modal = Some(SnippetModalState::from_existing(found, active_server_id));
                        }
                    }
                    SnippetAction::Delete(id) => {
                        if self.snippets_store.delete(&id) {
                            self.persist_snippets_now();
                            self.notify_info("Deleted saved query");
                        }
                    }
                }
            }
        }

        if open_new_modal {
            let current_sql = self.tabs[self.active_tab_idx].sql.clone();
            self.open_save_snippet_modal(current_sql);
        }
    }

    pub(super) fn render_snippet_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(mut state) = self.snippet_modal.take() else {
            return;
        };
        let mut keep_open = true;
        let mut save_clicked = false;
        let existing_folders = self.snippets_store.folders();
        let active_server_name = state.server_id.as_deref().and_then(|sid| {
            self.registry
                .iter()
                .find(|s| s.profile.id == sid)
                .map(|s| s.profile.name.clone())
        });
        let title = if state.editing_id.is_some() {
            "Edit Saved Query"
        } else {
            "Save Query Snippet"
        };

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "snippet_editor_modal",
                icon_glyph: icon::BOOKMARK_SIMPLE,
                icon_color: p.accent,
                title,
                subtitle: Some("Organize reusable SQL with folders, scope, and pins"),
                accent_bar: None,
                width: 560.0,
                max_height: Some(520.0),
                dismiss_on_backdrop: false,
            },
            |ui| {
                let full_w = ui.available_width();
                if let Some(err) = &state.error_msg {
                    banner(ui, p, p.danger, icon::WARNING, err, false);
                    ui.add_space(10.0);
                }

                let gap = 12.0;
                let half_w = (full_w - gap) / 2.0;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.vertical(|ui| {
                        ui.set_width(half_w);
                        field_label(ui, p, "Query Name");
                        styled_text_input(
                            ui,
                            p,
                            "snippet_name_input",
                            &mut state.name,
                            "e.g. Check Merchant Settlement",
                            half_w,
                        );
                    });
                    ui.vertical(|ui| {
                        ui.set_width(half_w);
                        field_label(ui, p, "Folder (Optional)");
                        styled_text_input(
                            ui,
                            p,
                            "snippet_folder_input",
                            &mut state.folder,
                            "e.g. Reports, Debug, Ops",
                            half_w,
                        );
                    });
                });

                if !existing_folders.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        caption(ui, p, "Folders:");
                        for folder in &existing_folders {
                            if ui.small_button(folder).clicked() {
                                state.folder = folder.clone();
                            }
                        }
                    });
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    field_label(ui, p, "Scope");
                    ui.add_space(8.0);
                    let server_label = active_server_name
                        .as_deref()
                        .map(|n| format!("Server: {n}"))
                        .unwrap_or_else(|| "Current Server".into());
                    let can_server_scope = state.server_id.is_some();
                    if can_server_scope {
                        segmented_tabs(
                            ui,
                            p,
                            &mut state.is_server_scoped,
                            &[
                                (false, icon::GLOBE, "Global"),
                                (true, icon::DATABASE, server_label.as_str()),
                            ],
                        );
                    } else {
                        state.is_server_scoped = false;
                        pill(ui, "GLOBAL", p.accent);
                    }
                });

                ui.add_space(10.0);
                toggle_card(
                    ui,
                    p,
                    &mut state.pinned,
                    "Pin to top",
                    "Keep this snippet pinned at the top of your Saved Queries list.",
                );

                ui.add_space(12.0);
                field_label(ui, p, "SQL");
                let palette_copy = p;
                let mut layouter = move |ui: &egui::Ui, string: &str, _wrap_width: f32| {
                    highlight_mysql_editor_layout(ui, string, palette_copy)
                };
                inset_frame(p).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("snippet_sql_scroll")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut state.sql)
                                    .code_editor()
                                    .frame(false)
                                    .layouter(&mut layouter)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(7),
                            );
                        });
                });
            },
            |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, p, icon::FLOPPY_DISK, "Save Snippet", true).clicked() {
                        save_clicked = true;
                    }
                    if secondary_button(ui, p, icon::X, "Cancel", true).clicked() {
                        keep_open = false;
                    }
                });
            },
        );

        if outcome.closed {
            keep_open = false;
        }

        if save_clicked {
            let scope = if state.is_server_scoped {
                match state.server_id.clone() {
                    Some(sid) => SnippetScope::Server(sid),
                    None => SnippetScope::Global,
                }
            } else {
                SnippetScope::Global
            };
            match self.snippets_store.upsert(
                state.editing_id.clone(),
                state.name.clone(),
                state.folder.clone(),
                scope,
                state.pinned,
                state.sql.clone(),
            ) {
                Ok(_) => {
                    self.persist_snippets_now();
                    self.notify_success(format!("Saved snippet `{}`", state.name.trim()));
                    keep_open = false;
                }
                Err(err) => {
                    state.error_msg = Some(err);
                }
            }
        }

        if keep_open {
            self.snippet_modal = Some(state);
        }
    }

    fn persist_snippets_now(&mut self) {
        if let Err(err) = self.storage.save_snippets(&self.snippets_store) {
            self.notify_error(format!("Could not save snippets: {err}"));
        }
    }
}
