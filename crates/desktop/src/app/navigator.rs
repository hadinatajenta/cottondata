use super::activity::SlotActivity;
use super::query_tab::BottomDeckTab;
use super::CottonDataApp;
use crate::column_glyph::column_type_glyph;
use crate::theme::{Palette, RADIUS_CONTROL};
use crate::ui_kit::{
    busy_indicator, caption, ghost_button, icon, icon_button, icon_text, inset_frame, section_title, tree_toggle_icon,
};
use crate::worker::SlotCmd;
use cottondata_introspect::{SchemaCatalog, SchemaEntry, TableEntry, TableKind};
use eframe::egui::{
    self, collapsing_header::CollapsingState, CornerRadius, FontFamily, FontId, Margin, RichText, Sense,
};

#[derive(Default)]
struct NavigatorRequests {
    load_schema: Option<String>,
    open_table: Option<(String, String)>,
    load_table_detail: Option<(String, String)>,
}

struct NavigatorView<'a> {
    palette: Palette,
    catalog: &'a SchemaCatalog,
    slot_index: usize,
    filter: &'a str,
    active_table: Option<(&'a str, &'a str)>,
    activity: &'a SlotActivity,
}

impl CottonDataApp {
    pub(super) fn render_navigator_card(&mut self, ui: &mut egui::Ui, palette: Palette) {
        ui.horizontal(|ui| {
            section_title(ui, palette, icon::TREE_STRUCTURE, "Schema Navigator");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .toggle_value(&mut self.show_system_schemas, RichText::new("System").size(11.5))
                    .on_hover_text("Show MySQL system schemas")
                    .changed()
                {
                    self.mark_session_dirty();
                }
            });
        });
        ui.add_space(4.0);
        inset_frame(palette)
            .inner_margin(Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(palette.text_muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.explorer_filter)
                            .hint_text("Filter schemas or tables")
                            .frame(false)
                            .desired_width(f32::INFINITY),
                    );
                });
            });
        ui.add_space(6.0);

        let Some(slot_id) = self.active_slot else {
            caption(ui, palette, "Select a server to browse its schemas.");
            return;
        };
        let Some(catalog) = &self.catalogs[slot_id.index()] else {
            caption(ui, palette, "Connect to the server to browse its schemas.");
            return;
        };

        let filter = self.explorer_filter.trim().to_ascii_lowercase();
        let active_tab = &self.tabs[self.active_tab_idx];
        let active_table = active_tab
            .bound_table
            .as_ref()
            .filter(|_| active_tab.slot == Some(slot_id))
            .map(|(schema, table)| (schema.as_str(), table.as_str()));
        let view = NavigatorView {
            palette,
            catalog,
            slot_index: slot_id.index(),
            filter: &filter,
            active_table,
            activity: &self.slot_activity[slot_id.index()],
        };
        let mut requests = NavigatorRequests::default();

        egui::ScrollArea::vertical()
            .id_salt("schema_navigator_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for schema in catalog.schemas() {
                    if schema.is_system && !self.show_system_schemas {
                        continue;
                    }
                    let schema_name = catalog.resolve(schema.name).to_string();
                    if !schema_matches_filter(&view, schema, &schema_name) {
                        continue;
                    }
                    let is_open = render_schema_node(ui, &view, schema, &schema_name, &mut requests);
                    let expansion_key = (view.slot_index, schema_name.clone());
                    let was_open = self.expanded_schemas.get(&expansion_key).copied().unwrap_or(false);
                    if is_open && !was_open && !schema.l1_loaded {
                        requests.load_schema = Some(schema_name);
                    }
                    self.expanded_schemas.insert(expansion_key, is_open);
                }
            });

        let activity = &mut self.slot_activity[slot_id.index()];
        if let Some(schema) = requests
            .load_schema
            .filter(|schema| !activity.loading_schemas.contains(schema))
        {
            activity.loading_schemas.insert(schema.clone());
            self.hub.send(slot_id, SlotCmd::LoadSchemaL1 { schema });
        }
        if let Some((schema, table)) = requests.open_table {
            self.open_table_tab(slot_id, schema, table);
        }
        if let Some((schema, table)) = requests.load_table_detail {
            let activity = &mut self.slot_activity[slot_id.index()];
            if !activity.is_loading_table_detail(&schema, &table) {
                activity.loading_table_detail = Some((schema.clone(), table.clone()));
                self.bottom_tab = BottomDeckTab::DdlInspector;
                self.hub.send(slot_id, SlotCmd::LoadTableL2 { schema, table });
            }
        }
    }
}

