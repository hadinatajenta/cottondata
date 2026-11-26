use super::icon_text;
use crate::theme::Palette;
use eframe::egui::{self, Color32, Rect, Response, Stroke, Ui};

const DOT_GRID_SPACING: f32 = 18.0;

pub fn tree_toggle_icon(ui: &mut Ui, openness: f32, response: &Response) {
    let color = if response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let center = response.rect.center();
    let half = 4.0;
    let stroke = Stroke::new(1.3, color);
    let painter = ui.painter();
    painter.hline(center.x - half..=center.x + half, center.y, stroke);
    let vertical_half = half * (1.0 - openness);
    if vertical_half > 0.1 {
        painter.vline(center.x, center.y - vertical_half..=center.y + vertical_half, stroke);
    }
}

pub fn paint_dot_grid(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter_at(rect);
    let start_x = (rect.left() / DOT_GRID_SPACING).ceil() * DOT_GRID_SPACING;
    let start_y = (rect.top() / DOT_GRID_SPACING).ceil() * DOT_GRID_SPACING;
    let mut y = start_y;
    while y < rect.bottom() {
        let mut x = start_x;
        while x < rect.right() {
            painter.circle_filled(egui::pos2(x, y), 1.0, color);
            x += DOT_GRID_SPACING;
        }
        y += DOT_GRID_SPACING;
    }
}

pub fn empty_state(ui: &mut Ui, palette: Palette, glyph: &str, message: &str) {
    let rect = ui.available_rect_before_wrap();
    paint_dot_grid(ui, rect, palette.dot_grid);
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.centered_and_justified(|ui| {
            ui.label(icon_text(glyph, palette.text_muted, message, palette.text_muted, 13.0));
        });
    });
}

pub fn loading_state(ui: &mut Ui, palette: Palette, message: &str) {
    let rect = ui.available_rect_before_wrap();
    paint_dot_grid(ui, rect, palette.dot_grid);
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((rect.height() / 2.0 - 24.0).max(0.0));
            ui.add(egui::Spinner::new().size(22.0).color(palette.accent));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(message).size(13.0).color(palette.text_muted));
        });
    });
}
