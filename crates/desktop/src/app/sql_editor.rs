use super::completion_popup::{
    collect_visible_completions, consume_completion_keys, render_completion_popup, typed_word, CompletionKeyAction,
};
use super::query_tab::{limit_label, QUERY_LIMIT_PRESETS};
use super::sql_fold::{
    fnv1a64 as fold_fnv1a64, FoldProjection, FoldedTextBuffer, QueryExecOutcome,
};
use super::{insert_completion, CottonDataApp};
use crate::theme::{
    highlight_mysql_editor_cached, highlight_mysql_layout, highlight_sql_projected_cached, Palette,
    RADIUS_CONTROL,
};
use crate::ui_kit::{
    banner, caption, ghost_button, icon, icon_button, inset_frame, primary_button, secondary_button,
    styled_text_input, tinted_ghost_button, toolbar_divider,
};
use crate::worker::SlotCmd;
use cottondata_sql::{
    analyze_statement, compute_line_spans, duplicate_lines, format_mysql, gutter_fold_regions,
    line_index_for_byte, move_lines_down, move_lines_up, replace_all_matches, replace_match,
    statement_at_offset, toggle_line_comment, EditResult, FoldRegion, SearchOptions,
    SqlDiagSeverity, SqlDiagSource, SqlDiagnostic, SqlQuickFix, WriteClass,
};
use eframe::egui::{
    self, text::CCursor, text::CCursorRange, Align2, Color32, CornerRadius, FontFamily, FontId,
    Margin, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Vec2,
};

const GUTTER_STATUS_COL_W: f32 = 16.0;
const GUTTER_FOLD_COL_W: f32 = 16.0;
const MAX_FOLD_PREVIEW_LINES: usize = 12;

impl CottonDataApp {
    pub(super) fn open_sql_find(&mut self, show_replace: bool) {
        let tab = &mut self.tabs[self.active_tab_idx];
        tab.show_sql_editor = true;
        tab.find_replace.open = true;
        if show_replace {
            tab.find_replace.show_replace = true;
        }
        tab.find_replace.focus_find = true;
        if let Some((lo, hi)) = tab.selection_byte_range {
            if lo < hi && hi <= tab.sql.len() && tab.sql.is_char_boundary(lo) && tab.sql.is_char_boundary(hi) {
                let selected = &tab.sql[lo..hi];
                if !selected.contains('\n') && selected.len() <= 120 {
                    tab.find_replace.query = selected.to_string();
                }
            }
        }
    }

    pub(super) fn render_sql_editor_section(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut run_current = false;
        let mut run_all = false;
        let mut stop_query = false;
        let mut do_explain = None;
        let mut do_format = false;
        let mut toggle_expand_editor = false;
        let mut open_find_bar = false;
        let mut save_snippet = false;
        let mut do_fold_all = false;
        let mut do_unfold_all = false;
        let mut jump_to_next_diag = false;

        let (is_running, safety_warning, has_selection, slot_for_tab, has_collapsed_folds, exec_outcome) = {
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
                tab.selection_byte_range.is_some(),
                tab.slot.or(self.active_slot),
                tab.fold_state.has_collapsed(),
                tab.last_exec_outcome,
            )
        };

        let dialect = self.slot_dialect(slot_for_tab);
        let (hints, schema_loaded) = slot_for_tab
            .and_then(|s| {
                let active_db = self
                    .registry
                    .get(s)
                    .and_then(|sl| sl.runtime.as_ref())
                    .and_then(|r| r.current_schema.as_deref());
                self.catalogs[s.index()].as_ref().map(|cat| {
                    let loaded = active_db.is_some_and(|db| {
                        cat.schemas()
                            .iter()
                            .any(|sc| cat.resolve(sc.name).eq_ignore_ascii_case(db) && sc.l1_loaded)
                    });
                    (cat.completion_hints(active_db), loaded)
                })
            })
            .unwrap_or_else(|| (Vec::new(), false));

        let all_diagnostics: Vec<SqlDiagnostic> = {
            let tab = &mut self.tabs[self.active_tab_idx];
            let mut combined = Vec::new();
            let current_hash = fold_fnv1a64(tab.sql.as_bytes());
            if let Some(server_diag) = &tab.server_diagnostic {
                if tab.server_diagnostic_sql_hash == current_hash {
                    combined.push(server_diag.clone());
                } else {
                    tab.server_diagnostic = None;
                }
            }
            let lint_diags = tab
                .lint_cache
                .diagnostics(&tab.sql, dialect, &hints, schema_loaded);
            for d in lint_diags {
                let overlaps_server = combined
                    .first()
                    .is_some_and(|sd| d.start_byte < sd.end_byte && sd.start_byte < d.end_byte);
                if !overlaps_server {
                    combined.push(d.clone());
                }
            }
            combined
        };

        let diag_error_count = all_diagnostics
            .iter()
            .filter(|d| d.severity == SqlDiagSeverity::Error)
            .count();
        let diag_warn_count = all_diagnostics
            .iter()
            .filter(|d| d.severity == SqlDiagSeverity::Warning)
            .count();

