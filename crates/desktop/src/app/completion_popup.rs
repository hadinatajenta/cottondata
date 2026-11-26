use crate::theme::{Palette, RADIUS_CONTROL};
use crate::ui_kit::icon;
use cottondata_sql::{CompletionItem, CompletionKind, CompletionProvider, SchemaTableHint};
use eframe::egui::{
    self, text::LayoutJob, Align2, Color32, CornerRadius, FontFamily, FontId, Margin, Rect, Sense,
    TextFormat,
};

const MAX_COMPLETIONS: usize = 50;
const MAX_VISIBLE_ROWS: usize = 10;
const ROW_HEIGHT: f32 = 24.0;
const POPUP_WIDTH: f32 = 420.0;
const FOOTER_HEIGHT: f32 = 24.0;
const CARET_GAP: f32 = 4.0;

#[derive(Clone, Default)]
pub(super) struct CompletionPopupState {
    pub(super) selected_idx: usize,
    pub(super) is_active: bool,
    pub(super) should_scroll_to_selected: bool,
    anchor: (usize, usize),
}

impl CompletionPopupState {
    pub(super) fn sync_anchor(&mut self, cursor_byte: usize, sql_len: usize) {
        if self.anchor == (cursor_byte, sql_len) {
            return;
        }
        let is_cursor_move_without_edit = self.anchor.1 == sql_len;
        if is_cursor_move_without_edit {
            self.is_active = false;
        }
        self.anchor = (cursor_byte, sql_len);
        self.selected_idx = 0;
        self.should_scroll_to_selected = true;
    }
}

pub(super) enum CompletionKeyAction {
    Accept(usize),
    Dismiss,
}

pub(super) fn word_prefix_start(text_before_cursor: &str) -> usize {
    text_before_cursor
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '`')))
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

pub(super) fn collect_visible_completions(
    sql: &str,
    cursor_byte: usize,
    hints: &[SchemaTableHint],
) -> Vec<CompletionItem> {
    let clamped = cursor_byte.min(sql.len());
    if !sql.is_char_boundary(clamped) {
        return Vec::new();
    }
    let typed_prefix = typed_word(sql, clamped);
    if typed_prefix.is_empty() {
        return Vec::new();
    }
    CompletionProvider::suggest(sql, clamped, hints, MAX_COMPLETIONS)
        .into_iter()
        .filter(|candidate| !candidate.label.eq_ignore_ascii_case(typed_prefix))
        .collect()
}

pub(super) fn consume_completion_keys(
    ui: &egui::Ui,
    state: &mut CompletionPopupState,
    item_count: usize,
) -> Option<CompletionKeyAction> {
    if item_count == 0 {
        return None;
    }
    ui.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
            return Some(CompletionKeyAction::Dismiss);
        }
        if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
            state.selected_idx = (state.selected_idx + 1) % item_count;
            state.should_scroll_to_selected = true;
        }
        if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
            state.selected_idx = state.selected_idx.checked_sub(1).unwrap_or(item_count - 1);
            state.should_scroll_to_selected = true;
        }
        let is_accept_pressed = input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
            || input.consume_key(egui::Modifiers::NONE, egui::Key::Tab);
        is_accept_pressed
            .then(|| CompletionKeyAction::Accept(state.selected_idx.min(item_count - 1)))
    })
}

