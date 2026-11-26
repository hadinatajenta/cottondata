mod cells;
mod context_menu;

use super::query_tab::{InlineCellEditor, InlineCommitMove};
use super::row_selection::{RowSelectMode, RowSelection};
use crate::column_glyph::column_type_glyph;
use crate::theme::Palette;
use crate::ui_kit::icon;
pub(super) use cells::{compute_column_widths, compute_single_column_width};
use cells::{append_text, modified_cell_tooltip, paint_cell, row_badge, single_line_galley};
use context_menu::render_cell_context_menu;
use cottondata_grid::{ColumnarPage, Editability, ExportFormat, SortDirection, StagedMutator};
use cottondata_introspect::{ForeignKeyInfo, InboundForeignKeyInfo, TableDetail};
use eframe::egui::{self, text::LayoutJob, Align2, FontId, Rect, Sense, Stroke, Ui};

const ROW_NUMBER_WIDTH: f32 = 56.0;
const HEADER_HEIGHT: f32 = 30.0;
const ROW_HEIGHT: f32 = 26.0;
const CELL_PADDING_X: f32 = 8.0;
const CELL_FONT_SIZE: f32 = 12.0;

pub(super) struct ResultTableView<'a> {
    pub(super) palette: Palette,
    pub(super) tab_id: u64,
    pub(super) page: &'a ColumnarPage,
    pub(super) mutator: &'a StagedMutator,
    pub(super) focused_cell: Option<(usize, usize)>,
    pub(super) row_selection: &'a RowSelection,
    pub(super) selected_row_count: usize,
    pub(super) column_widths: &'a [f32],
    pub(super) order_by: Option<&'a (String, SortDirection)>,
    pub(super) is_sortable: bool,
    pub(super) editability: &'a Editability,
    pub(super) table_detail: Option<&'a TableDetail>,
}

impl<'a> ResultTableView<'a> {
    pub(super) fn outbound_fk_for_column(&self, column: usize) -> Option<&'a ForeignKeyInfo> {
        let detail = self.table_detail?;
        let col_meta = self.page.columns.get(column)?;
        let col_name = col_meta.sql_column_name();
        detail
            .foreign_keys
            .iter()
            .find(|fk| fk.column_name.eq_ignore_ascii_case(col_name))
    }

    pub(super) fn inbound_fks_for_column(&self, column: usize) -> Vec<&'a InboundForeignKeyInfo> {
        let Some(detail) = self.table_detail else {
            return Vec::new();
        };
        let Some(col_meta) = self.page.columns.get(column) else {
            return Vec::new();
        };
        let col_name = col_meta.sql_column_name();
        detail
            .inbound_foreign_keys
            .iter()
            .filter(|fk| fk.target_column.eq_ignore_ascii_case(col_name))
            .collect()
    }
}

pub(super) enum TableAction {
    SelectCell {
        row: usize,
        column: usize,
        mode: RowSelectMode,
    },
    SelectRow {
        row: usize,
        mode: RowSelectMode,
    },
    StartInlineEdit {
        row: usize,
        column: usize,
    },
    CommitInlineEdit(InlineCommitMove),
    CancelInlineEdit,
    PromoteInlineToInspector,
    SetCellNull {
        row: usize,
        column: usize,
    },
    SetCellDefault {
        row: usize,
        column: usize,
    },
    RevertCell {
        row: usize,
        column: usize,
    },
    RevertRow {
        row: usize,
    },
    CloneRow {
        row: usize,
    },
    ToggleDeleteRow {
        row: usize,
    },
    OpenInspector {
        row: usize,
        column: usize,
    },
    OpenRecordView {
        row: usize,
    },
    CompareSelectedRows(usize, usize),
    QuickFilterValue {
        row: usize,
        column: usize,
        negate: bool,
    },
    NavigateFk {
        target_schema: String,
        target_table: String,
        target_column: String,
        row: usize,
        column: usize,
    },
    CopyColumnAsIn {
        row: usize,
        column: usize,
    },
    CopyValue {
        row: usize,
        column: usize,
    },
    CopyRows(ExportFormat),
    SelectAll,
}

#[derive(Default)]
pub(super) struct ResultTableInteraction {
    pub(super) action: Option<TableAction>,
    pub(super) clicked_header: Option<usize>,
    pub(super) resized_column: Option<(usize, f32)>,
    pub(super) autofit_column: Option<usize>,
}