        let effective_limit = self.effective_tab_limit(self.active_tab_idx);
        let mut selected_limit: Option<u32> = None;

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
            let limit_text = format!("Limit: {}", limit_label(effective_limit));
            egui::ComboBox::from_id_salt(("query_auto_limit", self.tabs[self.active_tab_idx].id))
                .selected_text(RichText::new(limit_text).size(12.0).color(p.text_primary))
                .width(102.0)
                .show_ui(ui, |ui| {
                    ui.label(
                        RichText::new("Server-side auto-limit")
                            .size(10.5)
                            .strong()
                            .color(p.text_muted),
                    );
                    for &(preset, label) in QUERY_LIMIT_PRESETS {
                        if ui.selectable_label(effective_limit == preset, label).clicked() {
                            selected_limit = Some(preset);
                        }
                    }
                })
                .response
                .on_hover_text("Auto-limit for SELECT queries without an explicit LIMIT/TOP");
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
            if icon_button(ui, p, icon::MINUS, "Fold all queries (Cmd/Ctrl+Alt+Shift+[)").clicked() {
                do_fold_all = true;
            }
            let unfold_resp = ui
                .add_enabled_ui(has_collapsed_folds, |ui| {
                    icon_button(ui, p, icon::PLUS, "Unfold all (Cmd/Ctrl+Alt+Shift+])")
                })
                .inner;
            if unfold_resp.clicked() {
                do_unfold_all = true;
            }
            if icon_button(ui, p, icon::MAGNIFYING_GLASS, "Find & Replace (Cmd/Ctrl+F / Cmd/Ctrl+H)").clicked() {
                open_find_bar = true;
            }
            if icon_button(ui, p, icon::BOOKMARK_SIMPLE, "Save query as snippet (Cmd/Ctrl+Shift+S)").clicked() {
                save_snippet = true;
            }
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
                let status_color = match exec_outcome {
                    Some(QueryExecOutcome::Failed) if !is_running => p.danger,
                    _ => p.text_muted,
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(&self.tabs[self.active_tab_idx].last_status_msg)
                            .size(11.5)
                            .color(status_color),
                    )
                    .truncate(),
                );
                if is_running {
                    ui.spinner();
                } else if let Some(outcome) = exec_outcome {
                    let (glyph, color, tip) = match outcome {
                        QueryExecOutcome::Succeeded => (icon::CHECK_CIRCLE, p.success, "Last query succeeded"),
                        QueryExecOutcome::Failed => (icon::X_CIRCLE, p.danger, "Last query failed"),
                    };
                    ui.label(RichText::new(glyph).size(14.0).color(color))
                        .on_hover_text(tip);
                }

                if diag_error_count > 0 || diag_warn_count > 0 {
                    let (tone, glyph, label) = if diag_error_count > 0 {
                        (
                            p.danger,
                            icon::X_CIRCLE,
                            if diag_warn_count > 0 {
                                format!("{diag_error_count} err · {diag_warn_count} warn")
                            } else {
                                format!("{diag_error_count} err")
                            },
                        )
                    } else {
                        (p.warning, icon::WARNING, format!("{diag_warn_count} warn"))
                    };
                    let btn = egui::Button::new(
                        RichText::new(format!("{glyph} {label}")).size(11.0).color(tone),
                    )
                    .fill(tone.gamma_multiply(0.12))
                    .stroke(Stroke::new(1.0, tone.gamma_multiply(0.4)))
                    .corner_radius(CornerRadius::same(255));
                    if ui
                        .add(btn)
                        .on_hover_text("Jump to next SQL issue & view fix suggestions (Alt+Enter / Cmd+.)")
                        .clicked()
                    {
                        jump_to_next_diag = true;
                    }
                }
            });
        });
        ui.add_space(4.0);

        let mut sql_edited = false;
        let mut fold_changed = false;
        let mut editor_lost_focus = false;
        let mut pending_quick_fix: Option<SqlQuickFix> = None;

        if let Some(new_limit) = selected_limit {
            self.tabs[self.active_tab_idx].row_limit = Some(new_limit);
        }
        if open_find_bar {
            self.open_sql_find(false);
        }
        if save_snippet {
            let sql = self.tabs[self.active_tab_idx].sql.clone();
            self.open_save_snippet_modal(sql);
        }
        if do_fold_all {
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.fold_state.fold_all_queries(&tab.sql, dialect);
            fold_changed = true;
        }
        if do_unfold_all {
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.fold_state.unfold_all();
            fold_changed = true;
        }

        if let Some(warn) = safety_warning {
            banner(ui, p, p.warning, icon::WARNING, &format!("Safety guard: {warn}"), false);
            ui.add_space(4.0);
        }

        let server_diag_banner = self.tabs[self.active_tab_idx].server_diagnostic.clone();
        if let Some(sdiag) = server_diag_banner {
            let mut dismiss_server_diag = false;
            let mut jump_to_server_diag = false;
            let line_label = {
                let spans = compute_line_spans(&self.tabs[self.active_tab_idx].sql);
                line_index_for_byte(&spans, sdiag.start_byte) + 1
            };
            egui::Frame::new()
                .fill(p.danger.gamma_multiply(0.09))
                .stroke(Stroke::new(1.0, p.danger.gamma_multiply(0.45)))
                .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    let row_w = ui.available_width();
                    let (row_rect, _) = ui.allocate_exact_size(Vec2::new(row_w, 24.0), Sense::hover());
                    let mut right_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(row_rect)
                            .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    );
                    right_ui.set_clip_rect(row_rect.intersect(ui.clip_rect()));
                    if icon_button(&mut right_ui, p, icon::X, "Dismiss error highlight").clicked() {
                        dismiss_server_diag = true;
                    }
                    if ghost_button(&mut right_ui, p, icon::ARROW_RIGHT, "Jump", true).clicked() {
                        jump_to_server_diag = true;
                    }
                    if let Some(first_fix) = sdiag.fixes.first() {
                        let fix_btn = egui::Button::new(
                            RichText::new(format!("{} {}", icon::SPARKLE, first_fix.title))
                                .size(11.5)
                                .color(p.accent),
                        )
                        .fill(p.accent_soft)
                        .stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.5)))
                        .corner_radius(CornerRadius::same(RADIUS_CONTROL));
                        if right_ui.add(fix_btn).clicked() {
                            pending_quick_fix = Some(first_fix.clone());
                        }
                    }
                    let left_right = (right_ui.min_rect().left() - 8.0).max(row_rect.left() + 40.0);
                    let left_rect = Rect::from_min_max(row_rect.min, Pos2::new(left_right, row_rect.max.y));
                    let mut left_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(left_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    left_ui.set_clip_rect(left_rect.intersect(ui.clip_rect()));
                    left_ui.label(RichText::new(icon::X_CIRCLE).size(13.5).color(p.danger));
                    let full_err_line = format!("Line {line_label}: {}", sdiag.message);
                    left_ui
                        .add(
                            egui::Label::new(
                                RichText::new(&full_err_line)
                                    .size(12.0)
                                    .strong()
                                    .color(p.text_primary),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&full_err_line);

                    if let Some(rec) = &sdiag.recommendation {
                        ui.add_space(2.0);
                        ui.add(
                            egui::Label::new(RichText::new(rec).size(11.5).color(p.text_muted)).wrap(),
                        );
                    }
                });
            ui.add_space(4.0);
            if dismiss_server_diag {
                self.tabs[self.active_tab_idx].server_diagnostic = None;
            }
            if jump_to_server_diag {
                let tab = &mut self.tabs[self.active_tab_idx];
                tab.fold_state.unfold_enclosing_byte(&tab.sql, dialect, sdiag.start_byte);
                tab.cursor_byte = sdiag.start_byte;
                tab.selection_byte_range = Some((sdiag.start_byte, sdiag.end_byte));
                tab.pending_cursor_range = Some((sdiag.start_byte, sdiag.end_byte));
                tab.quick_fix_popover_byte = Some(sdiag.start_byte);
                fold_changed = true;
            }
        }

        let tab_id = self.tabs[self.active_tab_idx].id;
        let editor_id = egui::Id::new(("sql_editor_text", tab_id));

        if jump_to_next_diag && !all_diagnostics.is_empty() {
            let tab = &mut self.tabs[self.active_tab_idx];
            let next = all_diagnostics
                .iter()
                .find(|d| d.start_byte > tab.cursor_byte)
                .unwrap_or(&all_diagnostics[0]);
            tab.fold_state.unfold_enclosing_byte(&tab.sql, dialect, next.start_byte);
            tab.cursor_byte = next.start_byte;
            tab.selection_byte_range = Some((next.start_byte, next.end_byte));
            tab.pending_cursor_range = Some((next.start_byte, next.end_byte));
            tab.quick_fix_popover_byte = Some(next.start_byte);
            ui.memory_mut(|m| m.request_focus(editor_id));
            fold_changed = true;
        }

        if self.tabs[self.active_tab_idx].find_replace.open {
            let (edited, replaced_count, unfolded_for_find) =
                self.render_find_replace_bar(ui, p, editor_id, dialect);
            if edited {
                sql_edited = true;
            }
            if unfolded_for_find {
                fold_changed = true;
            }
            if let Some(count) = replaced_count {
                self.notify_info(format!("Replaced {count} occurrence(s)"));
            }
            ui.add_space(4.0);
        }

        let now_secs = ui.input(|i| i.time);
        let is_editor_focused = ui.memory(|m| m.has_focus(editor_id));

        if is_editor_focused && !self.is_any_modal_open() {
            let (
                do_undo,
                do_redo,
                toggle_comment,
                dup_line,
                move_up,
                move_down,
                fold_cur,
                unfold_cur,
                fold_all_kb,
                unfold_all_kb,
                open_quick_fix,
                dismiss_quick_fix,
                intercepted_copy,
                intercepted_cut,
            ) = {
                let has_folds_now = self.tabs[self.active_tab_idx].fold_state.has_collapsed();
                let has_sel_now = self.tabs[self.active_tab_idx].selection_byte_range.is_some();
                let qf_open = self.tabs[self.active_tab_idx].quick_fix_popover_byte.is_some();
                ui.input_mut(|i| {
                    let redo = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z)
                        || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y);
                    let undo = !redo && i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
                    let tc = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash);
                    let dl = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D);
                    let mu = i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowUp);
                    let md = i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown);
                    let fa = i.consume_key(
                        egui::Modifiers::COMMAND | egui::Modifiers::ALT | egui::Modifiers::SHIFT,
                        egui::Key::OpenBracket,
                    );
                    let ua = i.consume_key(
                        egui::Modifiers::COMMAND | egui::Modifiers::ALT | egui::Modifiers::SHIFT,
                        egui::Key::CloseBracket,
                    );
                    let fc = !fa
                        && i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::ALT, egui::Key::OpenBracket);
                    let uc = !ua
                        && i.consume_key(
                            egui::Modifiers::COMMAND | egui::Modifiers::ALT,
                            egui::Key::CloseBracket,
                        );
                    let qf = i.consume_key(egui::Modifiers::ALT, egui::Key::Enter)
                        || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Period);
                    let esc_qf = qf_open && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);

                    let mut icopy = false;
                    let mut icut = false;
                    if has_folds_now && has_sel_now {
                        i.events.retain(|ev| match ev {
                            egui::Event::Copy => {
                                icopy = true;
                                false
                            }
                            egui::Event::Cut => {
                                icut = true;
                                false
                            }
                            _ => true,
                        });
                    }
                    (undo, redo, tc, dl, mu, md, fc, uc, fa, ua, qf, esc_qf, icopy, icut)
                })
            };

            if dismiss_quick_fix {
                self.tabs[self.active_tab_idx].quick_fix_popover_byte = None;
            }
            if open_quick_fix {
                let tab = &mut self.tabs[self.active_tab_idx];
                let spans = compute_line_spans(&tab.sql);
                let cur_line = line_index_for_byte(&spans, tab.cursor_byte);
                let found = all_diagnostics
                    .iter()
                    .find(|d| tab.cursor_byte >= d.start_byte && tab.cursor_byte <= d.end_byte)
                    .or_else(|| {
                        all_diagnostics
                            .iter()
                            .find(|d| line_index_for_byte(&spans, d.start_byte) == cur_line)
                    });
                tab.quick_fix_popover_byte = found.map(|d| d.start_byte);
            }

            if intercepted_copy {
                let tab = &self.tabs[self.active_tab_idx];
                if let Some((lo, hi)) = tab.selection_byte_range {
                    if lo < hi && hi <= tab.sql.len() && tab.sql.is_char_boundary(lo) && tab.sql.is_char_boundary(hi) {
                        ui.ctx().copy_text(tab.sql[lo..hi].to_string());
                    }
                }
            }
            if intercepted_cut {
                let tab = &mut self.tabs[self.active_tab_idx];
                if let Some((lo, hi)) = tab.selection_byte_range {
                    if lo < hi && hi <= tab.sql.len() && tab.sql.is_char_boundary(lo) && tab.sql.is_char_boundary(hi) {
                        ui.ctx().copy_text(tab.sql[lo..hi].to_string());
                        tab.undo_stack.record_before_edit(
                            &tab.sql,
                            tab.cursor_byte,
                            tab.selection_byte_range,
                            now_secs,
                            true,
                        );
                        let old_sql = tab.sql.clone();
                        tab.sql.drain(lo..hi);
                        tab.fold_state.on_doc_edit(&old_sql, lo, hi - lo, "");
                        tab.cursor_byte = lo;
                        tab.selection_byte_range = None;
                        tab.pending_cursor_range = Some((lo, lo));
                        tab.invalidate_stale_exec_state_on_edit();
                        sql_edited = true;
                        fold_changed = true;
                    }
                }
            }

            if do_undo {
                let tab = &mut self.tabs[self.active_tab_idx];
                if let Some(snap) = tab
                    .undo_stack
                    .undo(&tab.sql, tab.cursor_byte, tab.selection_byte_range)
                {
                    tab.sql = snap.sql;
                    tab.cursor_byte = snap.cursor_byte.min(tab.sql.len());
                    tab.selection_byte_range = snap.selection;
                    tab.fold_state.unfold_enclosing_byte(&tab.sql, dialect, tab.cursor_byte);
                    tab.pending_cursor_range =
                        Some(tab.selection_byte_range.unwrap_or((tab.cursor_byte, tab.cursor_byte)));
                    tab.invalidate_stale_exec_state_on_edit();
                    sql_edited = true;
                    fold_changed = true;
                }
            } else if do_redo {
                let tab = &mut self.tabs[self.active_tab_idx];
                if let Some(snap) = tab
                    .undo_stack
                    .redo(&tab.sql, tab.cursor_byte, tab.selection_byte_range)
                {
                    tab.sql = snap.sql;
                    tab.cursor_byte = snap.cursor_byte.min(tab.sql.len());
                    tab.selection_byte_range = snap.selection;
                    tab.fold_state.unfold_enclosing_byte(&tab.sql, dialect, tab.cursor_byte);
                    tab.pending_cursor_range =
                        Some(tab.selection_byte_range.unwrap_or((tab.cursor_byte, tab.cursor_byte)));
                    tab.invalidate_stale_exec_state_on_edit();
                    sql_edited = true;
                    fold_changed = true;
                }
            }

            if fold_all_kb {
                let tab = &mut self.tabs[self.active_tab_idx];
                tab.fold_state.fold_all_queries(&tab.sql, dialect);
                fold_changed = true;
            } else if unfold_all_kb {
                let tab = &mut self.tabs[self.active_tab_idx];
                tab.fold_state.unfold_all();
                fold_changed = true;
            } else if fold_cur {
                let tab = &mut self.tabs[self.active_tab_idx];
                let spans = compute_line_spans(&tab.sql);
                let line = line_index_for_byte(&spans, tab.cursor_byte);
                if tab.fold_state.fold_at_line(&tab.sql, dialect, line) {
                    fold_changed = true;
                }
            } else if unfold_cur {
                let tab = &mut self.tabs[self.active_tab_idx];
                let spans = compute_line_spans(&tab.sql);
                let line = line_index_for_byte(&spans, tab.cursor_byte);
                if tab.fold_state.unfold_at_line(&tab.sql, dialect, line) {
                    fold_changed = true;
                }
            }

            let edit_result: Option<EditResult> = {
                let tab = &mut self.tabs[self.active_tab_idx];
                if toggle_comment || dup_line || move_up || move_down {
                    let effective_sel = effective_selection_for_line_op(tab, dialect);
                    if toggle_comment {
                        Some(toggle_line_comment(&tab.sql, tab.cursor_byte, effective_sel))
                    } else if dup_line {
                        Some(duplicate_lines(&tab.sql, tab.cursor_byte, effective_sel))
                    } else if move_up {
                        Some(move_lines_up(&tab.sql, tab.cursor_byte, effective_sel))
                    } else {
                        Some(move_lines_down(&tab.sql, tab.cursor_byte, effective_sel))
                    }
                } else {
                    None
                }
            };
            if let Some(res) = edit_result {
                let tab = &mut self.tabs[self.active_tab_idx];
                if res.sql != tab.sql || res.cursor_byte != tab.cursor_byte || res.selection != tab.selection_byte_range
                {
                    tab.undo_stack.record_before_edit(
                        &tab.sql,
                        tab.cursor_byte,
                        tab.selection_byte_range,
                        now_secs,
                        true,
                    );
                    tab.sql = res.sql;
                    tab.cursor_byte = res.cursor_byte;
                    tab.selection_byte_range = res.selection;
                    tab.pending_cursor_range = Some(
                        res.selection
                            .unwrap_or((res.cursor_byte, res.cursor_byte)),
                    );
                    tab.invalidate_stale_exec_state_on_edit();
                    sql_edited = true;
                }
            }
        }

        if let Some(fix) = pending_quick_fix.take() {
            apply_quick_fix_to_tab(&mut self.tabs[self.active_tab_idx], &fix, dialect, now_secs);
            sql_edited = true;
            fold_changed = true;
        }

        let is_completion_allowed = is_editor_focused && !self.is_any_modal_open();
        let (completion_items, completion_prefix) = {
            let tab = &mut self.tabs[self.active_tab_idx];
            let cursor = tab.cursor_byte.min(tab.sql.len());
            tab.completion_popup.sync_anchor(cursor, tab.sql.len());
            if !is_completion_allowed {
                tab.completion_popup.is_active = false;
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space)) {
                tab.completion_popup.is_active = true;
            }
            if tab.completion_popup.is_active && tab.selection_byte_range.is_none() {
                let items = collect_visible_completions(&tab.sql, cursor, &hints);
                let prefix = if items.is_empty() {
                    String::new()
                } else {
                    typed_word(&tab.sql, cursor).to_string()
                };
                (items, prefix)
            } else {
                (Vec::new(), String::new())
            }
        };
        let mut accepted_completion = None;
        {
            let popup_state = &mut self.tabs[self.active_tab_idx].completion_popup;
            match consume_completion_keys(ui, popup_state, completion_items.len()) {
                Some(CompletionKeyAction::Accept(idx)) => accepted_completion = Some(idx),
                Some(CompletionKeyAction::Dismiss) => popup_state.is_active = false,
                None => {}
            }
        }

        let (search_matches, active_match_idx) = {
            let tab = &mut self.tabs[self.active_tab_idx];
            if tab.find_replace.open && !tab.find_replace.query.is_empty() {
                let matches = tab.find_replace.matches_for(&tab.sql).to_vec();
                let active = if matches.is_empty() {
                    None
                } else {
                    Some(tab.find_replace.active_match.min(matches.len() - 1))
                };
                (matches, active)
            } else {
                (Vec::new(), None)
            }
        };

        // Compute fold regions & projection
        let (all_regions, effective_folds, projection) = {
            let tab = &mut self.tabs[self.active_tab_idx];
            let regions = tab.fold_state.regions(&tab.sql, dialect).to_vec();
            let eff = tab.fold_state.effective_collapsed_regions(&tab.sql, dialect);
            let proj = FoldProjection::build(&tab.sql, &eff);
            (regions, eff, proj)
        };
        let gutter_regions: Vec<FoldRegion> = gutter_fold_regions(&all_regions)
            .into_iter()
            .cloned()
            .collect();

        if fold_changed {
            let tab = &mut self.tabs[self.active_tab_idx];
            let spans = compute_line_spans(&tab.sql);
            let cur_line = line_index_for_byte(&spans, tab.cursor_byte);
            if let Some(enclosing) = effective_folds
                .iter()
                .find(|r| cur_line > r.start_line && cur_line <= r.end_line)
            {
                let snap_byte = spans[enclosing.start_line].end;
                tab.cursor_byte = snap_byte;
                tab.selection_byte_range = None;
                tab.pending_cursor_range = Some((snap_byte, snap_byte));
            } else if tab.pending_cursor_range.is_none() {
                tab.pending_cursor_range = Some(
                    tab.selection_byte_range
                        .unwrap_or((tab.cursor_byte, tab.cursor_byte)),
                );
            }
        }

        let display_matches = if effective_folds.is_empty() {
            search_matches.clone()
        } else {
            projection.map_doc_ranges_to_display_bytes(&search_matches)
        };
        let display_active_match = active_match_idx.and_then(|idx| {
            let (doc_lo, doc_hi) = *search_matches.get(idx)?;
            if effective_folds.is_empty() {
                Some(idx)
            } else {
                let d_lo = projection.doc_byte_to_display_byte(doc_lo);
                let d_hi = projection.doc_byte_to_display_byte(doc_hi);
                display_matches.iter().position(|&(s, e)| s == d_lo && e == d_hi)
            }
        });

        let editor_h = self.editor_height;
        let font_id = FontId::new(13.0, FontFamily::Monospace);
        let row_height = ui.fonts(|f| f.row_height(&font_id)).max(16.0);
        let min_visible_lines = ((editor_h - 10.0) / row_height).floor().max(6.0) as usize;

        if let Some((lo_byte, hi_byte)) = self.tabs[self.active_tab_idx].pending_cursor_range.take() {
            let (lo_char, hi_char) = if effective_folds.is_empty() {
                let sql_ref = &self.tabs[self.active_tab_idx].sql;
                (
                    byte_to_char_index(sql_ref, lo_byte),
                    byte_to_char_index(sql_ref, hi_byte),
                )
            } else {
                (
                    projection.doc_byte_to_display_char(lo_byte),
                    projection.doc_byte_to_display_char(hi_byte),
                )
            };
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), editor_id) {
                state.cursor.set_char_range(Some(CCursorRange::two(
                    CCursor::new(lo_char),
                    CCursor::new(hi_char),
                )));
                state.store(ui.ctx(), editor_id);
            }
        }

        let has_mutating_input = is_editor_focused
            && ui.input(|i| {
                i.events.iter().any(|ev| {
                    matches!(
                        ev,
                        egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Cut
                            | egui::Event::Key {
                                key: egui::Key::Backspace
                                    | egui::Key::Delete
                                    | egui::Key::Enter
                                    | egui::Key::Tab,
                                pressed: true,
                                ..
                            }
                    )
                })
            });
        let force_new_undo_group = is_editor_focused
            && ui.input(|i| {
                i.events.iter().any(|ev| {
                    matches!(
                        ev,
                        egui::Event::Paste(_)
                            | egui::Event::Cut
                            | egui::Event::Key {
                                key: egui::Key::Enter,
                                pressed: true,
                                ..
                            }
                    )
                })
            });
        let pre_edit_snapshot = if has_mutating_input {
            let tab = &self.tabs[self.active_tab_idx];
            Some((tab.sql.clone(), tab.cursor_byte, tab.selection_byte_range))
        } else {
            None
        };

        let mut caret_screen_rect = None;
        let mut clicked_fold_toggle: Option<(usize, bool)> = None;
        let mut clicked_unfold_pill: Option<usize> = None;
        let mut hovered_diag_anchor: Option<(SqlDiagnostic, Rect)> = None;
        let mut keyboard_diag_anchor: Option<(SqlDiagnostic, Rect)> = None;

        let hl_cache_id = egui::Id::new(("sql_hl", tab_id));
        let full_sql_for_hl = self.tabs[self.active_tab_idx].sql.clone();
        let proj_display_for_hl = projection.display_text.clone();
        let proj_doc_segments: Vec<(usize, usize)> = projection
            .segments
            .iter()
            .map(|s| (s.doc_start, s.doc_end))
            .collect();
        let proj_fold_hash = projection.fold_hash();
        let has_active_folds = !effective_folds.is_empty();
        let palette_copy = p;

        let mut layouter = move |ui: &egui::Ui, string: &str, _wrap_width: f32| {
            if has_active_folds && string == proj_display_for_hl {
                highlight_sql_projected_cached(
                    ui,
                    hl_cache_id,
                    &full_sql_for_hl,
                    string,
                    &proj_doc_segments,
                    proj_fold_hash,
                    palette_copy,
                    &display_matches,
                    display_active_match,
                )
            } else {
                let matches_slice: &[(usize, usize)] = if !has_active_folds {
                    &display_matches
                } else {
                    &[]
                };
                let active = if !has_active_folds {
                    display_active_match
                } else {
                    None
                };
                highlight_mysql_editor_cached(ui, hl_cache_id, string, palette_copy, matches_slice, active)
            }
        };

        let total_doc_lines = compute_line_spans(&self.tabs[self.active_tab_idx].sql).len().max(1);
        let visible_rows_count = projection.visible_lines.len().max(1);
        let display_rows = visible_rows_count.max(min_visible_lines);
        let digit_width = total_doc_lines.max(display_rows).to_string().len().max(2);
        let mono_char_w = ui.fonts(|f| f.glyph_width(&font_id, '0')).max(7.5);
        let line_num_col_w = digit_width as f32 * mono_char_w;
        let gutter_w = 4.0 + GUTTER_STATUS_COL_W + line_num_col_w + 4.0 + GUTTER_FOLD_COL_W + 4.0;
        let gutter_h = (display_rows as f32 * row_height + 4.0).max(editor_h - 8.0);

        inset_frame(p).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt(("sql_editor_scroll", tab_id))
                .max_height(editor_h)
                .min_scrolled_height(editor_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let tab = &mut self.tabs[self.active_tab_idx];
                    let safe_cursor = tab.cursor_byte.min(tab.sql.len());
                    let active_doc_line = tab.sql[..safe_cursor].bytes().filter(|&b| b == b'\n').count();

                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let (gutter_rect, gutter_resp) =
                            ui.allocate_exact_size(Vec2::new(gutter_w, gutter_h), Sense::click());
                        ui.painter().vline(
                            gutter_rect.right(),
                            gutter_rect.y_range(),
                            Stroke::new(1.0, p.divider),
                        );
                        ui.add_space(8.0);

                        let editor_min_w = (ui.available_width() - 4.0).max(260.0);
                        let editor_min_h = (editor_h - 8.0).max(100.0);

                        let (out, final_projection, unfolded_boundary) = if !has_active_folds {
                            let out = egui::TextEdit::multiline(&mut tab.sql)
                                .id(editor_id)
                                .min_size(Vec2::new(editor_min_w, editor_min_h))
                                .desired_width(f32::INFINITY)
                                .code_editor()
                                .frame(false)
                                .layouter(&mut layouter)
                                .show(ui);
                            (out, projection, None)
                        } else {
                            let mut folded_buf = FoldedTextBuffer::new(
                                &mut tab.sql,
                                &mut tab.fold_state,
                                dialect,
                                projection,
                            );
                            let out = egui::TextEdit::multiline(&mut folded_buf)
                                .id(editor_id)
                                .min_size(Vec2::new(editor_min_w, editor_min_h))
                                .desired_width(f32::INFINITY)
                                .code_editor()
                                .frame(false)
                                .layouter(&mut layouter)
                                .show(ui);
                            let unfolded = folded_buf.unfolded_boundary_line;
                            let proj_after = folded_buf.projection;
                            (out, proj_after, unfolded)
                        };

                        if unfolded_boundary.is_some() {
                            fold_changed = true;
                            ui.ctx().request_repaint();
                        }

                        if out.response.changed() {
                            sql_edited = true;
                            tab.completion_popup.is_active = true;
                            ui.ctx().request_repaint();
                        }
                        if out.response.lost_focus() {
                            editor_lost_focus = true;
                        }
                        if let Some(range) = out.cursor_range {
                            let caret_rect = out
                                .galley
                                .pos_from_cursor(&range.primary)
                                .translate(out.galley_pos.to_vec2());
                            caret_screen_rect = ui.clip_rect().intersects(caret_rect).then_some(caret_rect);
                            let primary_idx = range.primary.ccursor.index;
                            let secondary_idx = range.secondary.ccursor.index;
                            if !has_active_folds && unfolded_boundary.is_none() {
                                tab.cursor_byte = char_index_to_byte(&tab.sql, primary_idx);
                                if primary_idx != secondary_idx {
                                    let lo = char_index_to_byte(&tab.sql, primary_idx.min(secondary_idx));
                                    let hi = char_index_to_byte(&tab.sql, primary_idx.max(secondary_idx));
                                    tab.selection_byte_range = (lo < hi).then_some((lo, hi));
                                } else {
                                    tab.selection_byte_range = None;
                                }
                            } else {
                                tab.cursor_byte = final_projection.display_char_to_doc_byte(primary_idx);
                                if primary_idx != secondary_idx {
                                    let (lo, hi) = final_projection
                                        .display_char_range_to_doc_byte_range(primary_idx, secondary_idx);
                                    tab.selection_byte_range = (lo < hi).then_some((lo, hi));
                                } else {
                                    tab.selection_byte_range = None;
                                }
                            }
                        }

                        // Exact row geometry anchored to TextEdit's galley
                        let row_y_range = |vis_row: usize| -> (f32, f32) {
                            if let Some(row) = out.galley.rows.get(vis_row) {
                                (
                                    out.galley_pos.y + row.rect.top(),
                                    out.galley_pos.y + row.rect.bottom(),
                                )
                            } else {
                                let base_y = out.galley_pos.y + vis_row as f32 * row_height;
                                (base_y, base_y + row_height)
                            }
                        };

                        let pointer_pos = ui.ctx().pointer_hover_pos();
                        let gutter_hovered = gutter_resp.hovered()
                            || pointer_pos.is_some_and(|pos| gutter_rect.contains(pos));
                        let alt_held = ui.input(|i| i.modifiers.alt);
                        let gutter_painter = ui.painter_at(gutter_rect);
                        let editor_painter = ui.painter_at(out.response.rect.expand(4.0));

                        let status_x_center = gutter_rect.left() + 4.0 + GUTTER_STATUS_COL_W * 0.5;
                        let line_num_right_x = gutter_rect.left() + 4.0 + GUTTER_STATUS_COL_W + line_num_col_w;
                        let fold_col_left = line_num_right_x + 4.0;
                        let fold_x_center = fold_col_left + GUTTER_FOLD_COL_W * 0.5;

                        let doc_line_spans = compute_line_spans(&tab.sql);
                        let mut hovered_expanded_region: Option<&FoldRegion> = None;

                        for vis_row in 0..display_rows {
                            let (ry_top, ry_bot) = row_y_range(vis_row);
                            if ry_bot < ui.clip_rect().top() - row_height
                                || ry_top > ui.clip_rect().bottom() + row_height
                            {
                                continue;
                            }
                            let center_y = (ry_top + ry_bot) * 0.5;
                            let doc_line = final_projection
                                .visible_lines
                                .get(vis_row)
                                .copied();

                            let line_display_num = doc_line
                                .map(|l| l + 1)
                                .unwrap_or_else(|| {
                                    let last_doc = final_projection.visible_lines.last().copied().unwrap_or(0);
                                    last_doc + 1 + (vis_row + 1 - final_projection.visible_lines.len())
                                });

                            let is_active = doc_line == Some(active_doc_line);
                            let num_color = if is_active { p.accent } else { p.text_muted };
                            gutter_painter.text(
                                Pos2::new(line_num_right_x, center_y),
                                Align2::RIGHT_CENTER,
                                format!("{line_display_num}"),
                                font_id.clone(),
                                num_color,
                            );

                            let Some(doc_line) = doc_line else {
                                continue;
                            };

                            // Exec status indicator (green checkmark / red X) or diagnostic dot
                            let exec_ind = tab
                                .exec_indicators
                                .iter()
                                .find(|ind| ind.start_line == doc_line);
                            let status_hit = Rect::from_center_size(
                                Pos2::new(status_x_center, center_y),
                                Vec2::new(GUTTER_STATUS_COL_W, row_height),
                            );
                            if let Some(ind) = exec_ind {
                                let (glyph, color) = match ind.outcome {
                                    QueryExecOutcome::Succeeded => (icon::CHECK_CIRCLE, p.success),
                                    QueryExecOutcome::Failed => (icon::X_CIRCLE, p.danger),
                                };
                                gutter_painter.text(
                                    Pos2::new(status_x_center, center_y),
                                    Align2::CENTER_CENTER,
                                    glyph,
                                    FontId::new(11.5, FontFamily::Proportional),
                                    color,
                                );
                                if pointer_pos.is_some_and(|pos| status_hit.contains(pos)) {
                                    egui::show_tooltip_at_pointer(
                                        ui.ctx(),
                                        ui.layer_id(),
                                        editor_id.with(("exec_tip", doc_line)),
                                        |ui| {
                                            ui.label(RichText::new(&ind.summary).size(11.5).color(p.text_primary));
                                        },
                                    );
                                }
                            } else {
                                let collapsed_end = final_projection
                                    .boundary_at_display_row(vis_row)
                                    .map(|b| b.region.end_line)
                                    .unwrap_or(doc_line);
                                let worst_sev = all_diagnostics
                                    .iter()
                                    .filter(|d| {
                                        let d_line = line_index_for_byte(&doc_line_spans, d.start_byte);
                                        d_line >= doc_line && d_line <= collapsed_end
                                    })
                                    .map(|d| d.severity)
                                    .max();
                                if let Some(sev) = worst_sev {
                                    let dot_color = diag_color(sev, p);
                                    gutter_painter.circle_filled(
                                        Pos2::new(status_x_center, center_y),
                                        3.2,
                                        dot_color,
                                    );
                                }
                            }

                            // Fold chevron in fold column
                            if let Some(region) = gutter_regions.iter().find(|r| r.start_line == doc_line) {
                                let is_collapsed = tab.fold_state.collapsed.contains(&doc_line);
                                let fold_hit = Rect::from_min_max(
                                    Pos2::new(fold_col_left, ry_top),
                                    Pos2::new(gutter_rect.right(), ry_bot),
                                );
                                let is_btn_hovered = pointer_pos.is_some_and(|pos| fold_hit.contains(pos));
                                if is_collapsed || gutter_hovered || is_btn_hovered {
                                    let glyph = if is_collapsed {
                                        icon::CARET_RIGHT
                                    } else {
                                        icon::CARET_DOWN
                                    };
                                    let color = if is_btn_hovered || is_collapsed {
                                        p.accent
                                    } else if gutter_hovered {
                                        p.text_muted
                                    } else {
                                        p.text_muted.gamma_multiply(0.55)
                                    };
                                    if is_btn_hovered {
                                        gutter_painter.rect_filled(
                                            fold_hit.shrink(1.0),
                                            CornerRadius::same(3),
                                            p.accent_soft.gamma_multiply(0.65),
                                        );
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    gutter_painter.text(
                                        Pos2::new(fold_x_center, center_y),
                                        Align2::CENTER_CENTER,
                                        glyph,
                                        FontId::new(10.5, FontFamily::Proportional),
                                        color,
                                    );
                                }
                                if is_btn_hovered && !is_collapsed {
                                    hovered_expanded_region = Some(region);
                                }
                                if gutter_resp.clicked() && is_btn_hovered {
                                    clicked_fold_toggle = Some((doc_line, alt_held));
                                }
                            }
                        }

                        // DataGrip-style hover scope vertical guide & subtle range tint
                        if let Some(region) = hovered_expanded_region {
                            if let Some(start_vis) = final_projection.doc_line_to_display_row(region.start_line) {
                                let end_vis = final_projection
                                    .doc_line_to_display_row(region.end_line)
                                    .unwrap_or_else(|| {
                                        final_projection
                                            .visible_lines
                                            .partition_point(|&l| l <= region.end_line)
                                            .saturating_sub(1)
                                    });
                                let (y0_top, y0_bot) = row_y_range(start_vis);
                                let (_, y1_bot) = row_y_range(end_vis);
                                let guide_stroke = Stroke::new(1.5, p.accent.gamma_multiply(0.65));
                                gutter_painter.vline(
                                    fold_x_center,
                                    (y0_bot - 1.0)..=y1_bot - 2.0,
                                    guide_stroke,
                                );
                                gutter_painter.hline(
                                    fold_x_center..=(fold_x_center + 4.0),
                                    y1_bot - 2.0,
                                    guide_stroke,
                                );
                                let scope_rect = Rect::from_min_max(
                                    Pos2::new(out.response.rect.left(), y0_top),
                                    Pos2::new(out.response.rect.right(), y1_bot),
                                );
                                editor_painter.rect_filled(
                                    scope_rect,
                                    CornerRadius::ZERO,
                                    p.accent_soft.gamma_multiply(0.24),
                                );
                            }
                        }

                        // Inline fold pills & hover SQL previews
                        for boundary in &final_projection.boundaries {
                            let Some(row) = out.galley.rows.get(boundary.header_display_row) else {
                                continue;
                            };
                            let (ry_top, ry_bot) = row_y_range(boundary.header_display_row);
                            if ry_bot < ui.clip_rect().top() - row_height
                                || ry_top > ui.clip_rect().bottom() + row_height
                            {
                                continue;
                            }
                            let label_text = boundary.region.badge_label();
                            let pill_font = FontId::new(10.5, FontFamily::Monospace);
                            let label_galley = ui.fonts(|f| {
                                f.layout_no_wrap(label_text.clone(), pill_font.clone(), p.text_muted)
                            });
                            let pill_w = label_galley.size().x + 12.0;
                            let pill_h = (row_height - 3.0).max(13.0);
                            let pill_x = out.galley_pos.x + row.rect.right() + 8.0;
                            let pill_y = (ry_top + ry_bot - pill_h) * 0.5;
                            let pill_rect = Rect::from_min_size(Pos2::new(pill_x, pill_y), Vec2::new(pill_w, pill_h));

                            let pill_resp = ui.interact(
                                pill_rect,
                                editor_id.with(("fold_pill", boundary.region.start_line)),
                                Sense::click(),
                            );
                            if pill_resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }

                            let hidden_worst_sev = all_diagnostics
                                .iter()
                                .filter(|d| {
                                    d.start_byte < boundary.hidden_doc_end
                                        && d.end_byte > boundary.hidden_doc_start
                                })
                                .map(|d| d.severity)
                                .max();
                            let has_hidden_match = search_matches.iter().any(|&(ms, me)| {
                                ms < boundary.hidden_doc_end && me > boundary.hidden_doc_start
                            });

                            let (fill, border_col, text_col) = if pill_resp.hovered() {
                                (p.accent_soft, p.accent, p.accent)
                            } else if let Some(sev) = hidden_worst_sev {
                                let c = diag_color(sev, p);
                                (c.gamma_multiply(0.14), c.gamma_multiply(0.65), c)
                            } else if has_hidden_match {
                                (
                                    p.warning.gamma_multiply(0.16),
                                    p.warning.gamma_multiply(0.65),
                                    p.warning,
                                )
                            } else {
                                (p.bg_hover, p.border, p.text_muted)
                            };

                            editor_painter.rect(
                                pill_rect,
                                CornerRadius::same(4),
                                fill,
                                Stroke::new(1.0, border_col),
                                StrokeKind::Inside,
                            );
                            editor_painter.text(
                                pill_rect.center(),
                                Align2::CENTER_CENTER,
                                label_text,
                                pill_font,
                                text_col,
                            );

                            if pill_resp.clicked() {
                                clicked_unfold_pill = Some(boundary.region.start_line);
                            }

                            if pill_resp.hovered() {
                                let preview_sql = build_fold_preview_snippet(
                                    &tab.sql,
                                    &doc_line_spans,
                                    boundary.region.start_line,
                                    boundary.region.end_line,
                                );
                                egui::show_tooltip_at_pointer(
                                    ui.ctx(),
                                    ui.layer_id(),
                                    editor_id.with(("fold_preview", boundary.region.start_line)),
                                    |ui| {
                                        ui.set_max_width(520.0);
                                        let preview_galley =
                                            highlight_mysql_layout(ui, &preview_sql, 500.0, p);
                                        ui.label(preview_galley);
                                    },
                                );
                            }
                        }

                        // Paint diagnostic squiggly underlines
                        for diag in &all_diagnostics {
                            let disp_ranges = if !has_active_folds {
                                vec![(
                                    diag.start_byte.min(tab.sql.len()),
                                    diag.end_byte.min(tab.sql.len()),
                                )]
                            } else {
                                final_projection.map_doc_ranges_to_display_bytes(&[(
                                    diag.start_byte,
                                    diag.end_byte,
                                )])
                            };
                            let color = diag_color(diag.severity, p);
                            for (d_lo, d_hi) in disp_ranges {
                                let text_ref = if !has_active_folds {
                                    tab.sql.as_str()
                                } else {
                                    final_projection.display_text.as_str()
                                };
                                let c_lo = byte_to_char_index(text_ref, d_lo);
                                let c_hi = byte_to_char_index(text_ref, d_hi).max(c_lo + 1);
                                let rects = screen_rects_for_char_range(
                                    &out.galley,
                                    out.galley_pos,
                                    c_lo,
                                    c_hi,
                                    mono_char_w,
                                );
                                for rect in rects {
                                    if rect.bottom() < ui.clip_rect().top()
                                        || rect.top() > ui.clip_rect().bottom()
                                    {
                                        continue;
                                    }
                                    if diag.severity == SqlDiagSeverity::Error {
                                        editor_painter.rect_filled(
                                            rect,
                                            CornerRadius::same(2),
                                            p.danger.gamma_multiply(0.11),
                                        );
                                    }
                                    paint_squiggly_underline(
                                        &editor_painter,
                                        rect.left(),
                                        rect.right(),
                                        rect.bottom() - 1.0,
                                        color,
                                    );
                                    if pointer_pos.is_some_and(|pos| rect.expand(2.0).contains(pos))
                                        && hovered_diag_anchor.is_none()
                                    {
                                        hovered_diag_anchor = Some((diag.clone(), rect));
                                    }
                                    if tab.quick_fix_popover_byte == Some(diag.start_byte)
                                        && keyboard_diag_anchor.is_none()
                                    {
                                        keyboard_diag_anchor = Some((diag.clone(), rect));
                                    }
                                }
                            }
                        }
                    });
                });
        });

        // Commit document-level undo snapshot if text actually mutated via TextEdit
        if let Some((pre_sql, pre_cursor, pre_sel)) = pre_edit_snapshot {
            let tab = &mut self.tabs[self.active_tab_idx];
            if tab.sql != pre_sql {
                tab.undo_stack
                    .record_before_edit(&pre_sql, pre_cursor, pre_sel, now_secs, force_new_undo_group);
                tab.invalidate_stale_exec_state_on_edit();
            }
        }

        if let Some((line, recursive)) = clicked_fold_toggle {
            let tab = &mut self.tabs[self.active_tab_idx];
            if recursive {
                tab.fold_state.toggle_line_recursive(&tab.sql, dialect, line);
            } else {
                tab.fold_state.toggle_line(line);
            }
            fold_changed = true;
            ui.ctx().request_repaint();
        }
        if let Some(line) = clicked_unfold_pill {
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.fold_state.collapsed.remove(&line);
            fold_changed = true;
            ui.ctx().request_repaint();
        }

        // Render Diagnostic / Quick-Fix Popover (hover OR keyboard Alt+Enter / Cmd+.)
        let active_popover_id = editor_id.with("diag_popover");
        let dismissed_key = active_popover_id.with("dismissed_byte");
        let dismissed_byte: Option<usize> = ui.ctx().data(|d| d.get_temp(dismissed_key));
        if dismissed_byte.is_some()
            && (keyboard_diag_anchor.is_some()
                || hovered_diag_anchor
                    .as_ref()
                    .is_none_or(|(d, _)| Some(d.start_byte) != dismissed_byte))
        {
            ui.ctx().data_mut(|d| d.remove::<usize>(dismissed_key));
        }
        let dismissed_byte: Option<usize> = ui.ctx().data(|d| d.get_temp(dismissed_key));
        let filtered_hover_anchor = hovered_diag_anchor.filter(|(d, _)| Some(d.start_byte) != dismissed_byte);

        let last_popover_rect: Option<Rect> = ui.ctx().data(|d| d.get_temp(active_popover_id));
        let last_popover_diag: Option<(SqlDiagnostic, Rect)> =
            ui.ctx().data(|d| d.get_temp(active_popover_id.with("diag")));
        let pointer_in_popover = ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|pos| last_popover_rect.is_some_and(|r| r.expand(10.0).contains(pos)));

        let popover_target = keyboard_diag_anchor
            .or(filtered_hover_anchor)
            .or(if pointer_in_popover { last_popover_diag } else { None });

        if let Some((diag, anchor_rect)) = popover_target {
            if !self.is_any_modal_open() && completion_items.is_empty() {
                let diag_start = diag.start_byte;
                let (pop_rect, chosen_fix, closed) =
                    render_diagnostic_popover(ui.ctx(), p, active_popover_id, anchor_rect, &diag);
                if closed {
                    self.tabs[self.active_tab_idx].quick_fix_popover_byte = None;
                    ui.ctx().data_mut(|d| {
                        d.remove::<Rect>(active_popover_id);
                        d.remove::<(SqlDiagnostic, Rect)>(active_popover_id.with("diag"));
                        d.insert_temp(dismissed_key, diag_start);
                    });
                    ui.ctx().request_repaint();
                } else {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(active_popover_id, pop_rect);
                        d.insert_temp(active_popover_id.with("diag"), (diag, anchor_rect));
                    });
                }
                if let Some(fix) = chosen_fix {
                    apply_quick_fix_to_tab(&mut self.tabs[self.active_tab_idx], &fix, dialect, now_secs);
                    sql_edited = true;
                    fold_changed = true;
                    ui.ctx().data_mut(|d| {
                        d.remove::<Rect>(active_popover_id);
                        d.remove::<(SqlDiagnostic, Rect)>(active_popover_id.with("diag"));
                        d.remove::<usize>(dismissed_key);
                    });
                    ui.memory_mut(|m| m.request_focus(editor_id));
                }
            }
        } else {
            ui.ctx().data_mut(|d| {
                d.remove::<Rect>(active_popover_id);
                d.remove::<(SqlDiagnostic, Rect)>(active_popover_id.with("diag"));
            });
        }

        if let Some(caret_rect) = caret_screen_rect.filter(|_| !completion_items.is_empty()) {
            let clicked_idx = render_completion_popup(
                ui.ctx(),
                p,
                editor_id.with("completion_popup"),
                caret_rect,
                &completion_prefix,
                &completion_items,
                &mut self.tabs[self.active_tab_idx].completion_popup,
            );
            if clicked_idx.is_some() {
                accepted_completion = clicked_idx;
                ui.memory_mut(|m| m.request_focus(editor_id));
            }
        }
        if let Some(chosen) = accepted_completion.and_then(|idx| completion_items.get(idx)) {
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.undo_stack.record_before_edit(
                &tab.sql,
                tab.cursor_byte,
                tab.selection_byte_range,
                now_secs,
                true,
            );
            let old_sql = tab.sql.clone();
            let old_cursor = tab.cursor_byte;
            let new_cursor = insert_completion(&mut tab.sql, tab.cursor_byte, &chosen.label);
            let inserted_len = new_cursor.saturating_sub(old_cursor);
            tab.fold_state.on_doc_edit(
                &old_sql,
                old_cursor.min(old_sql.len()),
                0,
                &tab.sql[old_cursor.min(tab.sql.len())..(old_cursor + inserted_len).min(tab.sql.len())],
            );
            tab.cursor_byte = new_cursor;
            tab.selection_byte_range = None;
            tab.pending_cursor_range = Some((new_cursor, new_cursor));
            tab.completion_popup.is_active = false;
            tab.invalidate_stale_exec_state_on_edit();
            sql_edited = true;
        }

        if toggle_expand_editor {
            self.editor_height = if self.editor_height >= 340.0 { 220.0 } else { 420.0 };
            self.mark_session_dirty();
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
            tab.undo_stack.record_before_edit(
                &tab.sql,
                tab.cursor_byte,
                tab.selection_byte_range,
                now_secs,
                true,
            );
            tab.sql = format_mysql(&tab.sql);
            tab.fold_state.unfold_all();
            tab.cursor_byte = tab.cursor_byte.min(tab.sql.len());
            tab.selection_byte_range = None;
            tab.pending_cursor_range = Some((tab.cursor_byte, tab.cursor_byte));
            tab.invalidate_stale_exec_state_on_edit();
            sql_edited = true;
            fold_changed = true;
        }
        if sql_edited || fold_changed {
            self.mark_session_dirty();
        }
        if editor_lost_focus && self.session_dirty_since.is_some() {
            self.persist_sessions_now();
        }
    }

    fn render_find_replace_bar(
        &mut self,
        ui: &mut egui::Ui,
        p: Palette,
        editor_id: egui::Id,
        dialect: cottondata_core::SqlDialect,
    ) -> (bool, Option<usize>, bool) {
        let mut sql_edited = false;
        let mut replaced_all_count = None;
        let mut unfolded_for_match = false;
        let now_secs = ui.input(|i| i.time);
        let tab = &mut self.tabs[self.active_tab_idx];
        let opts = SearchOptions {
            case_sensitive: tab.find_replace.case_sensitive,
            whole_word: tab.find_replace.whole_word,
        };
        let matches = tab.find_replace.matches_for(&tab.sql).to_vec();
        if !matches.is_empty() && tab.find_replace.active_match >= matches.len() {
            tab.find_replace.active_match = 0;
        }

        let mut step_next = false;
        let mut step_prev = false;
        let mut do_replace_one = false;
        let mut do_replace_all = false;
        let mut close_bar = false;

        inset_frame(p).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let toggle_glyph = if tab.find_replace.show_replace {
                    icon::CARET_DOWN
                } else {
                    icon::CARET_RIGHT
                };
                if icon_button(ui, p, toggle_glyph, "Toggle Replace (Cmd/Ctrl+H)").clicked() {
                    tab.find_replace.show_replace = !tab.find_replace.show_replace;
                }

                let find_resp = styled_text_input(
                    ui,
                    p,
                    ("sql_find_input", tab.id),
                    &mut tab.find_replace.query,
                    "Find in SQL…",
                    210.0,
                );
                if tab.find_replace.focus_find {
                    find_resp.request_focus();
                    tab.find_replace.focus_find = false;
                }
                if find_resp.changed() {
                    tab.find_replace.active_match = 0;
                }
                if find_resp.has_focus() || find_resp.lost_focus() {
                    let (enter, shift, esc) = ui.input(|i| {
                        (
                            i.key_pressed(egui::Key::Enter),
                            i.modifiers.shift,
                            i.key_pressed(egui::Key::Escape),
                        )
                    });
                    if esc {
                        close_bar = true;
                    } else if enter {
                        if shift {
                            step_prev = true;
                        } else {
                            step_next = true;
                        }
                        find_resp.request_focus();
                    }
                }

                ui.toggle_value(&mut tab.find_replace.case_sensitive, RichText::new("Aa").size(11.5))
                    .on_hover_text("Match case");
                ui.toggle_value(&mut tab.find_replace.whole_word, RichText::new("W").size(11.5))
                    .on_hover_text("Match whole word");

                let count_text = if tab.find_replace.query.is_empty() {
                    String::new()
                } else if matches.is_empty() {
                    "No matches".into()
                } else {
                    format!("{} / {}", tab.find_replace.active_match + 1, matches.len())
                };
                if !count_text.is_empty() {
                    caption(ui, p, count_text);
                }

                if ui
                    .add_enabled_ui(!matches.is_empty(), |ui| {
                        icon_button(ui, p, icon::ARROW_UP, "Previous match (Shift+Enter)")
                    })
                    .inner
                    .clicked()
                {
                    step_prev = true;
                }
                if ui
                    .add_enabled_ui(!matches.is_empty(), |ui| {
                        icon_button(ui, p, icon::ARROW_DOWN, "Next match (Enter)")
                    })
                    .inner
                    .clicked()
                {
                    step_next = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, p, icon::X, "Close (Esc)").clicked() {
                        close_bar = true;
                    }
                });
            });

            if tab.find_replace.show_replace {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(26.0);
                    let rep_resp = styled_text_input(
                        ui,
                        p,
                        ("sql_replace_input", tab.id),
                        &mut tab.find_replace.replace_with,
                        "Replace with…",
                        210.0,
                    );
                    if rep_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close_bar = true;
                    }
                    if secondary_button(ui, p, icon::ARROW_RIGHT, "Replace", !matches.is_empty()).clicked() {
                        do_replace_one = true;
                    }
                    if secondary_button(ui, p, icon::ARROWS_CLOCKWISE, "Replace all", !matches.is_empty()).clicked() {
                        do_replace_all = true;
                    }
                });
            }
        });

        if step_next && !matches.is_empty() {
            tab.find_replace.active_match = (tab.find_replace.active_match + 1) % matches.len();
            let (lo, hi) = matches[tab.find_replace.active_match];
            if tab.fold_state.unfold_enclosing_byte(&tab.sql, dialect, lo) {
                unfolded_for_match = true;
            }
            tab.cursor_byte = hi;
            tab.selection_byte_range = Some((lo, hi));
            tab.pending_cursor_range = Some((lo, hi));
        } else if step_prev && !matches.is_empty() {
            tab.find_replace.active_match = if tab.find_replace.active_match == 0 {
                matches.len() - 1
            } else {
                tab.find_replace.active_match - 1
            };
            let (lo, hi) = matches[tab.find_replace.active_match];
            if tab.fold_state.unfold_enclosing_byte(&tab.sql, dialect, lo) {
                unfolded_for_match = true;
            }
            tab.cursor_byte = hi;
            tab.selection_byte_range = Some((lo, hi));
            tab.pending_cursor_range = Some((lo, hi));
        } else if do_replace_one && !matches.is_empty() {
            let idx = tab.find_replace.active_match.min(matches.len() - 1);
            let range = matches[idx];
            tab.undo_stack.record_before_edit(
                &tab.sql,
                tab.cursor_byte,
                tab.selection_byte_range,
                now_secs,
                true,
            );
            if tab.fold_state.unfold_enclosing_byte(&tab.sql, dialect, range.0) {
                unfolded_for_match = true;
            }
            let old_sql = tab.sql.clone();
            let (new_sql, end_byte) = replace_match(&tab.sql, range, &tab.find_replace.replace_with);
            tab.fold_state.on_doc_edit(
                &old_sql,
                range.0,
                range.1.saturating_sub(range.0),
                &tab.find_replace.replace_with,
            );
            tab.sql = new_sql;
            tab.cursor_byte = end_byte;
            tab.selection_byte_range = None;
            tab.pending_cursor_range = Some((end_byte, end_byte));
            tab.invalidate_stale_exec_state_on_edit();
            sql_edited = true;
        } else if do_replace_all && !matches.is_empty() {
            let (new_sql, count) = replace_all_matches(
                &tab.sql,
                &tab.find_replace.query,
                &tab.find_replace.replace_with,
                opts,
            );
            if count > 0 {
                tab.undo_stack.record_before_edit(
                    &tab.sql,
                    tab.cursor_byte,
                    tab.selection_byte_range,
                    now_secs,
                    true,
                );
                tab.sql = new_sql;
                tab.fold_state.unfold_all();
                tab.cursor_byte = tab.cursor_byte.min(tab.sql.len());
                tab.selection_byte_range = None;
                tab.pending_cursor_range = Some((tab.cursor_byte, tab.cursor_byte));
                tab.invalidate_stale_exec_state_on_edit();
                sql_edited = true;
                unfolded_for_match = true;
                replaced_all_count = Some(count);
            }
        }

        if close_bar {
            tab.find_replace.open = false;
            ui.memory_mut(|m| m.request_focus(editor_id));
        }

        (sql_edited, replaced_all_count, unfolded_for_match)
    }
}

