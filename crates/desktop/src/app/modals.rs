use super::server_form::{ConnectionTestState, ServerFormTab, ServerModalState};
use super::CottonDataApp;
use crate::theme::{highlight_mysql_editor_layout, Palette};
use crate::ui_kit::{
    banner, busy_indicator, caption, env_pill_button, field_label, icon, inset_frame, modal_shell, option_card,
    password_input, pill, primary_button, secondary_button, section_heading, segmented_tabs, styled_text_input,
    toggle_card, ModalConfig,
};
use crate::worker::SlotCmd;
use cottondata_core::{DatabaseEngine, EnvironmentTag, TlsMode};
use eframe::egui::{self, Color32, RichText};

impl CottonDataApp {
    pub(super) fn render_modals(&mut self, ctx: &egui::Context, p: Palette) {
        self.render_sessions_modal(ctx, p);
        self.render_row_diff_modal(ctx, p);
        self.render_unsaved_guard_modal(ctx, p);
        self.render_sql_preview_modal(ctx, p);
        self.render_server_modal(ctx, p);
    }

    fn render_unsaved_guard_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(pending_action) = self.unsaved_guard_modal.clone() else {
            return;
        };
        let (deletes, updates, inserts) = self.tabs[self.active_tab_idx].mutator.pending_summary();
        let mut decision: Option<u8> = None;

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "unsaved_changes_modal",
                icon_glyph: icon::WARNING,
                icon_color: p.warning,
                title: "Unsaved Cell Changes",
                subtitle: Some("Commit or discard staged edits before continuing"),
                accent_bar: Some(p.warning),
                width: 440.0,
                max_height: None,
                dismiss_on_backdrop: true,
            },
            |ui| {
                ui.horizontal(|ui| {
                    if deletes > 0 {
                        pill(ui, &format!("DELETE {deletes}"), p.danger);
                    }
                    if updates > 0 {
                        pill(ui, &format!("UPDATE {updates}"), p.warning);
                    }
                    if inserts > 0 {
                        pill(ui, &format!("INSERT {inserts}"), p.success);
                    }
                });
                ui.add_space(10.0);
                ui.label(
                    RichText::new("This tab has uncommitted edits staged in memory.")
                        .size(13.0)
                        .color(p.text_primary),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Save your changes to the database first, or discard them to continue.")
                        .size(12.0)
                        .color(p.text_muted),
                );
            },
            |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
            },
        );

        if outcome.closed && decision.is_none() {
            decision = Some(0);
        }

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

    fn render_sql_preview_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some((tab_id, script, statements)) = self.sql_preview_modal.clone() else {
            return;
        };
        let active_slot = self.tabs[self.active_tab_idx].slot.or(self.active_slot);
        let server_info = active_slot
            .and_then(|slot| self.registry.get(slot))
            .map(|s| (format!("{} {}", s.slot_id, s.profile.name), s.profile.environment));
        let is_prod = server_info
            .as_ref()
            .is_some_and(|(_, env)| *env == EnvironmentTag::Production);
        let subtitle = match &server_info {
            Some((name, _)) => format!("{} statement(s) · {name}", statements.len()),
            None => format!("{} statement(s)", statements.len()),
        };

        let mut submit_now = false;
        let mut cancel_now = false;

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "sql_preview_modal",
                icon_glyph: icon::CODE,
                icon_color: if is_prod { p.danger } else { p.accent },
                title: "Preview Staged Transaction",
                subtitle: Some(&subtitle),
                accent_bar: Some(if is_prod { p.danger } else { p.accent }),
                width: 660.0,
                max_height: None,
                dismiss_on_backdrop: false,
            },
            |ui| {
                if is_prod {
                    banner(
                        ui,
                        p,
                        p.danger,
                        icon::WARNING,
                        "Production server target — verify every statement carefully before committing.",
                        false,
                    );
                    ui.add_space(10.0);
                }
                caption(
                    ui,
                    p,
                    "All changes run atomically in order: DELETE → UPDATE → INSERT (rolled back on any error).",
                );
                ui.add_space(8.0);
                let palette_copy = p;
                let mut layouter = move |ui: &egui::Ui, string: &str, _wrap_width: f32| {
                    highlight_mysql_editor_layout(ui, string, palette_copy)
                };
                let mut preview_buf = script.clone();
                inset_frame(p).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("sql_preview_scroll")
                        .max_height(300.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut preview_buf)
                                    .code_editor()
                                    .frame(false)
                                    .layouter(&mut layouter)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(12),
                            );
                        });
                });
            },
            |ui| {
                if secondary_button(ui, p, icon::COPY, "Copy SQL", true).clicked() {
                    ui.ctx().copy_text(script.clone());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, p, icon::CHECK_CIRCLE, "Commit transaction", true).clicked() {
                        submit_now = true;
                    }
                    if secondary_button(ui, p, icon::X, "Cancel", true).clicked() {
                        cancel_now = true;
                    }
                });
            },
        );

        if submit_now {
            if let Some(slot) = active_slot {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    tab.is_submitting_mutations = true;
                }
                self.hub.send(slot, SlotCmd::SubmitMutations { tab_id, statements });
            }
            self.sql_preview_modal = None;
        } else if cancel_now || outcome.closed {
            self.sql_preview_modal = None;
            self.after_save_action = None;
        }
    }

    fn render_server_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(mut state) = self.modal.take() else {
            return;
        };
        let mut keep_open = true;
        let mut save_clicked = false;
        let mut test_clicked = false;

        let [er, eg, eb] = state.environment.rgb();
        let env_color = Color32::from_rgb(er, eg, eb);
        let title = if let Some(slot) = state.editing_slot {
            format!("Edit Connection ({slot})")
        } else {
            format!(
                "New Connection (Slot {}/{})",
                self.registry.count() + 1,
                self.registry.max_servers()
            )
        };
        let subtitle = if state.use_unix_socket && state.engine.supports_unix_socket() {
            format!("{} · {}", state.engine.label(), state.socket_path)
        } else {
            format!("{} · {}:{}", state.engine.label(), state.host, state.port)
        };

        let cmd_enter = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        if cmd_enter {
            save_clicked = true;
        }
        let test_state_snapshot = state.test_state.clone();

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "server_connection_modal",
                icon_glyph: icon::DATABASE,
                icon_color: env_color,
                title: &title,
                subtitle: Some(&subtitle),
                accent_bar: Some(env_color),
                width: 600.0,
                max_height: Some(540.0),
                dismiss_on_backdrop: false,
            },
            |ui| {
                if let Some(err) = &state.error_msg {
                    banner(ui, p, p.danger, icon::WARNING, err, false);
                    ui.add_space(10.0);
                }

                ui.horizontal(|ui| {
                    segmented_tabs(
                        ui,
                        p,
                        &mut state.active_tab,
                        &[
                            (ServerFormTab::General, icon::PLUG, "General"),
                            (ServerFormTab::Security, icon::SHIELD_CHECK, "Security & TLS"),
                            (ServerFormTab::Ssh, icon::LOCK_KEY, "SSH Tunnel"),
                        ],
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let uri_label = if state.show_uri_bar { "Hide URL" } else { "Paste URL" };
                        if secondary_button(ui, p, icon::LINK, uri_label, true)
                            .on_hover_text("Parse mysql:// or sqlserver:// connection URL")
                            .clicked()
                        {
                            state.show_uri_bar = !state.show_uri_bar;
                        }
                    });
                });

                if state.show_uri_bar {
                    ui.add_space(10.0);
                    inset_frame(p).show(ui, |ui| {
                        field_label(ui, p, "Connection URL (mysql:// or sqlserver://)");
                        let avail = ui.available_width();
                        ui.horizontal(|ui| {
                            let input_w = (avail - 86.0).max(160.0);
                            let resp = styled_text_input(
                                ui,
                                p,
                                "server_uri_input",
                                &mut state.uri_input,
                                "mysql://user:pass@127.0.0.1:3306/mydb?ssl-mode=required",
                                input_w,
                            );
                            let submit_uri =
                                resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if submit_uri || primary_button(ui, p, icon::CHECK, "Apply", true).clicked() {
                                let raw = state.uri_input.clone();
                                match state.apply_connection_uri(&raw) {
                                    Ok(()) => {
                                        state.show_uri_bar = false;
                                        state.uri_input.clear();
                                    }
                                    Err(err) => {
                                        state.error_msg = Some(err);
                                    }
                                }
                            }
                        });
                    });
                }

                ui.add_space(14.0);

                match state.active_tab {
                    ServerFormTab::General => render_general_tab(ui, p, &mut state),
                    ServerFormTab::Security => render_security_tab(ui, p, &mut state),
                    ServerFormTab::Ssh => render_ssh_tab(ui, p, &mut state),
                }
            },
            |ui| {
                let is_testing = matches!(test_state_snapshot, ConnectionTestState::Testing(_));
                if secondary_button(ui, p, icon::LIGHTNING, "Test Connection", !is_testing)
                    .on_hover_text("Verify connectivity, credentials, and TLS before saving")
                    .clicked()
                {
                    test_clicked = true;
                }

                match &test_state_snapshot {
                    ConnectionTestState::Idle => {}
                    ConnectionTestState::Testing(_) => {
                        ui.add_space(6.0);
                        busy_indicator(ui, p, "Testing…");
                    }
                    ConnectionTestState::Success { summary, elapsed_ms } => {
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{} {summary} · {elapsed_ms} ms", icon::CHECK_CIRCLE))
                                    .size(11.5)
                                    .color(p.success),
                            )
                            .truncate(),
                        );
                    }
                    ConnectionTestState::Failed(err) => {
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{} Test failed", icon::WARNING))
                                    .size(11.5)
                                    .color(p.danger),
                            )
                            .truncate(),
                        )
                        .on_hover_text(err);
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, p, icon::FLOPPY_DISK, "Save Server", true)
                        .on_hover_text("Save server profile (Cmd/Ctrl+Enter)")
                        .clicked()
                    {
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

        if test_clicked {
            let existing_id = state
                .editing_slot
                .and_then(|s| self.registry.get(s))
                .map(|sl| sl.profile.id.clone());
            match state.build_profile(existing_id) {
                Ok(profile) => {
                    self.test_conn_seq += 1;
                    let req_id = self.test_conn_seq;
                    state.error_msg = None;
                    state.test_state = ConnectionTestState::Testing(req_id);
                    let pw = (!state.password.is_empty()).then(|| state.password.clone());
                    self.hub.test_connection_async(req_id, profile, pw);
                }
                Err(err) => {
                    state.error_msg = Some(err);
                }
            }
        }

        if save_clicked {
            let existing_id = state
                .editing_slot
                .and_then(|s| self.registry.get(s))
                .map(|sl| sl.profile.id.clone());
            match state.build_profile(existing_id) {
                Ok(profile) => {
                    let profile_id = profile.id.clone();
                    let saved_name = profile.name.clone();
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
                            } else {
                                self.notify_success(format!("Saved server {slot_id} {saved_name}"));
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

fn render_general_tab(ui: &mut egui::Ui, p: Palette, state: &mut ServerModalState) {
    let full_w = ui.available_width();
    let gap = 12.0;

    section_heading(ui, p, "Database Engine");
    ui.add_space(6.0);
    let card_w = (full_w - gap) / 2.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for engine in DatabaseEngine::ALL {
            let subtitle = match engine {
                DatabaseEngine::MySql => "MySQL 5.7+ / 8.x / MariaDB · Port 3306",
                DatabaseEngine::SqlServer => "Microsoft SQL Server · Port 1433",
            };
            if option_card(
                ui,
                p,
                icon::DATABASE,
                engine.label(),
                subtitle,
                state.engine == engine,
                card_w,
            )
            .clicked()
            {
                state.switch_engine(engine);
                state.test_state = ConnectionTestState::Idle;
            }
        }
    });

    ui.add_space(14.0);
    let name_w = (full_w - gap) * 0.56;
    let env_w = full_w - gap - name_w;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.vertical(|ui| {
            ui.set_width(name_w);
            field_label(ui, p, "Connection Name");
            styled_text_input(ui, p, "srv_name", &mut state.name, "e.g. Prod Main DB", name_w);
        });
        ui.vertical(|ui| {
            ui.set_width(env_w);
            field_label(ui, p, "Environment");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 4.0);
                for env in [
                    EnvironmentTag::Local,
                    EnvironmentTag::Development,
                    EnvironmentTag::Staging,
                    EnvironmentTag::Production,
                ] {
                    let [r, g, b] = env.rgb();
                    if env_pill_button(ui, p, env.label(), Color32::from_rgb(r, g, b), state.environment == env)
                        .clicked()
                    {
                        state.environment = env;
                    }
                }
            });
        });
    });

    ui.add_space(12.0);
    if state.engine.supports_unix_socket() {
        ui.horizontal(|ui| {
            field_label(ui, p, "Transport");
            ui.add_space(8.0);
            segmented_tabs(
                ui,
                p,
                &mut state.use_unix_socket,
                &[(false, icon::GLOBE, "TCP/IP"), (true, icon::PLUG, "Unix Socket")],
            );
        });
        ui.add_space(10.0);
    }

    if state.use_unix_socket && state.engine.supports_unix_socket() {
        field_label(ui, p, "Unix Socket Path");
        styled_text_input(ui, p, "srv_socket", &mut state.socket_path, "/tmp/mysql.sock", full_w);
    } else {
        let port_w = 110.0;
        let host_w = (full_w - gap - port_w).max(140.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.vertical(|ui| {
                ui.set_width(host_w);
                field_label(ui, p, "Host");
                styled_text_input(ui, p, "srv_host", &mut state.host, "127.0.0.1 or db.internal", host_w);
            });
            ui.vertical(|ui| {
                ui.set_width(port_w);
                field_label(ui, p, "Port");
                styled_text_input(ui, p, "srv_port", &mut state.port, "3306", port_w);
            });
        });
    }

    ui.add_space(12.0);
    let half_w = (full_w - gap) / 2.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.vertical(|ui| {
            ui.set_width(half_w);
            field_label(ui, p, "Username");
            styled_text_input(ui, p, "srv_user", &mut state.username, "root", half_w);
        });
        ui.vertical(|ui| {
            ui.set_width(half_w);
            field_label(ui, p, "Password");
            password_input(
                ui,
                p,
                "srv_pass",
                &mut state.password,
                &mut state.show_password,
                "Optional password",
                half_w,
            );
        });
    });

    ui.add_space(12.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.vertical(|ui| {
            ui.set_width(half_w);
            let schema_label = match state.engine {
                DatabaseEngine::MySql => "Default Schema",
                DatabaseEngine::SqlServer => "Default Database",
            };
            field_label(ui, p, schema_label);
            styled_text_input(ui, p, "srv_schema", &mut state.default_schema, "Optional", half_w);
        });
        ui.vertical(|ui| {
            ui.set_width(half_w);
            field_label(ui, p, "Credential Vault");
            ui.add_space(4.0);
            let vault_label = if cfg!(target_os = "macos") {
                "Store password in macOS Keychain"
            } else {
                "Keep password in memory for session"
            };
            ui.checkbox(&mut state.save_in_keychain, vault_label);
        });
    });
}

