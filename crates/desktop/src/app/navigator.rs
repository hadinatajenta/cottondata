use super::activity::SlotActivity;
use super::query_tab::BottomDeckTab;
use super::CottonDataApp;
use crate::column_glyph::column_type_glyph;
use crate::theme::{Palette, RADIUS_CONTROL};
use crate::ui_kit::{busy_indicator, caption, ghost_button, icon, icon_button, icon_text, tree_toggle_icon};
use crate::worker::SlotCmd;
use cottondata_core::ServerSlotId;
use cottondata_introspect::{SchemaCatalog, SchemaEntry, TableEntry, TableKind};
use eframe::egui::{
    self, collapsing_header::CollapsingState, CornerRadius, FontFamily, FontId, RichText, Sense,
};

#[derive(Default)]
pub(super) struct NavigatorRequests {
    pub(super) load_schema: Option<String>,
    pub(super) set_active_schema: Option<String>,
    pub(super) open_table: Option<(String, String)>,
    pub(super) load_table_detail: Option<(String, String)>,
    pub(super) generate_select: Option<(String, String)>,
    pub(super) copy_text: Option<String>,
}

impl NavigatorRequests {
    fn is_empty(&self) -> bool {
        self.load_schema.is_none()
            && self.set_active_schema.is_none()
            && self.open_table.is_none()
            && self.load_table_detail.is_none()
            && self.generate_select.is_none()
            && self.copy_text.is_none()
    }
}

struct NavigatorView<'a> {
    palette: Palette,
    catalog: &'a SchemaCatalog,
    slot_index: usize,
    filter: &'a str,
    active_table: Option<(&'a str, &'a str)>,
    active_schema: Option<&'a str>,
    activity: &'a SlotActivity,
}

impl CottonDataApp {
    pub(super) fn render_slot_schemas(
        &mut self,
        ui: &mut egui::Ui,
        palette: Palette,
        slot_id: ServerSlotId,
        filter: &str,
    ) -> Option<NavigatorRequests> {
        let catalog = self.catalogs[slot_id.index()].as_ref()?;
        let active_tab = &self.tabs[self.active_tab_idx];
        let active_table = active_tab
            .bound_table
            .as_ref()
            .filter(|_| active_tab.slot == Some(slot_id))
            .map(|(schema, table)| (schema.as_str(), table.as_str()));
        let active_schema = self
            .registry
            .get(slot_id)
            .and_then(|s| s.runtime.as_ref())
            .and_then(|r| r.current_schema.as_deref());

        let view = NavigatorView {
            palette,
            catalog,
            slot_index: slot_id.index(),
            filter,
            active_table,
            active_schema,
            activity: &self.slot_activity[slot_id.index()],
        };
        let mut requests = NavigatorRequests::default();

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

        (!requests.is_empty()).then_some(requests)
    }

    pub(super) fn apply_navigator_requests(
        &mut self,
        ctx: &egui::Context,
        slot_id: ServerSlotId,
        requests: NavigatorRequests,
    ) {
        let activity = &mut self.slot_activity[slot_id.index()];
        if let Some(schema) = requests
            .load_schema
            .filter(|schema| !activity.loading_schemas.contains(schema))
        {
            activity.loading_schemas.insert(schema.clone());
            self.hub.send(slot_id, SlotCmd::LoadSchemaL1 { schema });
        }
        if let Some(schema) = requests.set_active_schema {
            self.active_slot = Some(slot_id);
            self.send_session_command(slot_id, SlotCmd::SetSchema(schema));
        }
        if let Some((schema, table)) = requests.open_table {
            self.active_slot = Some(slot_id);
            self.open_table_tab(slot_id, schema, table);
        }
        if let Some((schema, table)) = requests.load_table_detail {
            self.active_slot = Some(slot_id);
            let activity = &mut self.slot_activity[slot_id.index()];
            if !activity.is_loading_table_detail(&schema, &table) {
                activity.loading_table_detail = Some((schema.clone(), table.clone()));
                self.bottom_tab = BottomDeckTab::DdlInspector;
                self.hub.send(
                    slot_id,
                    SlotCmd::LoadTableL2 {
                        schema,
                        table,
                        show_in_inspector: true,
                    },
                );
            }
        }
        if let Some((schema, table)) = requests.generate_select {
            let dialect = self.slot_dialect(Some(slot_id));
            let fq = dialect.qualified_table(&schema, &table);
            let sql = format!("SELECT *\nFROM {fq}\nLIMIT 100;");
            self.open_console_tab_for_slot(Some(slot_id), Some(sql));
        }
        if let Some(text) = requests.copy_text {
            ctx.copy_text(text.clone());
            self.notify_success(format!("Copied `{text}`"));
        }
    }
}