fn effective_selection_for_line_op(
    tab: &mut super::query_tab::QueryTab,
    dialect: cottondata_core::SqlDialect,
) -> Option<(usize, usize)> {
    if tab.selection_byte_range.is_some() {
        return tab.selection_byte_range;
    }
    let spans = compute_line_spans(&tab.sql);
    let cur_line = line_index_for_byte(&spans, tab.cursor_byte);
    if let Some(region) = tab.fold_state.collapsed_region_starting_at(&tab.sql, dialect, cur_line) {
        let start = spans.get(region.start_line)?.start;
        let end = spans.get(region.end_line)?.end;
        return Some((start, end));
    }
    None
}

fn apply_quick_fix_to_tab(
    tab: &mut super::query_tab::QueryTab,
    fix: &SqlQuickFix,
    dialect: cottondata_core::SqlDialect,
    now_secs: f64,
) {
    let Some((patched, new_cursor)) = SqlDiagnostic::apply_fix(&tab.sql, fix) else {
        return;
    };
    tab.undo_stack.record_before_edit(
        &tab.sql,
        tab.cursor_byte,
        tab.selection_byte_range,
        now_secs,
        true,
    );
    let old_sql = tab.sql.clone();
    tab.sql = patched;
    tab.fold_state.on_doc_edit(
        &old_sql,
        fix.start_byte,
        fix.end_byte.saturating_sub(fix.start_byte),
        &fix.replacement,
    );
    tab.fold_state.unfold_enclosing_byte(&tab.sql, dialect, new_cursor);
    tab.cursor_byte = new_cursor;
    tab.selection_byte_range = None;
    tab.pending_cursor_range = Some((new_cursor, new_cursor));
    tab.invalidate_stale_exec_state_on_edit();
}