fn render_security_tab(ui: &mut egui::Ui, p: Palette, state: &mut ServerModalState) {
    let full_w = ui.available_width();
    let gap = 10.0;

    section_heading(ui, p, "Transport Security (TLS)");
    ui.add_space(6.0);
    let card_w = (full_w - gap) / 2.0;
    let tls_modes = TlsMode::all();
    for pair in tls_modes.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &mode in pair {
                let desc = match mode {
                    TlsMode::Disabled => "Plaintext TCP without encryption",
                    TlsMode::Preferred => "Encrypt if server supports TLS",
                    TlsMode::Required => "Require TLS (skip CA verify)",
                    TlsMode::VerifyCa => "Require TLS + verify CA chain",
                    TlsMode::VerifyIdentity => "Strict CA + hostname verification",
                };
                if option_card(ui, p, icon::SHIELD_CHECK, mode.label(), desc, state.tls == mode, card_w).clicked() {
                    state.tls = mode;
                }
            }
        });
        ui.add_space(6.0);
    }

    if matches!(state.tls, TlsMode::VerifyCa | TlsMode::VerifyIdentity) {
        ui.add_space(6.0);
        field_label(ui, p, "CA Certificate PEM");
        let browse_w = 96.0;
        let path_w = (full_w - browse_w - 8.0).max(160.0);
        ui.horizontal(|ui| {
            styled_text_input(ui, p, "srv_ca_cert", &mut state.ca_cert_path, "/path/to/ca.pem", path_w);
            if secondary_button(ui, p, icon::FOLDER_OPEN, "Browse", true).clicked() {
                if let Some(f) = rfd::FileDialog::new().pick_file() {
                    state.ca_cert_path = f.display().to_string();
                }
            }
        });
    }

    ui.add_space(12.0);
    section_heading(ui, p, "Safety & Paging");
    ui.add_space(6.0);
    toggle_card(
        ui,
        p,
        &mut state.read_only,
        "Read-Only Guard",
        "Prevents accidental INSERT, UPDATE, DELETE, inline edits, and DDL changes on this server.",
    );

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        field_label(ui, p, "Default Page Size");
        ui.add_space(10.0);
        segmented_tabs(
            ui,
            p,
            &mut state.page_size,
            &[
                (100u32, "", "100"),
                (500, "", "500"),
                (1000, "", "1,000"),
                (2000, "", "2,000"),
            ],
        );
        caption(ui, p, "rows / page");
    });
}