fn contains_filter(candidate: &str, filter: &str) -> bool {
    filter.is_empty() || candidate.to_ascii_lowercase().contains(filter)
}

fn schema_matches_filter(view: &NavigatorView, schema: &SchemaEntry, schema_name: &str) -> bool {
    contains_filter(schema_name, view.filter)
        || schema
            .tables
            .iter()
            .any(|table| contains_filter(view.catalog.resolve(table.name), view.filter))
}

fn render_schema_node(
    ui: &mut egui::Ui,
    view: &NavigatorView,
    schema: &SchemaEntry,
    schema_name: &str,
    requests: &mut NavigatorRequests,
) -> bool {
    let palette = view.palette;
    let node_id = ui.make_persistent_id(("schema", view.slot_index, schema_name));
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), node_id, false);
    let header = ui.horizontal(|ui| {
        state.show_toggle_button(ui, tree_toggle_icon);
        let name_color = if schema.is_system {
            palette.text_muted
        } else {
            palette.text_primary
        };
        let label = egui::Label::new(icon_text(
            icon::DATABASE,
            palette.icon_schema,
            schema_name,
            name_color,
            13.0,
        ))
        .truncate()
        .sense(Sense::click());
        if ui.add(label).clicked() {
            state.toggle(ui);
        }
        if view.activity.loading_schemas.contains(schema_name) {
            busy_indicator(ui, palette, "");
        } else if schema.l1_loaded {
            caption(ui, palette, schema.tables.len().to_string());
        }
    });

    let schema_matches = contains_filter(schema_name, view.filter);
    state
        .show_body_indented(&header.response, ui, |ui| {
            if !schema.l1_loaded {
                if view.activity.loading_schemas.contains(schema_name) {
                    ui.horizontal(|ui| busy_indicator(ui, palette, "Loading tables…"));
                } else if ghost_button(ui, palette, icon::ARROW_CLOCKWISE, "Load tables", true).clicked() {
                    requests.load_schema = Some(schema_name.to_string());
                }
                return;
            }
            for table in &schema.tables {
                let table_name = view.catalog.resolve(table.name);
                if !schema_matches && !contains_filter(table_name, view.filter) {
                    continue;
                }
                render_table_node(ui, view, schema_name, table, table_name, requests);
            }
        })
        .is_some()
}

fn render_table_node(
    ui: &mut egui::Ui,
    view: &NavigatorView,
    schema_name: &str,
    table: &TableEntry,
    table_name: &str,
    requests: &mut NavigatorRequests,
) {
    let palette = view.palette;
    let (glyph, glyph_color) = match table.kind {
        TableKind::BaseTable => (icon::TABLE, palette.icon_table),
        TableKind::View => (icon::EYE, palette.icon_view),
    };
    let is_active = view.active_table == Some((schema_name, table_name));
    let node_id = ui.make_persistent_id(("table", view.slot_index, schema_name, table_name));
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), node_id, false);

    let header = egui::Frame::new()
        .fill(if is_active {
            palette.accent_soft
        } else {
            egui::Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if table.columns.is_empty() {
                    ui.add_space(ui.spacing().indent);
                } else {
                    state.show_toggle_button(ui, tree_toggle_icon);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if view.activity.is_loading_table_detail(schema_name, table_name) {
                        busy_indicator(ui, palette, "");
                    } else if icon_button(ui, palette, icon::CODE, "Inspect DDL and indexes").clicked() {
                        requests.load_table_detail = Some((schema_name.to_string(), table_name.to_string()));
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let label =
                            egui::Label::new(icon_text(glyph, glyph_color, table_name, palette.text_primary, 13.0))
                                .truncate()
                                .sense(Sense::click());
                        if ui
                            .add(label)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Open table data")
                            .clicked()
                        {
                            requests.open_table = Some((schema_name.to_string(), table_name.to_string()));
                        }
                    });
                });
            });
        });

    state.show_body_indented(&header.response, ui, |ui| {
        for column in &table.columns {
            let column_type = view.catalog.resolve(column.column_type);
            let (column_glyph, column_glyph_color) = if column.is_primary_key {
                (icon::KEY, palette.warning)
            } else {
                (column_type_glyph(column_type), palette.text_muted)
            };
            ui.horizontal(|ui| {
                ui.label(icon_text(
                    column_glyph,
                    column_glyph_color,
                    view.catalog.resolve(column.name),
                    palette.text_primary,
                    12.5,
                ));
                ui.label(
                    RichText::new(column_type)
                        .font(FontId::new(11.0, FontFamily::Monospace))
                        .color(palette.text_muted),
                );
            });
        }
    });
}
