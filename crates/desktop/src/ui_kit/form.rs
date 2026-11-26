use super::{icon, icon_button, icon_text};
use crate::theme::{Palette, RADIUS_CONTROL};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Margin, Response, RichText, Sense, Stroke, StrokeKind, Ui,
};
use std::hash::Hash;

const INPUT_HEIGHT: f32 = 32.0;
const CARD_RADIUS: u8 = 8;
const SEGMENT_HEIGHT: f32 = 28.0;
const ENV_PILL_HEIGHT: f32 = 26.0;

pub fn field_label(ui: &mut Ui, palette: Palette, text: &str) {
    ui.label(RichText::new(text).size(11.5).strong().color(palette.text_muted));
    ui.add_space(3.0);
}

pub fn section_heading(ui: &mut Ui, palette: Palette, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(11.0)
            .strong()
            .color(palette.text_muted),
    );
}

pub fn styled_text_input(
    ui: &mut Ui,
    palette: Palette,
    id_salt: impl Hash,
    value: &mut String,
    hint: &str,
    width: f32,
) -> Response {
    let edit_id = ui.make_persistent_id(id_salt);
    let is_focused = ui.memory(|m| m.has_focus(edit_id));
    let stroke = if is_focused {
        Stroke::new(1.5, palette.accent)
    } else {
        Stroke::new(1.0, palette.border)
    };
    let inner_width = (width - 20.0).max(0.0);

    egui::Frame::new()
        .fill(palette.bg_surface)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(Margin::symmetric(10, 0))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(inner_width, INPUT_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .id(edit_id)
                            .frame(false)
                            .clip_text(true)
                            .hint_text(RichText::new(hint).color(palette.text_muted.gamma_multiply(0.75)))
                            .desired_width(inner_width),
                    )
                },
            )
            .inner
        })
        .inner
}

pub fn password_input(
    ui: &mut Ui,
    palette: Palette,
    id_salt: impl Hash,
    value: &mut String,
    show_password: &mut bool,
    hint: &str,
    width: f32,
) -> Response {
    let edit_id = ui.make_persistent_id(id_salt);
    let is_focused = ui.memory(|m| m.has_focus(edit_id));
    let stroke = if is_focused {
        Stroke::new(1.5, palette.accent)
    } else {
        Stroke::new(1.0, palette.border)
    };
    let inner_width = (width - 16.0).max(0.0);
    let toggle_allowance = 28.0;
    let edit_width = (inner_width - toggle_allowance).max(0.0);

    egui::Frame::new()
        .fill(palette.bg_surface)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(Margin {
            left: 10,
            right: 6,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(inner_width, INPUT_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let edit_response = ui.add(
                        egui::TextEdit::singleline(value)
                            .id(edit_id)
                            .password(!*show_password)
                            .frame(false)
                            .clip_text(true)
                            .hint_text(RichText::new(hint).color(palette.text_muted.gamma_multiply(0.75)))
                            .desired_width(edit_width),
                    );
                    ui.spacing_mut().button_padding = egui::vec2(4.0, 2.0);
                    let (glyph, tooltip) = if *show_password {
                        (icon::EYE_SLASH, "Hide password")
                    } else {
                        (icon::EYE, "Show password")
                    };
                    if icon_button(ui, palette, glyph, tooltip).clicked() {
                        *show_password = !*show_password;
                    }
                    edit_response
                },
            )
            .inner
        })
        .inner
}

pub fn segmented_tabs<T: Copy + PartialEq>(
    ui: &mut Ui,
    palette: Palette,
    current: &mut T,
    items: &[(T, &str, &str)],
) {
    egui::Frame::new()
        .fill(palette.bg_surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for &(value, glyph, label) in items {
                    let is_active = *current == value;
                    let job = if glyph.is_empty() {
                        icon_text(label, Color32::PLACEHOLDER, "", Color32::PLACEHOLDER, 12.5)
                    } else {
                        icon_text(glyph, Color32::PLACEHOLDER, label, Color32::PLACEHOLDER, 12.5)
                    };
                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                    let size = egui::vec2(galley.size().x + 22.0, SEGMENT_HEIGHT);
                    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

                    if ui.is_rect_visible(rect) {
                        let painter = ui.painter();
                        let text_color = if is_active {
                            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), palette.bg_card);
                            painter.rect_stroke(
                                rect,
                                CornerRadius::same(RADIUS_CONTROL),
                                Stroke::new(1.0, palette.border),
                                StrokeKind::Inside,
                            );
                            palette.accent
                        } else if response.hovered() {
                            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), palette.bg_hover);
                            palette.text_primary
                        } else {
                            palette.text_muted
                        };
                        let text_pos = egui::pos2(
                            rect.center().x - galley.size().x / 2.0,
                            rect.center().y - galley.size().y / 2.0,
                        );
                        painter.galley(text_pos, galley, text_color);
                    }

                    if response.clicked() {
                        *current = value;
                    }
                }
            });
        });
}

