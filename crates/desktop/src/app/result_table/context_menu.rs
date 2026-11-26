use super::{ResultTableView, TableAction};
use crate::ui_kit::{icon, icon_text};
use cottondata_grid::ExportFormat;
use eframe::egui::{self, Ui};

const ROW_COPY_FORMATS: [(ExportFormat, &str); 5] = [
    (ExportFormat::Tsv, "⌘C"),
    (ExportFormat::Csv, ""),
    (ExportFormat::JsonArray, ""),
    (ExportFormat::JsonLines, ""),
    (ExportFormat::SqlInserts, ""),
];

pub(super) fn render_cell_context_menu(
    ui: &mut Ui,
    view: &ResultTableView,
    row: usize,
    column: usize,
) -> Option<TableAction> {
    let palette = view.palette;
    let mut action = None;
    let target_row_count = if view.row_selection.contains(row) {
        view.selected_row_count.max(1)
    } else {
        1
    };
    let row_noun = if target_row_count == 1 { "row" } else { "rows" };
    ui.set_min_width(220.0);

    let is_column_editable = view.editability.is_column_editable(column);
    let is_existing_row = row < view.page.row_count;
    let is_cell_modified = is_existing_row && view.mutator.get_modified(row, column).is_some();
    let is_row_modified =
        is_existing_row && (view.mutator.has_row_modifications(row) || view.mutator.is_deleted(row));
    let is_nullable = view
        .page
        .columns
        .get(column)
        .map(|meta| meta.is_nullable)
        .unwrap_or(true);

    if view.editability.is_editable() {
        if is_column_editable
            && menu_item(
                ui,
                icon_text(icon::PENCIL_SIMPLE, palette.accent, "Edit cell", palette.text_primary, 13.0),
                "↵",
            )
        {
            action = Some(TableAction::StartInlineEdit { row, column });
        }
        if is_column_editable
            && is_nullable
            && menu_item(
                ui,
                icon_text(icon::X_CIRCLE, palette.text_muted, "Set NULL", palette.text_primary, 13.0),
                "⌥⌘N",
            )
        {
            action = Some(TableAction::SetCellNull { row, column });
        }
        if is_column_editable
            && menu_item(
                ui,
                icon_text(
                    icon::ARROW_COUNTER_CLOCKWISE,
                    palette.text_muted,
                    "Set DEFAULT",
                    palette.text_primary,
                    13.0,
                ),
                "",
            )
        {
            action = Some(TableAction::SetCellDefault { row, column });
        }
        if is_cell_modified
            && menu_item(
                ui,
                icon_text(
                    icon::ARROW_U_UP_LEFT,
                    palette.warning,
                    "Revert cell",
                    palette.text_primary,
                    13.0,
                ),
                "⌘Z",
            )
        {
            action = Some(TableAction::RevertCell { row, column });
        }
        if is_row_modified
            && menu_item(
                ui,
                icon_text(
                    icon::ARROW_U_UP_LEFT,
                    palette.warning,
                    "Revert row",
                    palette.text_primary,
                    13.0,
                ),
                "",
            )
        {
            action = Some(TableAction::RevertRow { row });
        }
        if is_existing_row {
            if menu_item(
                ui,
                icon_text(icon::COPY, palette.text_muted, "Clone row", palette.text_primary, 13.0),
                "⌘D",
            ) {
                action = Some(TableAction::CloneRow { row });
            }
            let delete_label = if view.mutator.is_deleted(row) {
                "Undo delete row"
            } else {
                "Delete row"
            };
            if menu_item(
                ui,
                icon_text(icon::TRASH, palette.danger, delete_label, palette.danger, 13.0),
                "",
            ) {
                action = Some(TableAction::ToggleDeleteRow { row });
            }
        }
        ui.separator();
    }

    if menu_item(
        ui,
        icon_text(icon::COPY, palette.accent, "Copy value", palette.text_primary, 13.0),
        "",
    ) {
        action = Some(TableAction::CopyValue { row, column });
    }
    ui.separator();
    for (format, shortcut) in ROW_COPY_FORMATS {
        let label = format!("Copy {target_row_count} {row_noun} as {}", format.short_label());
        if menu_item(
            ui,
            icon_text(icon::ROWS, palette.text_muted, &label, palette.text_primary, 13.0),
            shortcut,
        ) {
            action = Some(TableAction::CopyRows(format));
        }
    }
    ui.separator();
    if menu_item(
        ui,
        icon_text(
            icon::SELECTION,
            palette.text_muted,
            "Select all rows",
            palette.text_primary,
            13.0,
        ),
        "⌘A",
    ) {
        action = Some(TableAction::SelectAll);
    }
    if menu_item(
        ui,
        icon_text(
            icon::PENCIL_SIMPLE,
            palette.text_muted,
            "Inspect cell",
            palette.text_primary,
            13.0,
        ),
        "",
    ) {
        action = Some(TableAction::OpenInspector { row, column });
    }

    if action.is_some() {
        ui.close_menu();
    }
    action
}

fn menu_item(ui: &mut Ui, text: egui::text::LayoutJob, shortcut: &str) -> bool {
    ui.add(egui::Button::new(text).shortcut_text(shortcut).frame(false))
        .clicked()
}
