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
