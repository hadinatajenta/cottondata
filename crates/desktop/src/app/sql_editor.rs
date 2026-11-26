use super::completion_popup::{
    collect_visible_completions, consume_completion_keys, render_completion_popup, typed_word, CompletionKeyAction,
};
use super::{insert_completion, CottonDataApp};
use crate::theme::{highlight_mysql_editor_with_matches, Palette};
use crate::ui_kit::{
    banner, caption, ghost_button, icon, icon_button, inset_frame, primary_button, secondary_button,
    styled_text_input, tinted_ghost_button, toolbar_divider,
};
use crate::worker::SlotCmd;
use cottondata_sql::{
    analyze_statement, duplicate_lines, find_matches, format_mysql, move_lines_down, move_lines_up,
    replace_all_matches, replace_match, statement_at_offset, toggle_line_comment, EditResult,
    SearchOptions, WriteClass,
};
use eframe::egui::{self, text::CCursor, text::CCursorRange, text::LayoutJob, FontFamily, FontId, RichText, Stroke, TextFormat};

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

        let (is_running, safety_warning, has_selection, slot_for_tab) = {
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

        if open_find_bar {
            self.open_sql_find(false);
        }
        if save_snippet {
            let sql = self.tabs[self.active_tab_idx].sql.clone();
            self.open_save_snippet_modal(sql);
        }

        if let Some(warn) = safety_warning {
            banner(ui, p, p.warning, icon::WARNING, &format!("Safety guard: {warn}"), false);
            ui.add_space(4.0);
        }

        let mut sql_edited = false;
        let mut editor_lost_focus = false;

        let tab_id = self.tabs[self.active_tab_idx].id;
        let editor_id = egui::Id::new(("sql_editor_text", tab_id));

        if self.tabs[self.active_tab_idx].find_replace.open {
            let (edited, replaced_count) = self.render_find_replace_bar(ui, p, editor_id);
            if edited {
                sql_edited = true;
            }
            if let Some(count) = replaced_count {
                self.notify_info(format!("Replaced {count} occurrence(s)"));
            }
            ui.add_space(4.0);
        }

        let is_editor_focused = ui.memory(|m| m.has_focus(editor_id));
        if is_editor_focused && !self.is_any_modal_open() {
            let (toggle_comment, dup_line, move_up, move_down) = ui.input_mut(|i| {
                let tc = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash);
                let dl = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D);
                let mu = i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowUp);
                let md = i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown);
                (tc, dl, mu, md)
            });
            let edit_result: Option<EditResult> = {
                let tab = &self.tabs[self.active_tab_idx];
                if toggle_comment {
                    Some(toggle_line_comment(&tab.sql, tab.cursor_byte, tab.selection_byte_range))
                } else if dup_line {
                    Some(duplicate_lines(&tab.sql, tab.cursor_byte, tab.selection_byte_range))
                } else if move_up {
                    Some(move_lines_up(&tab.sql, tab.cursor_byte, tab.selection_byte_range))
                } else if move_down {
                    Some(move_lines_down(&tab.sql, tab.cursor_byte, tab.selection_byte_range))
                } else {
                    None
                }
            };
            if let Some(res) = edit_result {
                let tab = &mut self.tabs[self.active_tab_idx];
                if res.sql != tab.sql || res.cursor_byte != tab.cursor_byte || res.selection != tab.selection_byte_range
                {
                    tab.sql = res.sql;
                    tab.cursor_byte = res.cursor_byte;
                    tab.selection_byte_range = res.selection;
                    tab.pending_cursor_range = Some(
                        res.selection
                            .unwrap_or((res.cursor_byte, res.cursor_byte)),
                    );
                    sql_edited = true;
                }
            }
        }

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
        let mut caret_screen_rect = None;

        let (search_matches, active_match_idx) = {
            let tab = &self.tabs[self.active_tab_idx];
            if tab.find_replace.open && !tab.find_replace.query.is_empty() {
                let opts = SearchOptions {
                    case_sensitive: tab.find_replace.case_sensitive,
                    whole_word: tab.find_replace.whole_word,
                };
                let matches = find_matches(&tab.sql, &tab.find_replace.query, opts);
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

        let palette_copy = p;
        let mut layouter = move |ui: &egui::Ui, string: &str, _wrap_width: f32| {
            highlight_mysql_editor_with_matches(ui, string, palette_copy, &search_matches, active_match_idx)
        };

        let editor_h = self.editor_height;
        let font_id = FontId::new(13.0, FontFamily::Monospace);
        let row_height = ui.fonts(|f| f.row_height(&font_id)).max(16.0);
        let min_visible_lines = ((editor_h - 10.0) / row_height).floor().max(6.0) as usize;

        if let Some((lo_byte, hi_byte)) = self.tabs[self.active_tab_idx].pending_cursor_range.take() {
            let sql_ref = &self.tabs[self.active_tab_idx].sql;
            let lo_char = byte_to_char_index(sql_ref, lo_byte);
            let hi_char = byte_to_char_index(sql_ref, hi_byte);
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), editor_id) {
                state.cursor.set_char_range(Some(CCursorRange::two(
                    CCursor::new(lo_char),
                    CCursor::new(hi_char),
                )));
                state.store(ui.ctx(), editor_id);
            }
        }

        inset_frame(p).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt(("sql_editor_scroll", tab_id))
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
                            .id(editor_id)
                            .min_size(egui::vec2(editor_min_w, editor_min_h))
                            .desired_width(f32::INFINITY)
                            .code_editor()
                            .frame(false)
                            .layouter(&mut layouter)
                            .show(ui);
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
            let new_cursor = insert_completion(&mut tab.sql, tab.cursor_byte, &chosen.label);
            tab.cursor_byte = new_cursor;
            tab.selection_byte_range = None;
            tab.pending_cursor_range = Some((new_cursor, new_cursor));
            tab.completion_popup.is_active = false;
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
            tab.sql = format_mysql(&tab.sql);
            sql_edited = true;
        }
        if sql_edited {
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
    ) -> (bool, Option<usize>) {
        let mut sql_edited = false;
        let mut replaced_all_count = None;
        let tab = &mut self.tabs[self.active_tab_idx];
        let opts = SearchOptions {
            case_sensitive: tab.find_replace.case_sensitive,
            whole_word: tab.find_replace.whole_word,
        };
        let matches = find_matches(&tab.sql, &tab.find_replace.query, opts);
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
            tab.cursor_byte = hi;
            tab.selection_byte_range = Some((lo, hi));
            tab.pending_cursor_range = Some((lo, hi));
        } else if do_replace_one && !matches.is_empty() {
            let idx = tab.find_replace.active_match.min(matches.len() - 1);
            let range = matches[idx];
            let (new_sql, end_byte) = replace_match(&tab.sql, range, &tab.find_replace.replace_with);
            tab.sql = new_sql;
            tab.cursor_byte = end_byte;
            tab.selection_byte_range = None;
            tab.pending_cursor_range = Some((end_byte, end_byte));
            sql_edited = true;
        } else if do_replace_all && !matches.is_empty() {
            let (new_sql, count) = replace_all_matches(
                &tab.sql,
                &tab.find_replace.query,
                &tab.find_replace.replace_with,
                opts,
            );
            if count > 0 {
                tab.sql = new_sql;
                tab.cursor_byte = tab.cursor_byte.min(tab.sql.len());
                tab.selection_byte_range = None;
                sql_edited = true;
                replaced_all_count = Some(count);
            }
        }

        if close_bar {
            tab.find_replace.open = false;
            ui.memory_mut(|m| m.request_focus(editor_id));
        }

        (sql_edited, replaced_all_count)
    }
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