fn render_diagnostic_popover(
    ctx: &egui::Context,
    p: Palette,
    popup_id: egui::Id,
    anchor_rect: Rect,
    diag: &SqlDiagnostic,
) -> (Rect, Option<SqlQuickFix>, bool) {
    let screen = ctx.screen_rect();
    let est_height = 110.0 + diag.fixes.len() as f32 * 28.0;
    let room_below = anchor_rect.bottom() + 6.0 + est_height <= screen.bottom();
    let (pos, pivot) = if room_below {
        (Pos2::new(anchor_rect.left(), anchor_rect.bottom() + 4.0), Align2::LEFT_TOP)
    } else {
        (Pos2::new(anchor_rect.left(), anchor_rect.top() - 4.0), Align2::LEFT_BOTTOM)
    };

    let mut chosen_fix = None;
    let mut closed = false;
    let (tone, glyph, sev_label) = match diag.severity {
        SqlDiagSeverity::Error => (p.danger, icon::X_CIRCLE, "Error"),
        SqlDiagSeverity::Warning => (p.warning, icon::WARNING, "Warning"),
        SqlDiagSeverity::Hint => (p.accent, icon::INFO, "Hint"),
    };
    let source_badge = match &diag.source {
        SqlDiagSource::StaticLint => format!("{sev_label} · SQL Lint"),
        SqlDiagSource::ServerError { code: Some(code) } => format!("{sev_label} · {code}"),
        SqlDiagSource::ServerError { code: None } => format!("{sev_label} · Server"),
    };

    let area_resp = egui::Area::new(popup_id)
        .order(egui::Order::Foreground)
        .pivot(pivot)
        .fixed_pos(pos)
        .constrain(true)
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(p.bg_card)
                .stroke(Stroke::new(1.0, tone.gamma_multiply(0.55)))
                .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                .shadow(ctx.style().visuals.popup_shadow)
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_max_width(380.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(glyph).size(13.5).color(tone));
                        ui.label(RichText::new(source_badge).size(11.0).strong().color(tone));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon_button(ui, p, icon::X, "Close (Esc)").clicked() {
                                closed = true;
                            }
                        });
                    });
                    ui.add_space(2.0);
                    ui.label(RichText::new(&diag.message).size(12.0).color(p.text_primary));
                    if let Some(rec) = &diag.recommendation {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("Suggestion: {rec}"))
                                .size(11.5)
                                .color(p.text_muted),
                        );
                    }
                    if !diag.fixes.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            for fix in &diag.fixes {
                                let btn = egui::Button::new(
                                    RichText::new(format!("{} {}", icon::SPARKLE, fix.title))
                                        .size(11.5)
                                        .color(p.accent),
                                )
                                .fill(p.accent_soft)
                                .stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.5)))
                                .corner_radius(CornerRadius::same(RADIUS_CONTROL));
                                if ui.add(btn).clicked() {
                                    chosen_fix = Some(fix.clone());
                                }
                            }
                        });
                    }
                });
        });

    (area_resp.response.rect, chosen_fix, closed)
}

