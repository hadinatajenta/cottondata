use crate::theme::{Palette, RADIUS_CARD, RADIUS_CONTROL};
use eframe::egui::{
    self, text::LayoutJob, Color32, CornerRadius, FontFamily, FontId, Margin, Response, RichText, Sense, Stroke,
    TextFormat, Ui,
};

mod canvas;
mod tabs;

pub use canvas::{empty_state, loading_state, tree_toggle_icon};
pub use egui_phosphor::regular as icon;
pub use tabs::{document_tab, underline_tab};

pub fn card_frame(palette: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.bg_card)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(Margin::same(10))
}

pub fn inset_frame(palette: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.bg_surface)
        .stroke(Stroke::new(1.0, palette.border))
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
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        widgets.inactive.bg_stroke = Stroke::NONE;
        widgets.hovered.bg_stroke = Stroke::NONE;
        add_contents(ui)
    })
    .inner
}

pub fn ghost_button(ui: &mut Ui, palette: Palette, glyph: &str, label: &str, is_enabled: bool) -> Response {
    let text = icon_text(glyph, palette.accent, label, palette.text_primary, 13.0);
    with_ghost_visuals(ui, |ui| ui.add_enabled(is_enabled, egui::Button::new(text)))
}

pub fn tinted_ghost_button(ui: &mut Ui, tint: Color32, glyph: &str, label: &str, is_enabled: bool) -> Response {
    let text = icon_text(glyph, tint, label, tint, 13.0);
    with_ghost_visuals(ui, |ui| ui.add_enabled(is_enabled, egui::Button::new(text)))
}

pub fn icon_button(ui: &mut Ui, palette: Palette, glyph: &str, tooltip: &str) -> Response {
    let text = RichText::new(glyph).size(14.0).color(palette.text_muted);
    with_ghost_visuals(ui, |ui| ui.add(egui::Button::new(text))).on_hover_text(tooltip)
}

pub fn primary_button(ui: &mut Ui, palette: Palette, glyph: &str, label: &str, is_enabled: bool) -> Response {
    let text = icon_text(glyph, palette.on_accent, label, palette.on_accent, 13.0);
    let button = egui::Button::new(text)
        .fill(palette.accent)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .min_size(egui::vec2(0.0, 28.0));
    ui.add_enabled(is_enabled, button)
}

pub fn secondary_button(ui: &mut Ui, palette: Palette, glyph: &str, label: &str, is_enabled: bool) -> Response {
    let text = icon_text(glyph, palette.text_muted, label, palette.text_primary, 13.0);
    let button = egui::Button::new(text)
        .fill(palette.bg_card)
        .stroke(Stroke::new(1.0, palette.border))
        .min_size(egui::vec2(0.0, 28.0));
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
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(8, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.5).strong().color(color));
        });
}

pub fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

pub struct BannerState {
    pub dismissed: bool,
    pub hovered: bool,
}

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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    dismissed = icon_button(ui, palette, icon::X, "Dismiss").clicked();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(icon_text(glyph, tone, message, palette.text_primary, 12.5)).wrap());
                    });
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
