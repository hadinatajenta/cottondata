use super::CottonDataApp;
use crate::theme::Palette;
use crate::ui_kit::{banner, icon, primary_button, secondary_button};
use crate::worker::SlotCmd;
use cottondata_core::{DatabaseEngine, EnvironmentTag, TlsMode};
use eframe::egui::{self, RichText};

impl CottonDataApp {
    pub(super) fn render_modals(&mut self, ctx: &egui::Context, p: Palette) {
        self.render_sessions_modal(ctx, p);
        self.render_row_diff_modal(ctx, p);

        if let Some(pending_action) = self.unsaved_guard_modal.clone() {
            let (deletes, updates, inserts) = self.tabs[self.active_tab_idx].mutator.pending_summary();
            let mut decision: Option<u8> = None;
            egui::Window::new("Unsaved Cell Changes")
                .collapsible(false)
                .resizable(false)
                .default_width(420.0)
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "You have uncommitted changes in this tab (D{deletes} · U{updates} · I{inserts})."
                        ))
                        .size(13.0)
                        .color(p.text_primary),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Save your staged changes first, or discard them to continue.")
                            .size(11.5)
                            .color(p.text_muted),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, p, icon::FLOPPY_DISK, "Save & continue", true).clicked() {
                            decision = Some(1);
                        }
                        if secondary_button(ui, p, icon::TRASH, "Discard", true).clicked() {
                            decision = Some(2);
                        }
                        if secondary_button(ui, p, icon::X, "Cancel", true).clicked() {
                            decision = Some(0);
                        }
                    });
                });
            match decision {
                Some(1) => {
                    self.unsaved_guard_modal = None;
                    self.after_save_action = Some(pending_action);
                    self.trigger_save_active_tab(false);
                }
                Some(2) => {
                    self.unsaved_guard_modal = None;
                    self.execute_pending_discard_action(pending_action);
                }
                Some(0) => {
                    self.unsaved_guard_modal = None;
                }
                _ => {}
            }
        }

        if let Some((tab_id, script, statements)) = self.sql_preview_modal.clone() {
            let mut open = true;
            let mut submit_now = false;
            egui::Window::new("Preview Staged Transaction")
                .open(&mut open)
                .resizable(true)
                .default_width(620.0)
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("All changes execute atomically in order: DELETE -> UPDATE -> INSERT.")
                            .size(11.5)
                            .color(p.text_muted),
                    );
                    let mut preview_buf = script.clone();
                    egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut preview_buf)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(12),
                        );
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, p, icon::CHECK_CIRCLE, "Commit transaction", true).clicked() {
                            submit_now = true;
                        }
                        if secondary_button(ui, p, icon::X, "Cancel", true).clicked() {
                            self.sql_preview_modal = None;
                            self.after_save_action = None;
                        }
                    });
                });
            if submit_now {
                if let Some(slot) = self.tabs[self.active_tab_idx].slot.or(self.active_slot) {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        tab.is_submitting_mutations = true;
                    }
                    self.hub.send(slot, SlotCmd::SubmitMutations { tab_id, statements });
                }
                self.sql_preview_modal = None;
            } else if !open {
                self.sql_preview_modal = None;
                self.after_save_action = None;
            }
        }

        let Some(mut state) = self.modal.take() else {
            return;
        };
        let mut keep_open = true;
        let mut save_clicked = false;
        let title = if let Some(slot) = state.editing_slot {
            format!("Edit Server ({slot})")
        } else {
            format!(
                "Add Server (Slot {}/{})",
                self.registry.count() + 1,
                self.registry.max_servers()
            )
        };

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .default_width(470.0)
            .show(ctx, |ui| {
                if let Some(err) = &state.error_msg {
                    banner(ui, p, p.danger, icon::WARNING, err, false);
                    ui.add_space(6.0);
                }

                egui::Grid::new("server_form_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Database");
                        ui.horizontal(|ui| {
                            for engine in DatabaseEngine::ALL {
                                if ui.selectable_label(state.engine == engine, engine.label()).clicked() {
                                    state.switch_engine(engine);
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("Server Name");
                        ui.text_edit_singleline(&mut state.name);
                        ui.end_row();

                        if state.engine.supports_unix_socket() {
                            ui.label("Transport");
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut state.use_unix_socket, false, "TCP/IP");
                                ui.radio_value(&mut state.use_unix_socket, true, "Unix Socket");
                            });
                            ui.end_row();
                        }

                        if state.use_unix_socket && state.engine.supports_unix_socket() {
                            ui.label("Socket Path");
                            ui.text_edit_singleline(&mut state.socket_path);
                            ui.end_row();
                        } else {
                            ui.label("Host & Port");
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut state.host).desired_width(210.0));
                                ui.add(egui::TextEdit::singleline(&mut state.port).desired_width(64.0));
                            });
                            ui.end_row();
                        }

                        ui.label("Username");
                        ui.text_edit_singleline(&mut state.username);
                        ui.end_row();

                        ui.label("Password");
                        ui.add(egui::TextEdit::singleline(&mut state.password).password(true));
                        ui.end_row();

                        ui.label("Credential Vault");
                        ui.checkbox(
                            &mut state.save_in_keychain,
                            if cfg!(target_os = "macos") {
                                "Store in macOS Keychain"
                            } else {
                                "Keep in memory for session"
                            },
                        );
                        ui.end_row();

                        ui.label(match state.engine {
                            DatabaseEngine::MySql => "Default Schema",
                            DatabaseEngine::SqlServer => "Default Database",
                        });
                        ui.text_edit_singleline(&mut state.default_schema);
                        ui.end_row();

                        ui.label("TLS Mode");
                        egui::ComboBox::from_id_salt("modal_tls")
                            .selected_text(state.tls.label())
                            .show_ui(ui, |ui| {
                                for &m in TlsMode::all() {
                                    ui.selectable_value(&mut state.tls, m, m.label());
                                }
                            });
                        ui.end_row();

                        if matches!(state.tls, TlsMode::VerifyCa | TlsMode::VerifyIdentity) {
                            ui.label("CA Cert PEM");
                            ui.horizontal(|ui| {
                                ui.text_edit_singleline(&mut state.ca_cert_path);
                                if ui.small_button("Browse").clicked() {
                                    if let Some(f) = rfd::FileDialog::new().pick_file() {
                                        state.ca_cert_path = f.display().to_string();
                                    }
                                }
                            });
                            ui.end_row();
                        }

                        ui.label("Environment");
                        ui.horizontal(|ui| {
                            for env in [
                                EnvironmentTag::Local,
                                EnvironmentTag::Development,
                                EnvironmentTag::Staging,
                                EnvironmentTag::Production,
                            ] {
                                ui.selectable_value(&mut state.environment, env, env.label());
                            }
                        });
                        ui.end_row();

                        ui.label("Safety & Paging");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut state.read_only, "Read-Only Guard");
                            egui::ComboBox::from_id_salt("modal_page_size")
                                .selected_text(format!("{} rows/page", state.page_size))
                                .show_ui(ui, |ui| {
                                    for sz in [100u32, 500, 1000, 2000] {
                                        ui.selectable_value(&mut state.page_size, sz, format!("{sz} rows"));
                                    }
                                });
                        });
                        ui.end_row();
                    });

                ui.add_space(4.0);
                ui.checkbox(&mut state.ssh_enabled, "Connect through SSH Tunnel");
                if state.ssh_enabled {
                    egui::Grid::new("ssh_form_grid")
                        .num_columns(2)
                        .spacing([12.0, 5.0])
                        .show(ui, |ui| {
                            ui.label("SSH Host & Port");
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut state.ssh_host).desired_width(200.0));
                                ui.add(egui::TextEdit::singleline(&mut state.ssh_port).desired_width(60.0));
                            });
                            ui.end_row();

                            ui.label("SSH User");
                            ui.text_edit_singleline(&mut state.ssh_user);
                            ui.end_row();

                            ui.label("Identity Key");
                            ui.horizontal(|ui| {
                                ui.text_edit_singleline(&mut state.ssh_key);
                                if ui.small_button("Browse").clicked() {
                                    if let Some(f) = rfd::FileDialog::new().pick_file() {
                                        state.ssh_key = f.display().to_string();
                                    }
                                }
                            });
                            ui.end_row();
                        });
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, p, icon::FLOPPY_DISK, "Save server", true).clicked() {
                        save_clicked = true;
                    }
                    if secondary_button(ui, p, icon::X, "Cancel", true).clicked() {
                        keep_open = false;
                    }
                });
            });

        if save_clicked {
            let existing_id = state
                .editing_slot
                .and_then(|s| self.registry.get(s))
                .map(|sl| sl.profile.id.clone());
            match state.build_profile(existing_id) {
                Ok(profile) => {
                    let profile_id = profile.id.clone();
                    let res = if let Some(slot_id) = state.editing_slot {
                        self.registry.update(slot_id, profile).map(|_| slot_id)
                    } else {
                        self.registry.add(profile)
                    };
                    match res {
                        Ok(slot_id) => {
                            if !state.password.is_empty() {
                                self.storage
                                    .save_password(&profile_id, &state.password, state.save_in_keychain);
                            }
                            if let Err(err) = self.storage.save_registry(&self.registry) {
                                self.notify_error(format!("Could not save server list: {err}"));
                            }
                            self.active_slot = Some(slot_id);
                            keep_open = false;
                        }
                        Err(err) => {
                            state.error_msg = Some(err.to_string());
                        }
                    }
                }
                Err(err) => {
                    state.error_msg = Some(err);
                }
            }
        }

        if keep_open {
            self.modal = Some(state);
        }
    }
}