pub(super) fn render_result_table(
    ui: &mut Ui,
    view: &ResultTableView,
    mut inline_editor: Option<&mut InlineCellEditor>,
) -> ResultTableInteraction {
    let mut interaction = ResultTableInteraction::default();
    let total_width = ROW_NUMBER_WIDTH + view.column_widths.iter().sum::<f32>();
    let total_rows = view.page.row_count + view.mutator.inserted_rows().len();
    let offset_memory_id = egui::Id::new(("result_table_offset_x", view.tab_id));
    let previous_offset_x = ui
        .ctx()
        .data(|memory| memory.get_temp::<f32>(offset_memory_id))
        .unwrap_or(0.0);

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        egui::ScrollArea::horizontal()
            .id_salt(("result_table_header", view.tab_id))
            .horizontal_scroll_offset(previous_offset_x)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .enable_scrolling(false)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                paint_header_row(ui, view, total_width, &mut interaction);
            });

        let body = egui::ScrollArea::both()
            .id_salt(("result_table_body", view.tab_id))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, total_rows, |ui, row_range| {
                for row in row_range {
                    if let Some(action) = paint_body_row(ui, view, row, total_width, inline_editor.as_deref_mut()) {
                        interaction.action = Some(action);
                    }
                }
            });

        let current_offset_x = body.state.offset.x;
        if (current_offset_x - previous_offset_x).abs() > f32::EPSILON {
            ui.ctx()
                .data_mut(|memory| memory.insert_temp(offset_memory_id, current_offset_x));
            ui.ctx().request_repaint();
        }
    });
    interaction
}

fn visible_columns<'a>(ui: &Ui, row_rect: Rect, view: &'a ResultTableView) -> impl Iterator<Item = (usize, Rect)> + 'a {
    let clip = ui.clip_rect();
    let mut column_left = row_rect.left() + ROW_NUMBER_WIDTH;
    view.column_widths
        .iter()
        .enumerate()
        .map(move |(column_index, width)| {
            let cell_rect = Rect::from_min_size(
                egui::pos2(column_left, row_rect.top()),
                egui::vec2(*width, row_rect.height()),
            );
            column_left += width;
            (column_index, cell_rect)
        })
        .filter(move |(_, cell_rect)| cell_rect.right() >= clip.left() && cell_rect.left() <= clip.right())
}

