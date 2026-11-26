use super::CottonDataApp;
use crate::theme::{Palette, RADIUS_CONTROL};
use crate::ui_kit::{
    busy_indicator, caption, icon, icon_button, modal_shell, secondary_button, styled_text_input, ModalConfig,
};
use cottondata_core::ServerSlotId;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(super) struct ErTableCard {
    pub(super) name: String,
    pub(super) columns: Vec<(String, String, bool)>,
    pub(super) pos: Vec2,
}

#[derive(Clone)]
pub(super) struct ErRelation {
    pub(super) constraint_name: String,
    pub(super) source_table: String,
    pub(super) source_column: String,
    pub(super) target_table: String,
    pub(super) target_column: String,
}

pub(super) struct ErDiagramState {
    pub(super) slot: ServerSlotId,
    pub(super) schema: String,
    pub(super) tables: Vec<ErTableCard>,
    pub(super) relations: Vec<ErRelation>,
    pub(super) is_loading_fks: bool,
    pub(super) filter: String,
    pub(super) only_related: bool,
    pub(super) selected_table: Option<String>,
    pub(super) pan: Vec2,
    pub(super) zoom: f32,
}

impl ErDiagramState {
    pub(super) fn new(slot: ServerSlotId, schema: String, mut tables: Vec<ErTableCard>) -> Self {
        layout_er_cards(&mut tables, &[]);
        Self {
            slot,
            schema,
            tables,
            relations: Vec::new(),
            is_loading_fks: true,
            filter: String::new(),
            only_related: false,
            selected_table: None,
            pan: Vec2::new(28.0, 28.0),
            zoom: 1.0,
        }
    }

    pub(super) fn apply_relations(&mut self, relations: Vec<ErRelation>) {
        self.relations = relations;
        self.is_loading_fks = false;
        if self.tables.len() > 60 && !self.relations.is_empty() {
            self.only_related = true;
        }
        layout_er_cards(&mut self.tables, &self.relations);
    }
}

const CARD_W: f32 = 220.0;
const CARD_HEADER_H: f32 = 28.0;
const CARD_ROW_H: f32 = 19.0;
const MAX_COLS_SHOWN: usize = 10;

fn card_height(card: &ErTableCard) -> f32 {
    let shown = card.columns.len().min(MAX_COLS_SHOWN);
    let extra = usize::from(card.columns.len() > MAX_COLS_SHOWN);
    CARD_HEADER_H + ((shown + extra).max(1) as f32) * CARD_ROW_H + 10.0
}

fn layout_er_cards(tables: &mut [ErTableCard], relations: &[ErRelation]) {
    let mut degree: HashMap<String, usize> = HashMap::new();
    for rel in relations {
        *degree.entry(rel.source_table.to_ascii_lowercase()).or_insert(0) += 1;
        *degree.entry(rel.target_table.to_ascii_lowercase()).or_insert(0) += 1;
    }
    tables.sort_by(|a, b| {
        let da = degree.get(&a.name.to_ascii_lowercase()).copied().unwrap_or(0);
        let db = degree.get(&b.name.to_ascii_lowercase()).copied().unwrap_or(0);
        db.cmp(&da).then_with(|| a.name.cmp(&b.name))
    });

    let cols = ((tables.len() as f32).sqrt().ceil() as usize).clamp(1, 6);
    let gap_x = 56.0;
    let gap_y = 40.0;
    let mut col_y = vec![0.0_f32; cols];
    for (idx, card) in tables.iter_mut().enumerate() {
        let col = idx % cols;
        card.pos = Vec2::new(col as f32 * (CARD_W + gap_x), col_y[col]);
        col_y[col] += card_height(card) + gap_y;
    }
}

impl CottonDataApp {
    pub(super) fn open_er_diagram(&mut self, slot: ServerSlotId, schema: String) {
        let tables: Vec<ErTableCard> = self.catalogs[slot.index()]
            .as_ref()
            .and_then(|cat| {
                cat.schemas()
                    .iter()
                    .find(|s| cat.resolve(s.name).eq_ignore_ascii_case(&schema))
                    .map(|s| {
                        s.tables
                            .iter()
                            .map(|t| ErTableCard {
                                name: cat.resolve(t.name).to_string(),
                                columns: t
                                    .columns
                                    .iter()
                                    .map(|c| {
                                        (
                                            cat.resolve(c.name).to_string(),
                                            cat.resolve(c.column_type).to_string(),
                                            c.is_primary_key,
                                        )
                                    })
                                    .collect(),
                                pos: Vec2::ZERO,
                            })
                            .collect()
                    })
            })
            .unwrap_or_default();

        self.er_diagram = Some(ErDiagramState::new(slot, schema.clone(), tables));
        if let Some(s) = self.registry.get(slot) {
            let profile = s.profile.clone();
            let pw = self.storage.get_password(&profile.id);
            self.hub.load_schema_er_async(slot, profile, pw, schema);
        }
    }