pub(super) fn render_completion_popup(
    ctx: &egui::Context,
    palette: Palette,
    popup_id: egui::Id,
    caret_rect: Rect,
    typed_prefix: &str,
    items: &[CompletionItem],
    state: &mut CompletionPopupState,
) -> Option<usize> {
    let visible_rows = items.len().min(MAX_VISIBLE_ROWS);
    let list_height = visible_rows as f32 * ROW_HEIGHT;
    let popup_height = list_height + FOOTER_HEIGHT + 12.0;
    let is_room_below =
        caret_rect.bottom() + CARET_GAP + popup_height <= ctx.screen_rect().bottom();
    let (anchor, pivot) = if is_room_below {
        (
            caret_rect.left_bottom() + egui::vec2(0.0, CARET_GAP),
            Align2::LEFT_TOP,
        )
    } else {
        (
            caret_rect.left_top() - egui::vec2(0.0, CARET_GAP),
            Align2::LEFT_BOTTOM,
        )
    };
    let should_scroll = std::mem::take(&mut state.should_scroll_to_selected);
    let selected_idx = state.selected_idx.min(items.len().saturating_sub(1));

    egui::Area::new(popup_id)
        .order(egui::Order::Foreground)
        .pivot(pivot)
        .fixed_pos(anchor)
        .constrain(true)
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(palette.bg_card)
                .stroke(egui::Stroke::new(1.0, palette.border))
                .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                .shadow(ctx.style().visuals.popup_shadow)
                .inner_margin(Margin::same(4))
                .show(ui, |ui| {
                    ui.set_width(POPUP_WIDTH);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut clicked_idx = None;
                    egui::ScrollArea::vertical()
                        .id_salt(popup_id.with("list"))
                        .max_height(list_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (idx, candidate) in items.iter().enumerate() {
                                let is_selected = idx == selected_idx;
                                let row_response = paint_completion_row(
                                    ui,
                                    palette,
                                    candidate,
                                    typed_prefix,
                                    is_selected,
                                );
                                if is_selected && should_scroll {
                                    row_response.scroll_to_me(None);
                                }
                                if row_response.clicked() {
                                    clicked_idx = Some(idx);
                                }
                            }
                        });
                    paint_footer(ui, palette);
                    clicked_idx
                })
                .inner
        })
        .inner
}

fn paint_completion_row(
    ui: &mut egui::Ui,
    palette: Palette,
    candidate: &CompletionItem,
    typed_prefix: &str,
    is_selected: bool,
) -> egui::Response {
    let (row_rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    if is_selected {
        ui.painter()
            .rect_filled(row_rect, CornerRadius::same(4), palette.accent_soft);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(row_rect, CornerRadius::same(4), palette.bg_hover);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let (glyph, glyph_color) = kind_glyph(candidate.kind, palette);
    let painter = ui.painter_at(row_rect);
    painter.text(
        egui::pos2(row_rect.left() + 8.0, row_rect.center().y),
        Align2::LEFT_CENTER,
        glyph,
        FontId::proportional(14.0),
        glyph_color,
    );

    let detail_galley = ui.fonts(|fonts| {
        fonts.layout_no_wrap(
            candidate.detail.clone(),
            FontId::proportional(11.5),
            palette.text_muted,
        )
    });
    let detail_width = detail_galley.size().x.min(row_rect.width() * 0.4);
    let detail_pos = egui::pos2(
        row_rect.right() - 8.0 - detail_width,
        row_rect.center().y - detail_galley.size().y / 2.0,
    );

    let label_left = row_rect.left() + 30.0;
    let label_max_width = (detail_pos.x - label_left - 12.0).max(40.0);
    let label_job = build_label_job(&candidate.label, typed_prefix, palette, label_max_width);
    let label_galley = ui.fonts(|fonts| fonts.layout_job(label_job));
    painter.galley(
        egui::pos2(
            label_left,
            row_rect.center().y - label_galley.size().y / 2.0,
        ),
        label_galley,
        palette.text_primary,
    );

    let detail_clip = Rect::from_min_max(
        egui::pos2(detail_pos.x, row_rect.top()),
        row_rect.right_bottom(),
    );
    ui.painter_at(detail_clip)
        .galley(detail_pos, detail_galley, palette.text_muted);
    response
}

fn paint_footer(ui: &mut egui::Ui, palette: Palette) {
    ui.add_space(4.0);
    let (footer_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), FOOTER_HEIGHT),
        Sense::hover(),
    );
    ui.painter().hline(
        footer_rect.x_range(),
        footer_rect.top(),
        egui::Stroke::new(1.0, palette.divider),
    );
    ui.painter().text(
        egui::pos2(footer_rect.left() + 8.0, footer_rect.center().y + 1.0),
        Align2::LEFT_CENTER,
        "↑↓ navigate  ·  Enter / Tab insert  ·  Esc close",
        FontId::proportional(11.0),
        palette.text_muted,
    );
}

