use super::CottonDataApp;
use crate::theme::{highlight_mysql_editor_layout, Palette};
use crate::ui_kit::{
    caption, icon, inset_frame, modal_shell, pill, primary_button, secondary_button, styled_text_input, ModalConfig,
};
use cottondata_core::ServerSlotId;
use cottondata_sql::{bind_statements_params, format_param_literal, ParamBinding, ParamValueKind, QueryParamSpec};
use eframe::egui::{self, FontFamily, FontId, RichText};
use std::collections::HashMap;

pub(super) struct ParamModalState {
    pub(super) slot: ServerSlotId,
    pub(super) tab_id: u64,
    pub(super) max_rows: u32,
    pub(super) fingerprint: u64,
    pub(super) statements: Vec<String>,
    pub(super) params: Vec<QueryParamSpec>,
    pub(super) bindings: HashMap<String, ParamBinding>,
    pub(super) focus_first: bool,
}

impl ParamModalState {
    pub(super) fn new(
        slot: ServerSlotId,
        tab_id: u64,
        max_rows: u32,
        fingerprint: u64,
        statements: Vec<String>,
        params: Vec<QueryParamSpec>,
        saved: Option<&HashMap<String, ParamBinding>>,
    ) -> Self {
        let mut bindings = HashMap::with_capacity(params.len());
        for spec in &params {
            let existing = saved.and_then(|m| m.get(&spec.key)).cloned().unwrap_or_default();
            bindings.insert(spec.key.clone(), existing);
        }
        Self {
            slot,
            tab_id,
            max_rows,
            fingerprint,
            statements,
            params,
            bindings,
            focus_first: true,
        }
    }
}

impl CottonDataApp {
    pub(super) fn render_param_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(mut state) = self.param_modal.take() else {
            return;
        };
        let dialect = self.slot_dialect(Some(state.slot));
        let server_label = self.slot_display_name(state.slot);
        let subtitle = format!("{} parameter(s) · {server_label}", state.params.len());

        let cmd_run = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        let mut run_from_body = false;
        let mut run_from_footer = false;
        let mut cancel_clicked = false;
        let mut reset_clicked = false;

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "query_params_modal",
                icon_glyph: icon::SLIDERS,
                icon_color: p.accent,
                title: "Query Parameters",
                subtitle: Some(&subtitle),
                accent_bar: Some(p.accent),
                width: 600.0,
                max_height: Some(520.0),
                dismiss_on_backdrop: false,
            },
            |ui| {
                let full_w = ui.available_width();
                let label_w = 130.0;
                let kind_w = 96.0;
                let preview_w = 110.0;
                let input_w = (full_w - label_w - kind_w - preview_w - 28.0).max(140.0);

                for (idx, spec) in state.params.iter().enumerate() {
                    let binding = state.bindings.entry(spec.key.clone()).or_default();
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(label_w, 32.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&spec.label)
                                            .font(FontId::new(12.5, FontFamily::Monospace))
                                            .strong()
                                            .color(p.accent),
                                    )
                                    .truncate(),
                                );
                            },
                        );

                        let is_null = binding.kind == ParamValueKind::Null;
                        let resp = ui
                            .add_enabled_ui(!is_null, |ui| {
                                styled_text_input(
                                    ui,
                                    p,
                                    ("param_val_input", state.fingerprint, &spec.key),
                                    &mut binding.value,
                                    if is_null { "NULL" } else { "Enter value…" },
                                    input_w,
                                )
                            })
                            .inner;
                        if idx == 0 && state.focus_first && !is_null {
                            resp.request_focus();
                            state.focus_first = false;
                        }
                        if idx + 1 == state.params.len()
                            && resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            run_from_body = true;
                        }

                        egui::ComboBox::from_id_salt(("param_kind_combo", &spec.key))
                            .selected_text(binding.kind.label())
                            .width(kind_w)
                            .show_ui(ui, |ui| {
                                for kind in ParamValueKind::ALL {
                                    ui.selectable_value(&mut binding.kind, kind, kind.label());
                                }
                            });

                        let literal = format_param_literal(binding, dialect);
                        ui.allocate_ui_with_layout(
                            egui::vec2(preview_w, 32.0),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                pill(
                                    ui,
                                    &super::truncate_display(&literal, 16),
                                    if binding.kind == ParamValueKind::Null {
                                        p.text_muted
                                    } else {
                                        p.success
                                    },
                                );
                            },
                        );
                    });
                    ui.add_space(6.0);
                }

                ui.add_space(6.0);
                caption(ui, p, "Bound SQL preview (values are remembered for this query):");
                ui.add_space(4.0);
                let bound_preview = bind_statements_params(&state.statements, dialect, &state.bindings).join(";\n");
                let mut preview_buf = bound_preview;
                let palette_copy = p;
                let mut layouter = move |ui: &egui::Ui, string: &str, _wrap_width: f32| {
                    highlight_mysql_editor_layout(ui, string, palette_copy)
                };
                inset_frame(p).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("param_preview_scroll")
                        .max_height(150.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut preview_buf)
                                    .code_editor()
                                    .frame(false)
                                    .layouter(&mut layouter)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(5),
                            );
                        });
                });
            },
            |ui| {
                if secondary_button(ui, p, icon::ARROW_COUNTER_CLOCKWISE, "Reset", true).clicked() {
                    reset_clicked = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, p, icon::PLAY, "Run Query", true)
                        .on_hover_text("Bind parameters and run (Cmd/Ctrl+Enter)")
                        .clicked()
                    {
                        run_from_footer = true;
                    }
                    if secondary_button(ui, p, icon::X, "Cancel", true).clicked() {
                        cancel_clicked = true;
                    }
                });
            },
        );

        let run_clicked = cmd_run || run_from_body || run_from_footer;

        if reset_clicked {
            for binding in state.bindings.values_mut() {
                *binding = ParamBinding::default();
            }
            self.param_modal = Some(state);
            return;
        }

        if run_clicked {
            let bound = bind_statements_params(&state.statements, dialect, &state.bindings);
            self.param_history.insert(state.fingerprint, state.bindings);
            self.dispatch_statements(state.slot, state.tab_id, bound, state.max_rows);
            return;
        }

        if !outcome.closed && !cancel_clicked {
            self.param_modal = Some(state);
        }
    }
}
