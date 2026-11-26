use super::{icon, icon_text};
use crate::theme::{Palette, RADIUS_CONTROL};
use eframe::egui::{self, Align2, CornerRadius, FontId, Rect, Response, Sense, Stroke, Ui};

const TAB_HEIGHT: f32 = 32.0;
const TAB_PADDING_X: f32 = 12.0;
const TAB_CLOSE_SIZE: f32 = 16.0;

pub struct DocumentTabResponse {
    pub is_clicked: bool,
    pub is_close_clicked: bool,
}

pub fn document_tab(
    ui: &mut Ui,
    palette: Palette,
    glyph: &str,
    label: &str,
    is_active: bool,
    is_closable: bool,
) -> DocumentTabResponse {
    let label_color = if is_active {
        palette.text_primary
    } else {
        palette.text_muted
    };
    let glyph_color = if is_active {
        palette.icon_schema
    } else {
        palette.text_muted
    };
    let galley = ui.fonts(|fonts| fonts.layout_job(icon_text(glyph, glyph_color, label, label_color, 13.0)));
    let close_width = if is_closable { TAB_CLOSE_SIZE + 6.0 } else { 0.0 };
    let tab_size = egui::vec2(galley.size().x + TAB_PADDING_X * 2.0 + close_width, TAB_HEIGHT);
    let (tab_rect, tab_response) = ui.allocate_exact_size(tab_size, Sense::click());

    let close_rect = Rect::from_center_size(
        egui::pos2(
            tab_rect.right() - TAB_PADDING_X - TAB_CLOSE_SIZE / 2.0 + 4.0,
            tab_rect.center().y,
        ),
        egui::vec2(TAB_CLOSE_SIZE, TAB_CLOSE_SIZE),
    );
    let close_response = is_closable.then(|| ui.interact(close_rect, tab_response.id.with("close"), Sense::click()));

    if ui.is_rect_visible(tab_rect) {
        let painter = ui.painter();
        let top_radius = CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        };
        if is_active {
            painter.rect_filled(tab_rect, top_radius, palette.bg_card);
            painter.rect_stroke(
                tab_rect,
                top_radius,
                Stroke::new(1.0, palette.border),
                egui::StrokeKind::Inside,
            );
            painter.hline(
                tab_rect.x_range().shrink(1.0),
                tab_rect.bottom() - 0.5,
                Stroke::new(1.5, palette.bg_card),
            );
        } else if tab_response.hovered() {
            painter.rect_filled(
                tab_rect.shrink2(egui::vec2(0.0, 3.0)),
                CornerRadius::same(RADIUS_CONTROL),
                palette.bg_hover,
            );
        }
        let text_pos = egui::pos2(
            tab_rect.left() + TAB_PADDING_X,
            tab_rect.center().y - galley.size().y / 2.0,
        );
        painter.galley(text_pos, galley, label_color);

        if let Some(close) = &close_response {
            if close.hovered() {
                painter.rect_filled(close_rect, CornerRadius::same(4), palette.bg_hover);
            }
            painter.text(
                close_rect.center(),
                Align2::CENTER_CENTER,
                icon::X,
                FontId::proportional(11.0),
                if close.hovered() {
                    palette.text_primary
                } else {
                    palette.text_muted
                },
            );
        }
    }

    let is_close_clicked = close_response.is_some_and(|close| close.clicked());
    DocumentTabResponse {
        is_clicked: tab_response.clicked() && !is_close_clicked,
        is_close_clicked,
    }
}

pub fn underline_tab(ui: &mut Ui, palette: Palette, glyph: &str, label: &str, is_active: bool) -> Response {
    let label_color = if is_active {
        palette.text_primary
    } else {
        palette.text_muted
    };
    let glyph_color = if is_active {
        palette.highlight
    } else {
        palette.text_muted
    };
    let galley = ui.fonts(|fonts| fonts.layout_job(icon_text(glyph, glyph_color, label, label_color, 13.0)));
    let size = egui::vec2(galley.size().x + 20.0, 32.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() && !is_active {
            painter.rect_filled(
                rect.shrink2(egui::vec2(0.0, 4.0)),
                CornerRadius::same(RADIUS_CONTROL),
                palette.bg_hover,
            );
        }
        let text_pos = egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
        painter.galley(text_pos, galley, label_color);
        if is_active {
            let underline = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 2.0), rect.max);
            painter.rect_filled(underline, CornerRadius::same(1), palette.highlight);
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
