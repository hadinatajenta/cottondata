use crate::theme::{Palette, RADIUS_CONTROL};
use crate::ui_kit::icon;
use cottondata_core::ServerSlotId;
use eframe::egui::{
    self, epaint::Shadow, Align, Align2, Color32, CornerRadius, FontId, Margin, RichText, Sense, Stroke, StrokeKind,
};

const PALETTE_WIDTH: f32 = 560.0;
const PALETTE_TOP_OFFSET: f32 = 84.0;
const MAX_VISIBLE_RESULTS: usize = 50;
const ROW_HEIGHT: f32 = 38.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickSwitcherTarget {
    Server(ServerSlotId),
    Schema {
        slot_id: ServerSlotId,
        schema: String,
    },
    Table {
        slot_id: ServerSlotId,
        schema: String,
        table: String,
    },
}

#[derive(Debug, Clone)]
pub struct QuickSwitcherItem {
    pub title: String,
    pub subtitle: String,
    pub badge: String,
    pub badge_rgb: [u8; 3],
    pub target: QuickSwitcherTarget,
}

#[derive(Debug, Clone, Default)]
pub struct QuickSwitcherState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    pub focus_input: bool,
}

impl QuickSwitcherState {
    pub fn open_fresh(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.focus_input = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focus_input = false;
    }
}

pub fn filter_quick_switcher_items(
    items: &[QuickSwitcherItem],
    query: &str,
    max_results: usize,
) -> Vec<usize> {
    if max_results == 0 || items.is_empty() {
        return Vec::new();
    }
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return (0..items.len().min(max_results)).collect();
    }

    let query_lower: Vec<char> = trimmed.to_lowercase().chars().collect();
    let mut scored: Vec<(i32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| score_item(item, &query_lower).map(|score| (score, idx)))
        .collect();

    scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.truncate(max_results);
    scored.into_iter().map(|(_, idx)| idx).collect()
}

fn score_item(item: &QuickSwitcherItem, query_lower: &[char]) -> Option<i32> {
    let title_lower: Vec<char> = item.title.to_lowercase().chars().collect();
    if let Some(score) = score_field(&title_lower, query_lower, 1_000) {
        return Some(score);
    }
    let subtitle_lower: Vec<char> = item.subtitle.to_lowercase().chars().collect();
    if let Some(score) = score_field(&subtitle_lower, query_lower, 400) {
        return Some(score);
    }
    let mut combined = Vec::with_capacity(title_lower.len() + 1 + subtitle_lower.len());
    combined.extend_from_slice(&title_lower);
    combined.push(' ');
    combined.extend_from_slice(&subtitle_lower);
    score_field(&combined, query_lower, 200)
}

fn score_field(haystack: &[char], needle: &[char], tier_bonus: i32) -> Option<i32> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    if haystack == needle {
        return Some(tier_bonus + 5_000);
    }
    if haystack.starts_with(needle) {
        let len_penalty = (haystack.len() - needle.len()).min(200) as i32;
        return Some(tier_bonus + 3_000 - len_penalty);
    }
    if let Some(pos) = find_subslice(haystack, needle) {
        let boundary_bonus = if pos == 0 || is_word_boundary(haystack[pos - 1]) {
            400
        } else {
            0
        };
        let pos_penalty = (pos * 4).min(400) as i32;
        let len_penalty = (haystack.len() - needle.len()).min(200) as i32;
        return Some(tier_bonus + 1_500 + boundary_bonus - pos_penalty - len_penalty);
    }

    let mut q_idx = 0usize;
    let mut prev_match: Option<usize> = None;
    let mut score = tier_bonus + 300;
    for (h_idx, &ch) in haystack.iter().enumerate() {
        if ch == needle[q_idx] {
            if let Some(prev) = prev_match {
                if h_idx == prev + 1 {
                    score += 35;
                } else {
                    score -= ((h_idx - prev).min(20) as i32) * 2;
                }
            } else {
                score -= (h_idx.min(30) as i32) * 2;
            }
            if h_idx == 0 || is_word_boundary(haystack[h_idx - 1]) {
                score += 25;
            }
            prev_match = Some(h_idx);
            q_idx += 1;
            if q_idx == needle.len() {
                let len_penalty = (haystack.len() - needle.len()).min(150) as i32;
                return Some(score - len_penalty);
            }
        }
    }
    None
}