pub(super) fn contains_ascii_ignore_case(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    let needle = needle_lower.as_bytes();
    let hay = haystack.as_bytes();
    if needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len())
        .any(|w| w.iter().zip(needle).all(|(&a, &b)| a.to_ascii_lowercase() == b))
}

fn contains_filter(candidate: &str, filter: &str) -> bool {
    contains_ascii_ignore_case(candidate, filter)
}

fn schema_matches_filter(view: &NavigatorView, schema: &SchemaEntry, schema_name: &str) -> bool {
    contains_filter(schema_name, view.filter)
        || schema
            .tables
            .iter()
            .any(|table| contains_filter(view.catalog.resolve(table.name), view.filter))
        || schema
            .routines
            .iter()
            .any(|(name, _)| contains_filter(view.catalog.resolve(*name), view.filter))
}

const MAX_FILTERED_TABLES_PER_SCHEMA: usize = 80;

fn render_schema_node(
    ui: &mut egui::Ui,
    view: &NavigatorView,
    schema: &SchemaEntry,
    schema_name: &str,
    requests: &mut NavigatorRequests,
) -> bool {
    let palette = view.palette;
    let is_active_schema = view
        .active_schema
        .is_some_and(|active| active.eq_ignore_ascii_case(schema_name));
    let schema_matches = contains_filter(schema_name, view.filter);
    let matching_table_count = if view.filter.is_empty() || schema_matches {
        schema.tables.len()
    } else {
        schema
            .tables
            .iter()
            .filter(|table| contains_filter(view.catalog.resolve(table.name), view.filter))
            .count()
    };

    let node_id = ui.make_persistent_id(("schema", view.slot_index, schema_name));
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), node_id, is_active_schema);
    if !view.filter.is_empty() && !schema_matches && matching_table_count > 0 {
        state.set_open(true);
    }

    let header = ui.horizontal(|ui| {
        state.show_toggle_button(ui, tree_toggle_icon);
        let name_color = if schema.is_system {
            palette.text_muted
        } else if is_active_schema {
            palette.accent
        } else {
            palette.text_primary
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if view.activity.loading_schemas.contains(schema_name) {
                busy_indicator(ui, palette, "");
            } else if schema.l1_loaded {
                let count_label = if !view.filter.is_empty() && matching_table_count != schema.tables.len() {
                    format!("{matching_table_count}/{}", schema.tables.len())
                } else {
                    schema.tables.len().to_string()
                };
                caption(ui, palette, count_label);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let label = egui::Label::new(icon_text(
                    icon::STACK,
                    if is_active_schema {
                        palette.accent
                    } else {
                        palette.icon_schema
                    },
                    schema_name,
                    name_color,
                    13.0,
                ))
                .truncate()
                .sense(Sense::click());
                let resp = ui.add(label).on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    state.toggle(ui);
                }
                resp.context_menu(|ui| {
                    ui.set_min_width(180.0);
                    if ui.button(format!("{}  Set as active schema", icon::CHECK_CIRCLE)).clicked() {
                        requests.set_active_schema = Some(schema_name.to_string());
                        ui.close_menu();
                    }
                    if ui.button(format!("{}  Refresh tables", icon::ARROW_CLOCKWISE)).clicked() {
                        requests.load_schema = Some(schema_name.to_string());
                        ui.close_menu();
                    }
                    if ui.button(format!("{}  Copy schema name", icon::COPY)).clicked() {
                        requests.copy_text = Some(schema_name.to_string());
                        ui.close_menu();
                    }
                });
            });
        });
    });

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
            let is_filtering = !view.filter.is_empty();
            let mut rendered = 0usize;
            let mut hidden_matches = 0usize;
            for table in &schema.tables {
                let table_name = view.catalog.resolve(table.name);
                if !schema_matches && !contains_filter(table_name, view.filter) {
                    continue;
                }
                if is_filtering && rendered >= MAX_FILTERED_TABLES_PER_SCHEMA {
                    hidden_matches += 1;
                    continue;
                }
                rendered += 1;
                render_table_node(ui, view, schema_name, table, table_name, requests);
            }
            if hidden_matches > 0 {
                caption(
                    ui,
                    palette,
                    format!("+{hidden_matches} more matches — type more to narrow"),
                );
            }
            if !schema.routines.is_empty() {
                let matching_routines: Vec<(&str, &str)> = schema
                    .routines
                    .iter()
                    .map(|(n, k)| (view.catalog.resolve(*n), view.catalog.resolve(*k)))
                    .filter(|(n, _)| schema_matches || contains_filter(n, view.filter))
                    .collect();
                if !matching_routines.is_empty() {
                    let routine_node_id = ui.make_persistent_id(("routines", view.slot_index, schema_name));
                    let mut routine_state = CollapsingState::load_with_default_open(ui.ctx(), routine_node_id, false);
                    let r_header = ui.horizontal(|ui| {
                        routine_state.show_toggle_button(ui, tree_toggle_icon);
                        ui.label(icon_text(
                            icon::CODE,
                            palette.icon_view,
                            &format!("Routines ({})", matching_routines.len()),
                            palette.text_muted,
                            12.0,
                        ));
                    });
                    routine_state.show_body_indented(&r_header.response, ui, |ui| {
                        for (r_name, r_kind) in matching_routines {
                            ui.horizontal(|ui| {
                                let label = egui::Label::new(icon_text(
                                    icon::CODE,
                                    palette.icon_view,
                                    r_name,
                                    palette.text_primary,
                                    12.5,
                                ))
                                .truncate()
                                .sense(Sense::click());
                                let resp = ui.add(label);
                                resp.context_menu(|ui| {
                                    if ui.button(format!("{}  Copy routine name", icon::COPY)).clicked() {
                                        requests.copy_text = Some(format!("{schema_name}.{r_name}"));
                                        ui.close_menu();
                                    }
                                });
                                caption(ui, palette, r_kind);
                            });
                        }
                    });
                }
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

    if !state.is_open() {
        let row_h = 22.0;
        let next_y = ui.cursor().top();
        let clip = ui.clip_rect();
        if next_y + row_h < clip.top() - 40.0 || next_y > clip.bottom() + 40.0 {
            ui.allocate_space(egui::vec2(1.0, row_h));
            return;
        }
    }

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
                        let resp = ui
                            .add(label)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Open table data");
                        if resp.clicked() {
                            requests.open_table = Some((schema_name.to_string(), table_name.to_string()));
                        }
                        resp.context_menu(|ui| {
                            ui.set_min_width(190.0);
                            if ui.button(format!("{}  Open table data", icon::TABLE)).clicked() {
                                requests.open_table = Some((schema_name.to_string(), table_name.to_string()));
                                ui.close_menu();
                            }
                            if ui.button(format!("{}  Inspect DDL & Indexes", icon::CODE)).clicked() {
                                requests.load_table_detail = Some((schema_name.to_string(), table_name.to_string()));
                                ui.close_menu();
                            }
                            if ui
                                .button(format!("{}  SELECT in new console", icon::TERMINAL_WINDOW))
                                .clicked()
                            {
                                requests.generate_select = Some((schema_name.to_string(), table_name.to_string()));
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button(format!("{}  Copy qualified name", icon::COPY)).clicked() {
                                requests.copy_text = Some(format!("{schema_name}.{table_name}"));
                                ui.close_menu();
                            }
                        });
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