fn diag_color(sev: SqlDiagSeverity, p: Palette) -> Color32 {
    match sev {
        SqlDiagSeverity::Error => p.danger,
        SqlDiagSeverity::Warning => p.warning,
        SqlDiagSeverity::Hint => p.accent,
    }
}

fn screen_rects_for_char_range(
    galley: &egui::Galley,
    galley_pos: Pos2,
    c_lo: usize,
    c_hi: usize,
    fallback_char_w: f32,
) -> Vec<Rect> {
    let start_cursor = galley.from_ccursor(CCursor::new(c_lo));
    let end_cursor = galley.from_ccursor(CCursor::new(c_hi));
    let start_row = start_cursor.rcursor.row;
    let end_row = end_cursor.rcursor.row;
    let mut rects = Vec::new();

    for r in start_row..=end_row {
        let Some(row) = galley.rows.get(r) else {
            break;
        };
        let x0 = if r == start_row {
            galley.pos_from_cursor(&start_cursor).left()
        } else {
            row.rect.left()
        };
        let mut x1 = if r == end_row {
            galley.pos_from_cursor(&end_cursor).left()
        } else {
            row.rect.right()
        };
        if x1 <= x0 + 2.0 {
            x1 = (x0 + fallback_char_w).max(row.rect.right().min(x0 + fallback_char_w * 2.0));
        }
        rects.push(Rect::from_min_max(
            Pos2::new(galley_pos.x + x0, galley_pos.y + row.rect.top()),
            Pos2::new(galley_pos.x + x1, galley_pos.y + row.rect.bottom()),
        ));
    }
    rects
}

