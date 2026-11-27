use crate::theme::{Palette, RADIUS_CARD, RADIUS_CONTROL};
use eframe::egui::{
    self, text::LayoutJob, Color32, CornerRadius, FontFamily, FontId, Margin, Response, RichText, Sense, Stroke,
    TextFormat, Ui,
};

mod canvas;
mod form;
mod modal;
mod tabs;

pub use canvas::{empty_state, loading_state, tree_toggle_icon};
pub use egui_phosphor::regular as icon;
pub use form::*;
pub use modal::*;
pub use tabs::{document_tab, underline_tab};

const CARD_SHADOW: egui::epaint::Shadow = egui::epaint::Shadow {
    offset: [0, 1],
    blur: 4,
    spread: 0,
    color: Color32::from_black_alpha(18),
};

pub fn card_frame(palette: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.bg_card)
        .shadow(CARD_SHADOW)
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(Margin::same(10))
}

pub fn inset_frame(palette: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.bg_surface)
        .stroke(Stroke::new(1.0, palette.divider))
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(Margin::same(8))
}

pub fn icon_text(glyph: &str, glyph_color: Color32, label: &str, label_color: Color32, size: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font_id = FontId::new(size, FontFamily::Proportional);
    job.append(
        glyph,
        0.0,
        TextFormat {
            font_id: font_id.clone(),
            color: glyph_color,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    if !label.is_empty() {
        job.append(
            label,
            6.0,
            TextFormat {
                font_id,
                color: label_color,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
    }
    job
}

fn with_ghost_visuals<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let widgets = &mut ui.visuals_mut().widgets;
    let prev_inactive_bg = widgets.inactive.weak_bg_fill;
    let prev_inactive_stroke = widgets.inactive.bg_stroke;
    let prev_hovered_stroke = widgets.hovered.bg_stroke;
    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.hovered.bg_stroke = Stroke::NONE;

    let result = add_contents(ui);

    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.weak_bg_fill = prev_inactive_bg;
    widgets.inactive.bg_stroke = prev_inactive_stroke;
    widgets.hovered.bg_stroke = prev_hovered_stroke;
    result
}

pub fn wrap_before_if_needed(ui: &mut Ui, required_width: f32) {
    if ui.layout().main_wrap()
        && ui.layout().is_horizontal()
        && ui.cursor().left() > ui.max_rect().left()
        && ui.available_size_before_wrap().x < required_width
    {
        ui.end_row();
    }
}

pub fn ghost_button(ui: &mut Ui, palette: Palette, glyph: &str, label: &str, is_enabled: bool) -> Response {
    let text = icon_text(glyph, palette.text_muted, label, palette.text_primary, 13.0);
    with_ghost_visuals(ui, |ui| {
        ui.add_enabled(
            is_enabled,
            egui::Button::new(text).wrap_mode(egui::TextWrapMode::Extend),
        )
    })
}

pub fn tinted_ghost_button(ui: &mut Ui, tint: Color32, glyph: &str, label: &str, is_enabled: bool) -> Response {
    let tone = if is_enabled {
        tint
    } else {
        ui.visuals().weak_text_color()
    };
    let text = icon_text(glyph, tone, label, tone, 13.0);
    with_ghost_visuals(ui, |ui| {
        ui.add_enabled(
            is_enabled,
            egui::Button::new(text).wrap_mode(egui::TextWrapMode::Extend),
        )
    })
}

pub fn icon_button(ui: &mut Ui, palette: Palette, glyph: &str, tooltip: &str) -> Response {
    icon_button_enabled(ui, palette, glyph, tooltip, true)
}

pub fn icon_button_enabled(
    ui: &mut Ui,
    palette: Palette,
    glyph: &str,
    tooltip: &str,
    is_enabled: bool,
) -> Response {
    let text = RichText::new(glyph).size(14.0).color(palette.text_muted);
    with_ghost_visuals(ui, |ui| {
        ui.add_enabled(
            is_enabled,
            egui::Button::new(text).wrap_mode(egui::TextWrapMode::Extend),
        )
    })
    .on_hover_text(tooltip)
}

pub fn primary_button(ui: &mut Ui, palette: Palette, glyph: &str, label: &str, is_enabled: bool) -> Response {
    let text = icon_text(glyph, palette.on_accent, label, palette.on_accent, 13.0);
    let button = egui::Button::new(text)
        .fill(palette.accent)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .min_size(egui::vec2(0.0, 28.0))
        .wrap_mode(egui::TextWrapMode::Extend);
    ui.add_enabled(is_enabled, button)
}

pub fn secondary_button(ui: &mut Ui, palette: Palette, glyph: &str, label: &str, is_enabled: bool) -> Response {
    let text = icon_text(glyph, palette.text_muted, label, palette.text_primary, 13.0);
    let button = egui::Button::new(text)
        .fill(palette.bg_card)
        .stroke(Stroke::new(1.0, palette.border))
        .min_size(egui::vec2(0.0, 28.0))
        .wrap_mode(egui::TextWrapMode::Extend);
    ui.add_enabled(is_enabled, button)
}

pub fn toolbar_divider(ui: &mut Ui, palette: Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 20.0), Sense::hover());
    ui.painter()
        .vline(rect.center().x, rect.y_range(), Stroke::new(1.0, palette.divider));
}

pub fn section_title(ui: &mut Ui, palette: Palette, glyph: &str, title: &str) {
    ui.add(egui::Label::new(icon_text(glyph, palette.icon_schema, title, palette.text_primary, 13.5)).truncate());
}

pub fn caption(ui: &mut Ui, palette: Palette, text: impl Into<String>) -> Response {
    ui.label(RichText::new(text).size(11.5).color(palette.text_muted))
}

pub fn pill(ui: &mut Ui, text: &str, color: Color32) {
    let galley = ui.fonts(|fonts| {
        fonts.layout_no_wrap(
            text.to_owned(),
            FontId::new(10.5, FontFamily::Proportional),
            color,
        )
    });
    let size = egui::vec2(galley.size().x + 16.0, galley.size().y + 2.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(255), color.gamma_multiply(0.14));
        let text_pos = egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        );
        painter.galley(text_pos, galley, color);
    }
}