pub fn option_card(
    ui: &mut Ui,
    palette: Palette,
    glyph: &str,
    title: &str,
    subtitle: &str,
    selected: bool,
    width: f32,
) -> Response {
    let mut prepared = egui::Frame::new()
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(12, 10))
        .begin(ui);
    {
        let content_ui = &mut prepared.content_ui;
        let inner_width = (width - 24.0).max(0.0);
        content_ui.set_width(inner_width);
        content_ui.horizontal(|ui| {
            if !glyph.is_empty() {
                let (badge_rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::hover());
                let icon_color = if selected { palette.accent } else { palette.text_muted };
                let painter = ui.painter();
                painter.rect_filled(
                    badge_rect,
                    CornerRadius::same(RADIUS_CONTROL),
                    icon_color.gamma_multiply(0.14),
                );
                painter.text(
                    badge_rect.center(),
                    Align2::CENTER_CENTER,
                    glyph,
                    FontId::proportional(15.0),
                    icon_color,
                );
                ui.add_space(2.0);
            }
            let text_width = ui.available_width().max(0.0);
            ui.allocate_ui_with_layout(
                egui::vec2(text_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(text_width);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let title_color = if selected {
                        palette.accent
                    } else {
                        palette.text_primary
                    };
                    ui.add(egui::Label::new(RichText::new(title).size(13.0).strong().color(title_color)).truncate());
                    if !subtitle.is_empty() {
                        ui.add(egui::Label::new(RichText::new(subtitle).size(11.5).color(palette.text_muted)).wrap());
                    }
                },
            );
        });
    }

    let rect = prepared.allocate_space(ui).rect;
    let id = ui.make_persistent_id(("option_card", title, subtitle));
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    prepared.frame.fill = if selected {
        palette.accent_soft
    } else if response.hovered() {
        palette.bg_hover
    } else {
        palette.bg_surface
    };
    prepared.frame.stroke = if selected {
        Stroke::new(1.5, palette.accent)
    } else {
        Stroke::new(1.0, palette.border)
    };
    prepared.paint(ui);
    response
}

pub fn toggle_card(
    ui: &mut Ui,
    palette: Palette,
    checked: &mut bool,
    title: &str,
    description: &str,
) -> Response {
    let full_width = ui.available_width();
    let mut prepared = egui::Frame::new()
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(12, 10))
        .begin(ui);
    {
        let content_ui = &mut prepared.content_ui;
        let inner_width = (full_width - 24.0).max(0.0);
        content_ui.set_width(inner_width);
        content_ui.horizontal(|ui| {
            let (box_rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
            let painter = ui.painter();
            if *checked {
                painter.rect_filled(box_rect, CornerRadius::same(4), palette.accent);
                painter.text(
                    box_rect.center(),
                    Align2::CENTER_CENTER,
                    icon::CHECK,
                    FontId::proportional(11.0),
                    palette.on_accent,
                );
            } else {
                painter.rect_filled(box_rect, CornerRadius::same(4), palette.bg_card);
                painter.rect_stroke(
                    box_rect,
                    CornerRadius::same(4),
                    Stroke::new(1.0, palette.border),
                    StrokeKind::Inside,
                );
            }
            ui.add_space(2.0);
            let text_width = ui.available_width().max(0.0);
            ui.allocate_ui_with_layout(
                egui::vec2(text_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(text_width);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(
                        egui::Label::new(RichText::new(title).size(13.0).strong().color(palette.text_primary)).wrap(),
                    );
                    if !description.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(description).size(11.5).color(palette.text_muted)).wrap(),
                        );
                    }
                },
            );
        });
    }

    let rect = prepared.allocate_space(ui).rect;
    let id = ui.make_persistent_id(("toggle_card", title, description));
    let mut response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }

    prepared.frame.fill = if *checked {
        palette.accent_soft
    } else if response.hovered() {
        palette.bg_hover
    } else {
        palette.bg_surface
    };
    prepared.frame.stroke = if *checked {
        Stroke::new(1.0, palette.accent)
    } else {
        Stroke::new(1.0, palette.border)
    };
    prepared.paint(ui);
    response
}

pub fn env_pill_button(ui: &mut Ui, palette: Palette, label: &str, color: Color32, selected: bool) -> Response {
    let font_id = FontId::proportional(12.0);
    let galley = ui.fonts(|fonts| fonts.layout_no_wrap(label.to_owned(), font_id, Color32::PLACEHOLDER));
    let dot_allowance = 14.0;
    let size = egui::vec2(galley.size().x + dot_allowance + 20.0, ENV_PILL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let pill_radius = CornerRadius::same(255);
        let (fill, stroke, dot_color, label_color) = if selected {
            (
                color.gamma_multiply(0.18),
                Stroke::new(1.5, color),
                color,
                color,
            )
        } else if response.hovered() {
            (
                palette.bg_hover,
                Stroke::new(1.0, palette.border),
                color.gamma_multiply(0.85),
                palette.text_primary,
            )
        } else {
            (
                palette.bg_surface,
                Stroke::new(1.0, palette.border),
                color.gamma_multiply(0.65),
                palette.text_muted,
            )
        };
        painter.rect_filled(rect, pill_radius, fill);
        painter.rect_stroke(rect, pill_radius, stroke, StrokeKind::Inside);

        let dot_center = egui::pos2(rect.left() + 12.0, rect.center().y);
        painter.circle_filled(dot_center, 3.5, dot_color);

        let text_pos = egui::pos2(rect.left() + 21.0, rect.center().y - galley.size().y / 2.0);
        painter.galley(text_pos, galley, label_color);
    }

    response
}