fn paint_header_row(ui: &mut Ui, view: &ResultTableView, total_width: f32, interaction: &mut ResultTableInteraction) {
    let palette = view.palette;
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(total_width, HEADER_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(row_rect, 0.0, palette.bg_surface);
    painter.text(
        egui::pos2(row_rect.left() + ROW_NUMBER_WIDTH / 2.0, row_rect.center().y),
        Align2::CENTER_CENTER,
        icon::HASH,
        FontId::proportional(CELL_FONT_SIZE),
        palette.text_muted,
    );

    let mut resizing_column = None;
    for (column_index, cell_rect) in visible_columns(ui, row_rect, view) {
        let column = &view.page.columns[column_index];
        let resize_rect = Rect::from_min_max(
            egui::pos2((cell_rect.right() - 4.0).max(cell_rect.left()), cell_rect.top()),
            egui::pos2(cell_rect.right() + 3.0, cell_rect.bottom()),
        );
        let resize_resp = ui.interact(
            resize_rect,
            ui.id().with(("header_resize", view.tab_id, column_index)),
            Sense::click_and_drag(),
        );
        let is_resizing = resize_resp.hovered() || resize_resp.dragged();
        if is_resizing {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            resizing_column = Some(cell_rect.right() - 0.5);
        }
        if resize_resp.double_clicked() {
            interaction.autofit_column = Some(column_index);
        } else if resize_resp.dragged() {
            let new_width = (cell_rect.width() + resize_resp.drag_delta().x).clamp(48.0, 720.0);
            interaction.resized_column = Some((column_index, new_width));
        }

        let click_rect = Rect::from_min_max(
            cell_rect.min,
            egui::pos2((cell_rect.right() - 4.0).max(cell_rect.left()), cell_rect.bottom()),
        );
        let response = ui.interact(click_rect, ui.id().with(("header", view.tab_id, column_index)), Sense::click());
        if response.hovered() && view.is_sortable && !is_resizing {
            ui.painter().rect_filled(cell_rect, 0.0, palette.bg_hover);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let (glyph, glyph_color) = if column.is_primary_key {
            (icon::KEY, palette.warning)
        } else {
            (column_type_glyph(&column.type_name), palette.text_muted)
        };
        let sort_glyph = match view.order_by {
            Some((name, SortDirection::Asc)) if name == &column.name => icon::SORT_ASCENDING,
            Some((name, SortDirection::Desc)) if name == &column.name => icon::SORT_DESCENDING,
            _ => "",
        };
        let mut job = LayoutJob::default();
        let header_font = FontId::proportional(CELL_FONT_SIZE);
        append_text(&mut job, glyph, header_font.clone(), glyph_color, 0.0);
        append_text(&mut job, &column.name, header_font.clone(), palette.text_primary, 6.0);
        append_text(&mut job, sort_glyph, header_font, palette.accent, 6.0);
        let galley = single_line_galley(ui, job, cell_rect.width() - CELL_PADDING_X * 2.0);
        let text_pos = egui::pos2(
            cell_rect.left() + CELL_PADDING_X,
            cell_rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(text_pos, galley, palette.text_primary);
        if response.clicked() {
            interaction.clicked_header = Some(column_index);
        }
        response.on_hover_text(format!("{} · Drag right edge to resize", column.type_name));
    }

    paint_grid_lines(ui, view, row_rect);
    if let Some(x) = resizing_column {
        ui.painter().vline(x, row_rect.y_range(), Stroke::new(2.0, palette.accent));
    }
    ui.painter().hline(
        row_rect.x_range(),
        row_rect.bottom() - 0.5,
        Stroke::new(1.0, palette.border),
    );
}

fn paint_body_row(
    ui: &mut Ui,
    view: &ResultTableView,
    row: usize,
    total_width: f32,
    mut inline_editor: Option<&mut InlineCellEditor>,
) -> Option<TableAction> {
    let palette = view.palette;
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(total_width, ROW_HEIGHT), Sense::hover());
    let is_row_selected = view.row_selection.contains(row);
    let is_row_hovered = ui.rect_contains_pointer(row_rect);
    let row_highlight = if is_row_selected {
        Some(palette.accent_soft)
    } else {
        is_row_hovered.then_some(palette.bg_hover)
    };
    if let Some(highlight) = row_highlight {
        ui.painter().rect_filled(row_rect, 0.0, highlight);
    }

    let mut action = None;
    let number_rect = Rect::from_min_size(row_rect.min, egui::vec2(ROW_NUMBER_WIDTH, ROW_HEIGHT));
    let number_response = ui.interact(number_rect, ui.id().with(("row_number", row)), Sense::click());
    if number_response.clicked() {
        let mode = RowSelectMode::from_modifiers(ui.input(|input| input.modifiers));
        action = Some(TableAction::SelectRow { row, mode });
    }
    number_response.context_menu(|ui| {
        if let Some(menu_action) = render_cell_context_menu(ui, view, row, 0) {
            action = Some(menu_action);
        }
    });
    ui.painter().text(
        egui::pos2(row_rect.left() + ROW_NUMBER_WIDTH - CELL_PADDING_X, row_rect.center().y),
        Align2::RIGHT_CENTER,
        row_badge(view, row),
        FontId::monospace(11.0),
        if is_row_selected {
            palette.accent
        } else {
            palette.text_muted
        },
    );

    for (column_index, cell_rect) in visible_columns(ui, row_rect, view) {
        let is_editing_here = inline_editor
            .as_deref()
            .is_some_and(|editor| editor.row == row && editor.column == column_index);
        if is_editing_here {
            if let Some(editor) = inline_editor.as_deref_mut() {
                if let Some(edit_action) = render_inline_cell_editor(ui, view, cell_rect, editor) {
                    action = Some(edit_action);
                }
            }
            continue;
        }
        paint_cell(ui, view, row, column_index, cell_rect, row_highlight.is_some());
        let response = ui.interact(cell_rect, ui.id().with(("cell", row, column_index)), Sense::click());
        if let Some(cell_action) = cell_response_action(ui, view, &response, row, column_index) {
            action = Some(cell_action);
        }
        if let Some(tooltip) = modified_cell_tooltip(view, row, column_index) {
            response.clone().on_hover_text(tooltip);
        } else if row < view.page.row_count && view.page.cell_str(row, column_index).is_some() {
            if let Some(fk) = view.outbound_fk_for_column(column_index) {
                response
                    .clone()
                    .on_hover_text(format!("FK → {}.{} · Cmd+Click to open", fk.ref_table, fk.ref_column));
            }
        }
        response.context_menu(|ui| {
            if let Some(menu_action) = render_cell_context_menu(ui, view, row, column_index) {
                action = Some(menu_action);
            }
        });
    }

    paint_grid_lines(ui, view, row_rect);
    ui.painter().hline(
        row_rect.x_range(),
        row_rect.bottom() - 0.5,
        Stroke::new(1.0, palette.divider),
    );
    action
}

fn render_inline_cell_editor(
    ui: &mut Ui,
    view: &ResultTableView,
    cell_rect: Rect,
    editor: &mut InlineCellEditor,
) -> Option<TableAction> {
    let palette = view.palette;
    ui.painter().rect_filled(cell_rect, 0.0, palette.bg_card);
    ui.painter().rect_stroke(
        cell_rect.shrink(0.5),
        0.0,
        Stroke::new(1.5, palette.accent),
        egui::StrokeKind::Inside,
    );

    let was_newly_focused = editor.needs_focus;
    let inner_rect = cell_rect.shrink2(egui::vec2(5.0, 2.0));
    let mut output = ui
        .allocate_new_ui(egui::UiBuilder::new().max_rect(inner_rect), |ui| {
            egui::TextEdit::singleline(&mut editor.buffer)
                .id(egui::Id::new(("inline_cell_editor", view.tab_id, editor.row, editor.column)))
                .font(FontId::monospace(CELL_FONT_SIZE))
                .frame(false)
                .desired_width(f32::INFINITY)
                .show(ui)
        })
        .inner;

    if editor.needs_focus {
        output.response.request_focus();
        if editor.select_all {
            let char_len = editor.buffer.chars().count();
            output.state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(char_len),
            )));
            output.state.store(ui.ctx(), output.response.id);
        }
        editor.needs_focus = false;
        return None;
    }

    if was_newly_focused {
        return None;
    }

    let (esc, shift_enter, enter, tab, shift_tab) = ui.ctx().input_mut(|input| {
        let esc = input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
        let shift_enter = input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter);
        let enter = input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
        let shift_tab = input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab);
        let tab = input.consume_key(egui::Modifiers::NONE, egui::Key::Tab);
        (esc, shift_enter, enter, tab, shift_tab)
    });

    if esc {
        return Some(TableAction::CancelInlineEdit);
    }
    if shift_enter {
        return Some(TableAction::PromoteInlineToInspector);
    }
    if enter {
        return Some(TableAction::CommitInlineEdit(InlineCommitMove::Down));
    }
    if shift_tab {
        return Some(TableAction::CommitInlineEdit(InlineCommitMove::Left));
    }
    if tab {
        return Some(TableAction::CommitInlineEdit(InlineCommitMove::Right));
    }
    if output.response.lost_focus() {
        return Some(TableAction::CommitInlineEdit(InlineCommitMove::Stay));
    }
    None
}

