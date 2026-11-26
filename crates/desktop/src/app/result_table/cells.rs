use super::{ResultTableView, CELL_FONT_SIZE, CELL_PADDING_X};
use crate::app::truncate_display;
use cottondata_grid::ColumnarPage;
use eframe::egui::{
    self,
    text::{LayoutJob, TextWrapping},
    Color32, FontFamily, FontId, Galley, Rect, Stroke, TextFormat, Ui,
};
use std::borrow::Cow;
use std::sync::Arc;

const MIN_COLUMN_WIDTH: f32 = 72.0;
const MAX_COLUMN_WIDTH: f32 = 280.0;
const HEADER_ICON_ALLOWANCE: f32 = 40.0;
const WIDTH_SAMPLE_ROWS: usize = 40;
const MAX_RENDERED_CHARS: usize = 120;

pub(super) struct CellDisplay<'a> {
    pub(super) text: Cow<'a, str>,
    pub(super) is_null: bool,
    pub(super) background: Color32,
}

pub(crate) fn sanitize_cell_text<'a>(raw: &'a str, max_chars: usize) -> Cow<'a, str> {
    let has_control = raw
        .as_bytes()
        .iter()
        .any(|&b| matches!(b, b'\n' | b'\r' | b'\t'));
    if !has_control && (raw.len() <= max_chars || raw.chars().take(max_chars + 1).count() <= max_chars) {
        return Cow::Borrowed(raw);
    }

    let mut out = String::with_capacity(raw.len().min(max_chars.saturating_mul(4)));
    let mut chars = raw.chars();
    for ch in chars.by_ref().take(max_chars) {
        if matches!(ch, '\n' | '\r' | '\t') {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    if chars.next().is_some() {
        out.push('…');
    }
    Cow::Owned(out)
}

pub(in crate::app) fn compute_column_widths(ui: &Ui, page: &ColumnarPage) -> Vec<f32> {
    (0..page.columns.len())
        .map(|column_index| compute_single_column_width(ui, page, column_index))
        .collect()
}

pub(in crate::app) fn compute_single_column_width(ui: &Ui, page: &ColumnarPage, column_index: usize) -> f32 {
    let Some(column) = page.columns.get(column_index) else {
        return MIN_COLUMN_WIDTH;
    };
    let header_font = FontId::proportional(CELL_FONT_SIZE);
    let cell_font = FontId::monospace(CELL_FONT_SIZE);
    let sampled_rows = page.row_count.min(WIDTH_SAMPLE_ROWS);
    ui.fonts(|fonts| {
        let header_width = fonts
            .layout_no_wrap(column.name.clone(), header_font, Color32::WHITE)
            .size()
            .x
            + HEADER_ICON_ALLOWANCE;
        let widest_cell = (0..sampled_rows)
            .filter_map(|row| page.cell_str(row, column_index))
            .map(|text| {
                fonts
                    .layout_no_wrap(truncate_display(text, 48), cell_font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max);
        (header_width.max(widest_cell) + CELL_PADDING_X * 2.0).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
    })
}

pub(super) fn row_badge(view: &ResultTableView, row: usize) -> String {
    let page_rows = view.page.row_count;
    if row >= page_rows {
        format!("+{}", row - page_rows + 1)
    } else if view.mutator.is_deleted(row) {
        format!("-{}", row + 1)
    } else if view.mutator.has_row_modifications(row) {
        format!("*{}", row + 1)
    } else {
        (row + 1).to_string()
    }
}

pub(super) fn modified_cell_tooltip(view: &ResultTableView, row: usize, column: usize) -> Option<String> {
    if row >= view.page.row_count {
        return None;
    }
    let staged = view.mutator.get_modified(row, column)?;
    let original = view.page.cell_value(row, column);
    Some(format!(
        "Original: {}\nStaged: {}",
        truncate_display(original.as_display_str(), 120),
        truncate_display(staged.as_display_str(), 120),
    ))
}

fn cell_display<'a>(view: &'a ResultTableView<'a>, row: usize, column: usize) -> CellDisplay<'a> {
    let palette = view.palette;
    let page = view.page;
    if row >= page.row_count {
        let value = &view.mutator.inserted_rows()[row - page.row_count][column];
        return CellDisplay {
            text: sanitize_cell_text(value.as_display_str(), MAX_RENDERED_CHARS),
            is_null: value.is_null() || value.is_default(),
            background: palette.cell_inserted,
        };
    }
    if let Some(modified) = view.mutator.get_modified(row, column) {
        return CellDisplay {
            text: sanitize_cell_text(modified.as_display_str(), MAX_RENDERED_CHARS),
            is_null: modified.is_null() || modified.is_default(),
            background: palette.cell_modified,
        };
    }
    let background = if view.mutator.is_deleted(row) {
        palette.cell_deleted
    } else if row % 2 == 1 {
        palette.row_alt
    } else {
        Color32::TRANSPARENT
    };
    match page.cell_str(row, column) {
        None => CellDisplay {
            text: Cow::Borrowed("NULL"),
            is_null: true,
            background,
        },
        Some(text) => CellDisplay {
            text: sanitize_cell_text(text, MAX_RENDERED_CHARS),
            is_null: false,
            background,
        },
    }
}

pub(super) fn append_text(job: &mut LayoutJob, text: &str, font_id: FontId, color: Color32, leading_space: f32) {
    if text.is_empty() {
        return;
    }
    job.append(
        text,
        leading_space,
        TextFormat {
            font_id,
            color,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
}

pub(super) fn single_line_galley(ui: &Ui, mut job: LayoutJob, max_width: f32) -> Arc<Galley> {
    job.wrap = TextWrapping {
        max_width: max_width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.fonts(|fonts| fonts.layout_job(job))
}

pub(super) fn render_data_cell_with_fk(
    ui: &Ui,
    view: &ResultTableView,
    row: usize,
    column: usize,
    cell_rect: Rect,
    is_row_highlighted: bool,
    has_outbound_fk: bool,
) {
    let palette = view.palette;
    let display = cell_display(view, row, column);
    let is_focused = view.focused_cell == Some((row, column));
    let is_plain_background = display.background == Color32::TRANSPARENT || display.background == palette.row_alt;
    if !(is_row_highlighted && is_plain_background) && display.background != Color32::TRANSPARENT {
        ui.painter().rect_filled(cell_rect, 0.0, display.background);
    }

    let show_outbound_fk = has_outbound_fk && !display.is_null && row < view.page.row_count;
    let text_color = if display.is_null {
        palette.text_muted
    } else if show_outbound_fk {
        palette.accent
    } else {
        palette.text_primary
    };
    let mut job = LayoutJob::default();
    append_text(
        &mut job,
        &display.text,
        FontId::new(CELL_FONT_SIZE, FontFamily::Monospace),
        text_color,
        0.0,
    );
    let galley = single_line_galley(ui, job, cell_rect.width() - CELL_PADDING_X * 2.0);
    let is_right_aligned = view.page.columns[column].is_numeric && !display.is_null;
    let text_x = if is_right_aligned {
        cell_rect.right() - CELL_PADDING_X - galley.size().x
    } else {
        cell_rect.left() + CELL_PADDING_X
    };
    let text_y = cell_rect.center().y - galley.size().y / 2.0;
    let galley_w = galley.size().x;
    let galley_h = galley.size().y;
    ui.painter().galley(egui::pos2(text_x, text_y), galley, text_color);
    if show_outbound_fk {
        ui.painter().hline(
            text_x..=(text_x + galley_w),
            text_y + galley_h - 0.5,
            Stroke::new(1.0, palette.accent.gamma_multiply(0.45)),
        );
    }

    if is_focused {
        ui.painter().rect_stroke(
            cell_rect.shrink(0.5),
            0.0,
            Stroke::new(1.5, palette.accent),
            egui::StrokeKind::Inside,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_cell_text_borrows_clean_short_values_without_allocating() {
        let clean = sanitize_cell_text("ada@example.com", 120);
        assert!(matches!(clean, Cow::Borrowed("ada@example.com")));

        let multibyte = sanitize_cell_text("café résumé", 12);
        assert!(matches!(multibyte, Cow::Borrowed("café résumé")));
    }

    #[test]
    fn sanitize_cell_text_replaces_control_whitespace_and_truncates() {
        let multiline = sanitize_cell_text("line1\nline2\tval\rend", 120);
        assert!(matches!(multiline, Cow::Owned(_)));
        assert_eq!(multiline.as_ref(), "line1 line2 val end");

        let long = sanitize_cell_text("abcdefghij", 5);
        assert!(matches!(long, Cow::Owned(_)));
        assert_eq!(long.as_ref(), "abcde…");
    }
}