fn render_ssh_tab(ui: &mut egui::Ui, p: Palette, state: &mut ServerModalState) {
    let full_w = ui.available_width();
    let gap = 12.0;

    toggle_card(
        ui,
        p,
        &mut state.ssh_enabled,
        "Connect through SSH Tunnel",
        "Forward database TCP traffic securely through a jump host or bastion server.",
    );

    if !state.ssh_enabled {
        ui.add_space(10.0);
        caption(
            ui,
            p,
            "Enable SSH Tunnel above to configure bastion host, port, username, and private key.",
        );
        return;
    }

    ui.add_space(14.0);
    let port_w = 110.0;
    let host_w = (full_w - gap - port_w).max(140.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.vertical(|ui| {
            ui.set_width(host_w);
            field_label(ui, p, "SSH Bastion Host");
            styled_text_input(ui, p, "ssh_host", &mut state.ssh_host, "bastion.example.com", host_w);
        });
        ui.vertical(|ui| {
            ui.set_width(port_w);
            field_label(ui, p, "SSH Port");
            styled_text_input(ui, p, "ssh_port", &mut state.ssh_port, "22", port_w);
        });
    });

    ui.add_space(12.0);
    field_label(ui, p, "SSH Username");
    styled_text_input(ui, p, "ssh_user", &mut state.ssh_user, "ubuntu", full_w);

    ui.add_space(12.0);
    field_label(ui, p, "Private Identity Key");
    let browse_w = 96.0;
    let path_w = (full_w - browse_w - 8.0).max(160.0);
    ui.horizontal(|ui| {
        styled_text_input(ui, p, "ssh_key", &mut state.ssh_key, "~/.ssh/id_ed25519", path_w);
        if secondary_button(ui, p, icon::FOLDER_OPEN, "Browse", true).clicked() {
            if let Some(f) = rfd::FileDialog::new().pick_file() {
                state.ssh_key = f.display().to_string();
            }
        }
    });
}