fn cell_response_action(
    ui: &Ui,
    view: &ResultTableView,
    response: &egui::Response,
    row: usize,
    column: usize,
) -> Option<TableAction> {
    if response.double_clicked() {
        if view.editability.is_column_editable(column) {
            return Some(TableAction::StartInlineEdit { row, column });
        }
        return Some(TableAction::OpenInspector { row, column });
    }
    if response.clicked() {
        let modifiers = ui.input(|input| input.modifiers);
        if modifiers.command && !modifiers.shift && row < view.page.row_count && view.page.cell_str(row, column).is_some()
        {
            if let Some(fk) = view.outbound_fk_for_column(column) {
                return Some(TableAction::NavigateFk {
                    target_schema: fk.ref_schema.clone(),
                    target_table: fk.ref_table.clone(),
                    target_column: fk.ref_column.clone(),
                    row,
                    column,
                });
            }
        }
        let mode = RowSelectMode::from_modifiers(modifiers);
        return Some(TableAction::SelectCell { row, column, mode });
    }
    if response.secondary_clicked() && !view.row_selection.contains(row) {
        return Some(TableAction::SelectCell {
            row,
            column,
            mode: RowSelectMode::Replace,
        });
    }
    None
}

fn paint_grid_lines(ui: &Ui, view: &ResultTableView, row_rect: Rect) {
    let stroke = Stroke::new(1.0, view.palette.divider);
    ui.painter()
        .vline(row_rect.left() + ROW_NUMBER_WIDTH - 0.5, row_rect.y_range(), stroke);
    for (_, cell_rect) in visible_columns(ui, row_rect, view) {
        ui.painter().vline(cell_rect.right() - 0.5, row_rect.y_range(), stroke);
    }
}