pub fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

pub struct BannerState {
    pub dismissed: bool,
    pub hovered: bool,
}

const DISMISS_BUTTON_ALLOWANCE: f32 = 28.0;

pub fn banner(
    ui: &mut Ui,
    palette: Palette,
    tone: Color32,
    glyph: &str,
    message: &str,
    is_dismissible: bool,
) -> BannerState {
    let mut dismissed = false;
    let resp = egui::Frame::new()
        .fill(tone.gamma_multiply(0.1))
        .stroke(Stroke::new(1.0, tone.gamma_multiply(0.45)))
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if is_dismissible {
                ui.horizontal(|ui| {
                    let message_width = (ui.available_width() - DISMISS_BUTTON_ALLOWANCE).max(0.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(message_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(message_width);
                            ui.add(egui::Label::new(icon_text(glyph, tone, message, palette.text_primary, 12.5)).wrap());
                        },
                    );
                    dismissed = icon_button(ui, palette, icon::X, "Dismiss").clicked();
                });
            } else {
                ui.add(egui::Label::new(icon_text(glyph, tone, message, palette.text_primary, 12.5)).wrap());
            }
        })
        .response;
    BannerState {
        dismissed,
        hovered: resp.hovered(),
    }
}

pub fn busy_indicator(ui: &mut Ui, palette: Palette, message: &str) {
    ui.add(egui::Spinner::new().size(13.0).color(palette.accent));
    if !message.is_empty() {
        caption(ui, palette, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{install_fonts, AppTheme};

    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        AppTheme::Light.apply(&ctx);
        ctx
    }

    #[test]
    fn ghost_buttons_and_pills_wrap_cleanly_in_horizontal_wrapped_across_widths() {
        let ctx = test_ctx();
        let palette = AppTheme::Light.palette();
        let mut where_clause = "dcc = '1'".to_string();

        for avail_w in (420..=1520).step_by(8) {
            let avail_w = avail_w as f32;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(avail_w, 900.0),
                )),
                ..Default::default()
            };
            let mut toolbar_rect = egui::Rect::NOTHING;
            let mut max_item_h = 0.0_f32;

            for _ in 0..2 {
                let _ = ctx.run(raw.clone(), |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().inner_margin(Margin::ZERO))
                        .show(ctx, |ui| {
                            let resp = ui.horizontal_wrapped(|ui| {
                                max_item_h = max_item_h
                                    .max(ghost_button(ui, palette, icon::CODE, "Hide SQL", true).rect.height());
                                toolbar_divider(ui, palette);
                                ui.add(
                                    egui::Label::new(icon_text(
                                        icon::FUNNEL,
                                        palette.accent,
                                        "WHERE",
                                        palette.text_muted,
                                        11.5,
                                    ))
                                    .extend(),
                                );
                                ui.add(egui::TextEdit::singleline(&mut where_clause).desired_width(220.0));
                                max_item_h = max_item_h
                                    .max(ghost_button(ui, palette, icon::CHECK, "Apply", true).rect.height());
                                toolbar_divider(ui, palette);
                                max_item_h = max_item_h.max(
                                    icon_button_enabled(ui, palette, icon::CARET_LEFT, "Previous page", false)
                                        .rect
                                        .height(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new("Page 1").size(11.5).color(palette.text_muted),
                                    )
                                    .extend(),
                                );
                                max_item_h = max_item_h.max(
                                    icon_button_enabled(ui, palette, icon::CARET_RIGHT, "Next page", true)
                                        .rect
                                        .height(),
                                );
                                wrap_before_if_needed(ui, 108.0);
                                egui::ComboBox::from_id_salt("test_page_size")
                                    .selected_text("1,000 rows")
                                    .width(78.0)
                                    .show_ui(ui, |_| {});
                                max_item_h = max_item_h.max(
                                    ghost_button(ui, palette, icon::HASH, "~600.3K rows · Exact", true)
                                        .rect
                                        .height(),
                                );
                                toolbar_divider(ui, palette);
                                pill(ui, "EDIT merchant_data", palette.success);
                                max_item_h = max_item_h
                                    .max(ghost_button(ui, palette, icon::PLUS, "Row", true).rect.height());
                                max_item_h = max_item_h
                                    .max(ghost_button(ui, palette, icon::COPY, "Clone", true).rect.height());
                                max_item_h = max_item_h.max(
                                    tinted_ghost_button(ui, palette.danger, icon::TRASH, "Delete", true)
                                        .rect
                                        .height(),
                                );
                                toolbar_divider(ui, palette);
                                max_item_h = max_item_h
                                    .max(ghost_button(ui, palette, icon::FUNNEL, "Filter", true).rect.height());
                                toolbar_divider(ui, palette);
                                max_item_h = max_item_h
                                    .max(ghost_button(ui, palette, icon::ROWS, "Row list", true).rect.height());
                                toolbar_divider(ui, palette);
                                wrap_before_if_needed(ui, 118.0);
                                egui::ComboBox::from_id_salt("test_export_menu")
                                    .selected_text(icon_text(
                                        icon::EXPORT,
                                        palette.accent,
                                        "Export",
                                        palette.text_primary,
                                        13.0,
                                    ))
                                    .width(110.0)
                                    .show_ui(ui, |_| {});
                            });
                            toolbar_rect = resp.response.rect;
                        });
                });
            }

            assert!(
                max_item_h <= 30.0,
                "button vertically collapsed at avail_w={avail_w}: max_item_h={max_item_h}"
            );
            let max_expected_h = if avail_w >= 960.0 {
                64.0
            } else if avail_w >= 640.0 {
                96.0
            } else {
                148.0
            };
            assert!(
                toolbar_rect.height() <= max_expected_h,
                "toolbar height inflated at avail_w={avail_w}: h={} (max expected {max_expected_h})",
                toolbar_rect.height()
            );
            assert!(
                toolbar_rect.right() <= avail_w + 4.0,
                "toolbar overflowed right edge at avail_w={avail_w}: right={}",
                toolbar_rect.right()
            );
        }
    }

    #[test]
    fn with_ghost_visuals_restores_parent_widget_visuals() {
        let ctx = test_ctx();
        let palette = AppTheme::Light.palette();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0))),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let before_bg = ui.visuals().widgets.inactive.weak_bg_fill;
                let before_stroke = ui.visuals().widgets.inactive.bg_stroke;
                let before_hover_stroke = ui.visuals().widgets.hovered.bg_stroke;
                let _ = ghost_button(ui, palette, icon::FUNNEL, "Filter", true);
                assert_eq!(ui.visuals().widgets.inactive.weak_bg_fill, before_bg);
                assert_eq!(ui.visuals().widgets.inactive.bg_stroke, before_stroke);
                assert_eq!(ui.visuals().widgets.hovered.bg_stroke, before_hover_stroke);
            });
        });
    }
}