fn find_subslice(haystack: &[char], needle: &[char]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn is_word_boundary(ch: char) -> bool {
    matches!(ch, '_' | '-' | '.' | '/' | ' ' | ':')
}

pub fn render_quick_switcher(
    ctx: &egui::Context,
    state: &mut QuickSwitcherState,
    items: &[QuickSwitcherItem],
    p: &Palette,
) -> Option<QuickSwitcherTarget> {
    if !state.open {
        return None;
    }

    let palette = *p;
    let screen = ctx.screen_rect();
    let is_dark = palette.bg_card.r() < 128;

    let (esc, arrow_down, arrow_up, enter) = ctx.input_mut(|input| {
        let esc = input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
        let down = input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
        let up = input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
        let enter = input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
        (esc, down, up, enter)
    });

    if esc {
        state.close();
        return None;
    }

    let mut backdrop_clicked = false;
    egui::Area::new(egui::Id::new("quick_switcher_backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .movable(false)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::click());
            let scrim_alpha = if is_dark { 116 } else { 64 };
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(scrim_alpha));
            if response.clicked() {
                backdrop_clicked = true;
            }
        });

    if backdrop_clicked {
        state.close();
        return None;
    }

    let mut filtered = filter_quick_switcher_items(items, &state.query, MAX_VISIBLE_RESULTS);
    let mut navigated_by_key = false;
    if !filtered.is_empty() {
        if arrow_down {
            state.selected = (state.selected + 1) % filtered.len();
            navigated_by_key = true;
        } else if arrow_up {
            state.selected = if state.selected == 0 {
                filtered.len() - 1
            } else {
                state.selected - 1
            };
            navigated_by_key = true;
        }
        if state.selected >= filtered.len() {
            state.selected = filtered.len() - 1;
        }
    } else {
        state.selected = 0;
    }

    let mut chosen: Option<QuickSwitcherTarget> = None;
    if enter && !filtered.is_empty() {
        if let Some(&item_idx) = filtered.get(state.selected) {
            chosen = Some(items[item_idx].target.clone());
        }
    }

    let card_width = PALETTE_WIDTH.min((screen.width() - 32.0).max(280.0));
    let shadow_alpha = if is_dark { 150 } else { 56 };
    let input_id = egui::Id::new("quick_switcher_input");

    egui::Area::new(egui::Id::new("quick_switcher_palette"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_TOP, egui::vec2(0.0, PALETTE_TOP_OFFSET))
        .movable(false)
        .show(ctx, |ui| {
            let frame_resp = egui::Frame::new()
                .fill(palette.bg_card)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(CornerRadius::same(12))
                .shadow(Shadow {
                    offset: [0, 14],
                    blur: 36,
                    spread: 0,
                    color: Color32::from_black_alpha(shadow_alpha),
                })
                .inner_margin(Margin::ZERO)
                .show(ui, |ui| {
                    ui.set_width(card_width);
                    ui.set_max_width(card_width);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_width(card_width - 28.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                ui.label(
                                    RichText::new(icon::MAGNIFYING_GLASS)
                                        .size(15.0)
                                        .color(palette.text_muted),
                                );
                                let edit_w = (ui.available_width() - 28.0).max(120.0);
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut state.query)
                                        .id(input_id)
                                        .frame(false)
                                        .font(FontId::proportional(14.0))
                                        .hint_text(
                                            RichText::new("Jump to server, schema, or table…")
                                                .color(palette.text_muted.gamma_multiply(0.75)),
                                        )
                                        .desired_width(edit_w),
                                );
                                if state.focus_input {
                                    resp.request_focus();
                                    state.focus_input = false;
                                }
                                if resp.changed() {
                                    state.selected = 0;
                                    filtered = filter_quick_switcher_items(
                                        items,
                                        &state.query,
                                        MAX_VISIBLE_RESULTS,
                                    );
                                }
                                if !state.query.is_empty()
                                    && ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(icon::X)
                                                    .size(12.0)
                                                    .color(palette.text_muted),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text("Clear")
                                        .clicked()
                                {
                                    state.query.clear();
                                    state.selected = 0;
                                    state.focus_input = true;
                                    filtered = filter_quick_switcher_items(
                                        items,
                                        &state.query,
                                        MAX_VISIBLE_RESULTS,
                                    );
                                }
                            });
                        });

                    let (div_rect, _) = ui.allocate_exact_size(egui::vec2(card_width, 1.0), Sense::hover());
                    ui.painter().hline(
                        div_rect.x_range(),
                        div_rect.center().y,
                        Stroke::new(1.0, palette.divider),
                    );

                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            let inner_w = card_width - 16.0;
                            ui.set_width(inner_w);
                            if filtered.is_empty() {
                                ui.add_space(18.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        RichText::new("No matching servers, schemas, or tables")
                                            .size(12.5)
                                            .color(palette.text_muted),
                                    );
                                });
                                ui.add_space(18.0);
                            } else {
                                egui::ScrollArea::vertical()
                                    .id_salt("quick_switcher_results")
                                    .max_height(340.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        for (vis_idx, &item_idx) in filtered.iter().enumerate() {
                                            let item = &items[item_idx];
                                            let is_selected = vis_idx == state.selected;
                                            let row_resp = render_result_row(
                                                ui,
                                                palette,
                                                item,
                                                inner_w,
                                                is_selected,
                                            );
                                            if is_selected && navigated_by_key {
                                                row_resp.scroll_to_me(Some(Align::Center));
                                            }
                                            if row_resp.hovered() && !navigated_by_key {
                                                state.selected = vis_idx;
                                            }
                                            if row_resp.clicked() {
                                                chosen = Some(item.target.clone());
                                            }
                                        }
                                    });
                            }
                        });

                    let (footer_div, _) = ui.allocate_exact_size(egui::vec2(card_width, 1.0), Sense::hover());
                    ui.painter().hline(
                        footer_div.x_range(),
                        footer_div.center().y,
                        Stroke::new(1.0, palette.divider),
                    );

                    egui::Frame::new()
                        .fill(palette.bg_surface)
                        .corner_radius(CornerRadius {
                            nw: 0,
                            ne: 0,
                            sw: 12,
                            se: 12,
                        })
                        .inner_margin(Margin::symmetric(14, 7))
                        .show(ui, |ui| {
                            ui.set_width(card_width - 28.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("↑↓ Navigate · ↵ Open · Esc Close")
                                        .size(11.0)
                                        .color(palette.text_muted),
                                );
                                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                    let count_label = if filtered.len() == MAX_VISIBLE_RESULTS
                                        && items.len() > MAX_VISIBLE_RESULTS
                                    {
                                        format!("Top {MAX_VISIBLE_RESULTS} matches")
                                    } else {
                                        format!("{} result(s)", filtered.len())
                                    };
                                    ui.label(
                                        RichText::new(count_label)
                                            .size(11.0)
                                            .color(palette.text_muted),
                                    );
                                });
                            });
                        });
                });

            ui.painter().rect_stroke(
                frame_resp.response.rect,
                CornerRadius::same(12),
                Stroke::new(1.0, palette.border),
                StrokeKind::Inside,
            );
        });

    if chosen.is_some() {
        state.close();
    }
    chosen
}