fn build_label_job(label: &str, typed_prefix: &str, palette: Palette, max_width: f32) -> LayoutJob {
    let font_id = FontId::new(13.0, FontFamily::Monospace);
    let matched_len = matched_prefix_len(label, typed_prefix);
    let mut job = LayoutJob::default();
    job.wrap.max_width = max_width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let segments = [
        (&label[..matched_len], palette.accent),
        (&label[matched_len..], palette.text_primary),
    ];
    for (segment, color) in segments {
        if !segment.is_empty() {
            job.append(
                segment,
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color,
                    ..Default::default()
                },
            );
        }
    }
    job
}

fn matched_prefix_len(label: &str, typed_prefix: &str) -> usize {
    let typed_clean = typed_prefix.trim_start_matches('`');
    let is_prefix_match = label
        .get(..typed_clean.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(typed_clean));
    if is_prefix_match {
        typed_clean.len()
    } else {
        0
    }
}

fn kind_glyph(kind: CompletionKind, palette: Palette) -> (&'static str, Color32) {
    match kind {
        CompletionKind::Table => (icon::TABLE, palette.icon_table),
        CompletionKind::Column => (icon::COLUMNS, palette.accent),
        CompletionKind::Function => (icon::FUNCTION, palette.sql_number),
        CompletionKind::Keyword => (icon::TEXT_AA, palette.sql_keyword),
        CompletionKind::SystemVar => (icon::AT, palette.sql_sysvar),
    }
}

pub(super) fn typed_word(sql: &str, cursor_byte: usize) -> &str {
    let before = &sql[..cursor_byte];
    &before[word_prefix_start(before)..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merchant_hints() -> Vec<SchemaTableHint> {
        vec![
            SchemaTableHint {
                name: "merchant_data".into(),
                columns: vec![("id".into(), "bigint".into())],
            },
            SchemaTableHint {
                name: "merchant_data_param".into(),
                columns: vec![("id".into(), "bigint".into())],
            },
        ]
    }

    #[test]
    fn returns_no_completions_when_cursor_is_not_on_a_word() {
        let sql = "SELECT * FROM ";
        assert!(collect_visible_completions(sql, sql.len(), &merchant_hints()).is_empty());
    }

    #[test]
    fn hides_candidate_that_exactly_matches_typed_word() {
        let sql = "SELECT * FROM merchant_data";
        let labels: Vec<String> = collect_visible_completions(sql, sql.len(), &merchant_hints())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect();
        assert_eq!(labels, vec!["merchant_data_param".to_string()]);
    }

    #[test]
    fn suggests_tables_matching_typed_prefix() {
        let sql = "SELECT * FROM merch";
        let labels: Vec<String> = collect_visible_completions(sql, sql.len(), &merchant_hints())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect();
        assert_eq!(
            labels,
            vec![
                "merchant_data".to_string(),
                "merchant_data_param".to_string()
            ]
        );
    }

    #[test]
    fn ignores_cursor_inside_multibyte_character() {
        let sql = "SELECT 'é'";
        let inside_accent = sql.find('é').unwrap() + 1;
        assert!(collect_visible_completions(sql, inside_accent, &merchant_hints()).is_empty());
    }

    #[test]
    fn matched_prefix_is_case_insensitive_and_skips_backtick() {
        assert_eq!(matched_prefix_len("merchant_data", "MERCH"), 5);
        assert_eq!(matched_prefix_len("merchant_data", "`merch"), 5);
        assert_eq!(matched_prefix_len("merchant_data", "data"), 0);
    }
}