fn paint_squiggly_underline(painter: &egui::Painter, x0: f32, x1: f32, y: f32, color: Color32) {
    let width = x1 - x0;
    if width < 2.0 {
        return;
    }
    let step = 3.2_f32;
    let amp = 1.4_f32;
    let mut points = Vec::with_capacity((width / step) as usize + 3);
    let mut x = x0;
    let mut up = false;
    while x < x1 {
        points.push(Pos2::new(x, if up { y - amp } else { y + amp * 0.5 }));
        x += step;
        up = !up;
    }
    points.push(Pos2::new(x1, if up { y - amp } else { y + amp * 0.5 }));
    painter.add(Shape::line(points, Stroke::new(1.25, color)));
}

fn build_fold_preview_snippet(
    sql: &str,
    spans: &[cottondata_sql::LineByteSpan],
    start_line: usize,
    end_line: usize,
) -> String {
    let total_lines = end_line.saturating_sub(start_line) + 1;
    let shown_end = end_line.min(start_line + MAX_FOLD_PREVIEW_LINES - 1);
    let mut out = String::new();
    for line in start_line..=shown_end {
        if let Some(span) = spans.get(line) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&sql[span.start..span.end]);
        }
    }
    if total_lines > MAX_FOLD_PREVIEW_LINES {
        let more = total_lines - MAX_FOLD_PREVIEW_LINES;
        out.push_str(&format!("\n-- … +{more} more line(s)"));
    }
    out
}

fn char_index_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(text.len())
}

fn byte_to_char_index(text: &str, byte_idx: usize) -> usize {
    let mut clamped = byte_idx.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    text[..clamped].chars().count()
}