fn render_result_row(
    ui: &mut egui::Ui,
    palette: Palette,
    item: &QuickSwitcherItem,
    width: f32,
    is_selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if is_selected {
            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), palette.accent_soft);
        } else if response.hovered() {
            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), palette.bg_hover);
        }

        let (glyph, glyph_color) = match &item.target {
            QuickSwitcherTarget::Server(_) => (icon::DATABASE, palette.accent),
            QuickSwitcherTarget::Schema { .. } => (icon::FOLDER, palette.icon_schema),
            QuickSwitcherTarget::Table { .. } => (icon::TABLE, palette.icon_table),
        };

        painter.text(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(14.0),
            glyph_color,
        );

        let badge_width = if item.badge.is_empty() {
            0.0
        } else {
            let badge_galley = ui.fonts(|f| {
                f.layout_no_wrap(item.badge.clone(), FontId::proportional(10.5), Color32::WHITE)
            });
            let pill_w = badge_galley.size().x + 14.0;
            let pill_h = 19.0;
            let pill_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - pill_w - 10.0, rect.center().y - pill_h * 0.5),
                egui::vec2(pill_w, pill_h),
            );
            let [r, g, b] = item.badge_rgb;
            let badge_color = Color32::from_rgb(r, g, b);
            painter.rect_filled(
                pill_rect,
                CornerRadius::same(255),
                badge_color.gamma_multiply(0.16),
            );
            painter.galley(
                egui::pos2(
                    pill_rect.center().x - badge_galley.size().x * 0.5,
                    pill_rect.center().y - badge_galley.size().y * 0.5,
                ),
                badge_galley,
                badge_color,
            );
            pill_w + 18.0
        };

        let text_left = rect.left() + 30.0;
        let text_max_w = (rect.right() - badge_width - text_left - 8.0).max(40.0);

        let title_color = if is_selected {
            palette.accent
        } else {
            palette.text_primary
        };
        let title_galley = ui.fonts(|f| {
            let mut job = egui::text::LayoutJob::simple(
                item.title.clone(),
                FontId::proportional(13.0),
                title_color,
                text_max_w,
            );
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            f.layout_job(job)
        });

        if item.subtitle.is_empty() {
            painter.galley(
                egui::pos2(text_left, rect.center().y - title_galley.size().y * 0.5),
                title_galley,
                title_color,
            );
        } else {
            let sub_galley = ui.fonts(|f| {
                let mut job = egui::text::LayoutJob::simple(
                    item.subtitle.clone(),
                    FontId::proportional(11.0),
                    palette.text_muted,
                    text_max_w,
                );
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                job.wrap.overflow_character = Some('…');
                f.layout_job(job)
            });
            let total_h = title_galley.size().y + sub_galley.size().y;
            let top = rect.center().y - total_h * 0.5;
            let title_h = title_galley.size().y;
            painter.galley(egui::pos2(text_left, top), title_galley, title_color);
            painter.galley(
                egui::pos2(text_left, top + title_h),
                sub_galley,
                palette.text_muted,
            );
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_table_item(title: &str, subtitle: &str) -> QuickSwitcherItem {
        let slot_id = ServerSlotId::new(1).expect("valid slot");
        QuickSwitcherItem {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            badge: "Table".to_string(),
            badge_rgb: [58, 104, 222],
            target: QuickSwitcherTarget::Table {
                slot_id,
                schema: "app".to_string(),
                table: title.to_string(),
            },
        }
    }

    #[test]
    fn ranks_exact_prefix_substring_and_fuzzy_matches() {
        let items = vec![
            make_table_item("u_s_e_r_logs", "analytics"),
            make_table_item("audit_user_events", "analytics"),
            make_table_item("users_archive", "app"),
            make_table_item("user", "app"),
            make_table_item("orders", "billing"),
        ];

        let ranked = filter_quick_switcher_items(&items, "user", 50);
        assert_eq!(ranked, vec![3, 2, 1, 0]);
    }

    #[test]
    fn matches_case_insensitively_and_across_subtitle() {
        let items = vec![
            make_table_item("invoices", "Prod Main · billing"),
            make_table_item("users", "Local · app"),
        ];

        let ranked = filter_quick_switcher_items(&items, "BILLING", 50);
        assert_eq!(ranked, vec![0]);

        let ranked_title = filter_quick_switcher_items(&items, "USR", 50);
        assert_eq!(ranked_title, vec![1]);
    }

    #[test]
    fn empty_query_returns_original_order_and_respects_limit() {
        let items: Vec<QuickSwitcherItem> = (0..10)
            .map(|i| make_table_item(&format!("table_{i}"), "app"))
            .collect();

        assert_eq!(filter_quick_switcher_items(&items, "   ", 4), vec![0, 1, 2, 3]);
        assert!(filter_quick_switcher_items(&items, "table", 0).is_empty());
    }
}