    pub(super) fn render_er_diagram_modal(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(mut state) = self.er_diagram.take() else {
            return;
        };
        let mut keep_open = true;
        let mut open_table_req: Option<(ServerSlotId, String, String)> = None;
        let server_label = self.slot_display_name(state.slot);
        let title = format!("ER Diagram · {}", state.schema);
        let subtitle = format!(
            "{server_label} · {} tables · {} foreign keys · Drag canvas to pan, double-click table to open",
            state.tables.len(),
            state.relations.len()
        );
        let modal_w = (ctx.screen_rect().width() - 80.0).clamp(720.0, 1180.0);
        let canvas_h = (ctx.screen_rect().height() - 250.0).clamp(360.0, 620.0);

        let mut zoom_delta = 0.0_f32;
        let mut reset_view = false;
        let table_count = state.tables.len();
        let relation_count = state.relations.len();
        let selected_rel_summary: Option<String> = state.selected_table.as_ref().map(|sel| {
            let matching: Vec<String> = state
                .relations
                .iter()
                .filter(|r| r.source_table.eq_ignore_ascii_case(sel) || r.target_table.eq_ignore_ascii_case(sel))
                .take(3)
                .map(|r| {
                    format!(
                        "{}.{} → {}.{} ({})",
                        r.source_table, r.source_column, r.target_table, r.target_column, r.constraint_name
                    )
                })
                .collect();
            if matching.is_empty() {
                format!("{sel}: no foreign key relations in this schema")
            } else {
                matching.join("  ·  ")
            }
        });

        let outcome = modal_shell(
            ctx,
            p,
            ModalConfig {
                id: "er_diagram_modal",
                icon_glyph: icon::TREE_STRUCTURE,
                icon_color: p.accent,
                title: &title,
                subtitle: Some(&subtitle),
                accent_bar: Some(p.accent),
                width: modal_w,
                max_height: None,
                dismiss_on_backdrop: true,
            },
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    styled_text_input(
                        ui,
                        p,
                        "er_filter_input",
                        &mut state.filter,
                        "Filter tables…",
                        200.0,
                    );
                    ui.checkbox(&mut state.only_related, "Only tables with foreign keys");
                    if state.is_loading_fks {
                        busy_indicator(ui, p, "Loading foreign keys…");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if secondary_button(ui, p, icon::ARROW_COUNTER_CLOCKWISE, "Reset", true).clicked() {
                            reset_view = true;
                        }
                        if icon_button(ui, p, icon::PLUS, "Zoom in").clicked() {
                            zoom_delta = 0.15;
                        }
                        caption(ui, p, format!("{}%", (state.zoom * 100.0).round() as i32));
                        if icon_button(ui, p, icon::MINUS, "Zoom out").clicked() {
                            zoom_delta = -0.15;
                        }
                    });
                });
                ui.add_space(8.0);

                let needle = state.filter.trim().to_ascii_lowercase();
                let related_names: HashSet<String> = state
                    .relations
                    .iter()
                    .flat_map(|r| [r.source_table.to_ascii_lowercase(), r.target_table.to_ascii_lowercase()])
                    .collect();
                let visible_indices: Vec<usize> = state
                    .tables
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| {
                        let lower = t.name.to_ascii_lowercase();
                        if state.only_related && !related_names.contains(&lower) {
                            return false;
                        }
                        needle.is_empty() || lower.contains(&needle)
                    })
                    .take(80)
                    .map(|(i, _)| i)
                    .collect();

                let (canvas_rect, canvas_resp) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), canvas_h), Sense::click_and_drag());
                let painter = ui.painter_at(canvas_rect);
                painter.rect(
                    canvas_rect,
                    CornerRadius::same(RADIUS_CONTROL),
                    p.bg_surface,
                    Stroke::new(1.0, p.border),
                    egui::StrokeKind::Inside,
                );

                if canvas_resp.dragged() {
                    state.pan += canvas_resp.drag_delta();
                }
                if canvas_resp.hovered() {
                    let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll_y.abs() > 0.1 {
                        state.zoom = (state.zoom * (1.0 + scroll_y * 0.0015)).clamp(0.35, 1.8);
                    }
                }

                let grid_step = (28.0 * state.zoom).max(12.0);
                let mut gx = canvas_rect.left() + state.pan.x.rem_euclid(grid_step);
                while gx < canvas_rect.right() {
                    let mut gy = canvas_rect.top() + state.pan.y.rem_euclid(grid_step);
                    while gy < canvas_rect.bottom() {
                        painter.circle_filled(Pos2::new(gx, gy), 1.0, p.dot_grid);
                        gy += grid_step;
                    }
                    gx += grid_step;
                }

                let mut screen_rects: HashMap<String, Rect> = HashMap::new();
                let fk_cols_by_table: HashSet<(String, String)> = state
                    .relations
                    .iter()
                    .map(|r| (r.source_table.to_ascii_lowercase(), r.source_column.to_ascii_lowercase()))
                    .collect();

                for &idx in &visible_indices {
                    let card = &state.tables[idx];
                    let min = canvas_rect.min + state.pan + card.pos * state.zoom;
                    let size = Vec2::new(CARD_W * state.zoom, card_height(card) * state.zoom);
                    screen_rects.insert(card.name.to_ascii_lowercase(), Rect::from_min_size(min, size));
                }

                for rel in &state.relations {
                    let src_key = rel.source_table.to_ascii_lowercase();
                    let dst_key = rel.target_table.to_ascii_lowercase();
                    let (Some(&src_rect), Some(&dst_rect)) = (screen_rects.get(&src_key), screen_rects.get(&dst_key))
                    else {
                        continue;
                    };
                    let is_highlighted = state
                        .selected_table
                        .as_deref()
                        .is_some_and(|sel| sel.eq_ignore_ascii_case(&rel.source_table) || sel.eq_ignore_ascii_case(&rel.target_table));
                    let edge_color = if is_highlighted {
                        p.accent
                    } else {
                        p.text_muted.gamma_multiply(0.55)
                    };
                    let stroke_w = if is_highlighted { 2.0 } else { 1.2 };

                    let (p0, p3) = if src_rect.center().x <= dst_rect.center().x {
                        (
                            Pos2::new(src_rect.right(), src_rect.center().y),
                            Pos2::new(dst_rect.left(), dst_rect.center().y),
                        )
                    } else {
                        (
                            Pos2::new(src_rect.left(), src_rect.center().y),
                            Pos2::new(dst_rect.right(), dst_rect.center().y),
                        )
                    };
                    let mid_dx = ((p3.x - p0.x).abs() * 0.45).max(32.0 * state.zoom);
                    let dir = if p3.x >= p0.x { 1.0 } else { -1.0 };
                    let p1 = Pos2::new(p0.x + dir * mid_dx, p0.y);
                    let p2 = Pos2::new(p3.x - dir * mid_dx, p3.y);
                    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                        [p0, p1, p2, p3],
                        false,
                        Color32::TRANSPARENT,
                        Stroke::new(stroke_w, edge_color),
                    ));
                    painter.circle_filled(p3, 3.5 * state.zoom.clamp(0.7, 1.3), edge_color);
                }

                let mut clicked_table: Option<String> = None;
                let mut dbl_clicked_table: Option<String> = None;
                for &idx in &visible_indices {
                    let card = &state.tables[idx];
                    let Some(&rect) = screen_rects.get(&card.name.to_ascii_lowercase()) else {
                        continue;
                    };
                    if !canvas_rect.intersects(rect) {
                        continue;
                    }
                    let is_selected = state
                        .selected_table
                        .as_deref()
                        .is_some_and(|sel| sel.eq_ignore_ascii_case(&card.name));
                    let card_resp = ui.interact(
                        rect.intersect(canvas_rect),
                        egui::Id::new(("er_card", &card.name)),
                        Sense::click(),
                    );
                    if card_resp.double_clicked() {
                        dbl_clicked_table = Some(card.name.clone());
                    } else if card_resp.clicked() {
                        clicked_table = Some(card.name.clone());
                    }

                    let border = if is_selected {
                        Stroke::new(2.0, p.accent)
                    } else {
                        Stroke::new(1.0, p.border)
                    };
                    painter.rect(
                        rect,
                        CornerRadius::same(8),
                        p.bg_card,
                        border,
                        egui::StrokeKind::Inside,
                    );
                    let header_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), CARD_HEADER_H * state.zoom));
                    painter.rect_filled(
                        header_rect,
                        CornerRadius {
                            nw: 8,
                            ne: 8,
                            sw: 0,
                            se: 0,
                        },
                        if is_selected { p.accent_soft } else { p.bg_hover },
                    );
                    let header_font = FontId::proportional((12.0 * state.zoom).clamp(8.0, 16.0));
                    painter.text(
                        Pos2::new(header_rect.left() + 8.0 * state.zoom, header_rect.center().y),
                        Align2::LEFT_CENTER,
                        super::truncate_display(&card.name, 26),
                        header_font,
                        if is_selected { p.accent } else { p.text_primary },
                    );

                    let row_font = FontId::new((10.5 * state.zoom).clamp(7.0, 14.0), FontFamily::Monospace);
                    let mut y = header_rect.bottom() + 4.0 * state.zoom;
                    let lower_tbl = card.name.to_ascii_lowercase();
                    for (col_name, col_type, is_pk) in card.columns.iter().take(MAX_COLS_SHOWN) {
                        let is_fk = fk_cols_by_table.contains(&(lower_tbl.clone(), col_name.to_ascii_lowercase()));
                        let prefix = if *is_pk {
                            "PK "
                        } else if is_fk {
                            "FK "
                        } else {
                            "   "
                        };
                        let col_color = if *is_pk {
                            p.warning
                        } else if is_fk {
                            p.accent
                        } else {
                            p.text_primary
                        };
                        painter.text(
                            Pos2::new(rect.left() + 8.0 * state.zoom, y + (CARD_ROW_H * 0.5) * state.zoom),
                            Align2::LEFT_CENTER,
                            format!("{prefix}{}", super::truncate_display(col_name, 16)),
                            row_font.clone(),
                            col_color,
                        );
                        painter.text(
                            Pos2::new(rect.right() - 8.0 * state.zoom, y + (CARD_ROW_H * 0.5) * state.zoom),
                            Align2::RIGHT_CENTER,
                            super::truncate_display(col_type, 12),
                            row_font.clone(),
                            p.text_muted,
                        );
                        y += CARD_ROW_H * state.zoom;
                    }
                    if card.columns.len() > MAX_COLS_SHOWN {
                        painter.text(
                            Pos2::new(rect.left() + 8.0 * state.zoom, y + (CARD_ROW_H * 0.5) * state.zoom),
                            Align2::LEFT_CENTER,
                            format!("+{} more columns", card.columns.len() - MAX_COLS_SHOWN),
                            row_font,
                            p.text_muted,
                        );
                    }
                }

                if let Some(tbl) = clicked_table {
                    state.selected_table = Some(tbl);
                }
                if let Some(tbl) = dbl_clicked_table {
                    open_table_req = Some((state.slot, state.schema.clone(), tbl));
                }
            },
            |ui| {
                let shown_note = if let Some(ref rel_summary) = selected_rel_summary {
                    super::truncate_display(rel_summary, 96)
                } else if table_count > 80 {
                    format!("Showing up to 80 of {table_count} tables — use Filter to narrow")
                } else {
                    format!("{table_count} tables · {relation_count} relations")
                };
                caption(ui, p, shown_note);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if secondary_button(ui, p, icon::X, "Close", true).clicked() {
                        keep_open = false;
                    }
                });
            },
        );

        if zoom_delta != 0.0 {
            state.zoom = (state.zoom + zoom_delta).clamp(0.35, 1.8);
        }
        if reset_view {
            state.pan = Vec2::new(28.0, 28.0);
            state.zoom = 1.0;
        }
        if outcome.closed {
            keep_open = false;
        }
        if let Some((slot, schema, table)) = open_table_req {
            self.open_table_tab(slot, schema, table);
            keep_open = false;
        }
        if keep_open {
            self.er_diagram = Some(state);
        }
    }
}
