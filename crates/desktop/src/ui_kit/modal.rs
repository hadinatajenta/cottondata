use super::{icon, icon_button};
use crate::theme::Palette;
use eframe::egui::{
    self, epaint::Shadow, Align2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
};

const MODAL_RADIUS: u8 = 12;
const ACCENT_BAR_HEIGHT: f32 = 3.0;
const ICON_BADGE_SIZE: f32 = 32.0;

pub struct ModalConfig<'a> {
    pub id: &'a str,
    pub icon_glyph: &'a str,
    pub icon_color: Color32,
    pub title: &'a str,
    pub subtitle: Option<&'a str>,
    pub accent_bar: Option<Color32>,
    pub width: f32,
    pub max_height: Option<f32>,
    pub dismiss_on_backdrop: bool,
}

pub struct ModalOutcome {
    pub closed: bool,
}

pub fn modal_shell(
    ctx: &egui::Context,
    palette: Palette,
    cfg: ModalConfig<'_>,
    body: impl FnOnce(&mut Ui),
    footer: impl FnOnce(&mut Ui),
) -> ModalOutcome {
    let screen = ctx.screen_rect();
    let is_dark = palette.bg_card.r() < 128;
    let mut closed = ctx.input(|i| i.key_pressed(egui::Key::Escape));

    egui::Area::new(egui::Id::new((cfg.id, "backdrop")))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .movable(false)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::click());
            let scrim_alpha = if is_dark { 120 } else { 72 };
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(scrim_alpha));
            if cfg.dismiss_on_backdrop && response.clicked() {
                closed = true;
            }
        });

    let card_width = cfg.width.min((screen.width() - 32.0).max(0.0));
    let shadow_alpha = if is_dark { 140 } else { 52 };

    egui::Area::new(egui::Id::new((cfg.id, "dialog")))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .movable(false)
        .show(ctx, |ui| {
            let frame_response = egui::Frame::new()
                .fill(palette.bg_card)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(CornerRadius::same(MODAL_RADIUS))
                .shadow(Shadow {
                    offset: [0, 14],
                    blur: 36,
                    spread: 0,
                    color: Color32::from_black_alpha(shadow_alpha),
                })
                .inner_margin(Margin::ZERO)
                .show(ui, |ui| {
                    let default_spacing = ui.spacing().item_spacing;
                    ui.set_width(card_width);
                    ui.set_max_width(card_width);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                    if let Some(accent) = cfg.accent_bar {
                        paint_accent_bar(ui, card_width, accent);
                    }

                    if render_header(ui, palette, &cfg, card_width, default_spacing) {
                        closed = true;
                    }

                    paint_divider(ui, card_width, palette.divider);

                    render_body(ui, &cfg, screen, card_width, default_spacing, body);

                    paint_divider(ui, card_width, palette.divider);

                    render_footer(ui, palette, card_width, default_spacing, footer);
                });

            ui.painter().rect_stroke(
                frame_response.response.rect,
                CornerRadius::same(MODAL_RADIUS),
                Stroke::new(1.0, palette.border),
                StrokeKind::Inside,
            );
        });

    ModalOutcome { closed }
}

fn paint_accent_bar(ui: &mut Ui, width: f32, color: Color32) {
    let (bar_rect, _) = ui.allocate_exact_size(egui::vec2(width, ACCENT_BAR_HEIGHT), Sense::hover());
    let painter = ui.painter_at(bar_rect);
    let rounded_top = Rect::from_min_size(bar_rect.min, egui::vec2(bar_rect.width(), f32::from(MODAL_RADIUS) * 2.0));
    painter.rect_filled(
        rounded_top,
        CornerRadius {
            nw: MODAL_RADIUS,
            ne: MODAL_RADIUS,
            sw: 0,
            se: 0,
        },
        color,
    );
}

fn paint_divider(ui: &mut Ui, width: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, color));
}

fn render_header(
    ui: &mut Ui,
    palette: Palette,
    cfg: &ModalConfig<'_>,
    card_width: f32,
    default_spacing: egui::Vec2,
) -> bool {
    let mut close_clicked = false;
    egui::Frame::new()
        .inner_margin(Margin::symmetric(20, 14))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = default_spacing;
            let inner_width = (card_width - 40.0).max(0.0);
            ui.set_width(inner_width);
            ui.horizontal(|ui| {
                let (badge_rect, _) =
                    ui.allocate_exact_size(egui::vec2(ICON_BADGE_SIZE, ICON_BADGE_SIZE), Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(badge_rect, CornerRadius::same(8), cfg.icon_color.gamma_multiply(0.14));
                painter.text(
                    badge_rect.center(),
                    Align2::CENTER_CENTER,
                    cfg.icon_glyph,
                    FontId::proportional(16.0),
                    cfg.icon_color,
                );

                ui.add_space(4.0);

                let text_width = (ui.available_width() - 36.0).max(0.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(text_width, ICON_BADGE_SIZE),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_max_width(text_width);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if cfg.subtitle.is_none() {
                            ui.add_space(6.0);
                        }
                        ui.add(
                            egui::Label::new(
                                RichText::new(cfg.title)
                                    .size(15.0)
                                    .strong()
                                    .color(palette.text_primary),
                            )
                            .truncate(),
                        );
                        if let Some(subtitle) = cfg.subtitle {
                            ui.add(
                                egui::Label::new(RichText::new(subtitle).size(11.5).color(palette.text_muted))
                                    .truncate(),
                            );
                        }
                    },
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, palette, icon::X, "Close (Esc)").clicked() {
                        close_clicked = true;
                    }
                });
            });
        });
    close_clicked
}

fn render_body(
    ui: &mut Ui,
    cfg: &ModalConfig<'_>,
    screen: Rect,
    card_width: f32,
    default_spacing: egui::Vec2,
    body: impl FnOnce(&mut Ui),
) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(20, 16))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = default_spacing;
            let inner_width = (card_width - 40.0).max(0.0);
            ui.set_width(inner_width);
            ui.set_max_width(inner_width);
            if let Some(max_h) = cfg.max_height {
                let clamped_height = max_h.min((screen.height() - 180.0).max(120.0));
                egui::ScrollArea::vertical()
                    .id_salt((cfg.id, "scroll"))
                    .max_height(clamped_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        body(ui);
                    });
            } else {
                body(ui);
            }
        });
}

fn render_footer(
    ui: &mut Ui,
    palette: Palette,
    card_width: f32,
    default_spacing: egui::Vec2,
    footer: impl FnOnce(&mut Ui),
) {
    egui::Frame::new()
        .fill(palette.bg_surface)
        .corner_radius(CornerRadius {
            nw: 0,
            ne: 0,
            sw: MODAL_RADIUS,
            se: MODAL_RADIUS,
        })
        .inner_margin(Margin::symmetric(20, 12))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = default_spacing;
            let inner_width = (card_width - 40.0).max(0.0);
            ui.set_width(inner_width);
            ui.horizontal(|ui| {
                ui.set_min_width(ui.available_width());
                footer(ui);
            });
        });
}
