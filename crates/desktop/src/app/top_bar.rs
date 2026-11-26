use super::server_form::ServerModalState;
use super::sessions::SessionModalState;
use super::update_modal::UpdateStatus;
use super::CottonDataApp;
use crate::theme::{AppTheme, Palette, GUTTER};
use crate::ui_kit::{card_frame, ghost_button, icon, icon_button, icon_text, pill, primary_button, toolbar_divider};
use crate::worker::SlotCmd;
use cottondata_core::ServerSlotId;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};

const LABELED_TOOLBAR_MIN_WIDTH: f32 = 1480.0;
const ADD_SERVER_LABEL_MIN_WIDTH: f32 = 1180.0;
const BRAND_TEXT_MIN_WIDTH: f32 = 980.0;
const NO_SCHEMA_LABEL: &str = "(no schema)";

struct TopBarLayout {
    show_action_labels: bool,
    show_brand_text: bool,
    show_add_server_label: bool,
    session_combo_w: f32,
    server_combo_w: f32,
    schema_combo_w: f32,
}

impl TopBarLayout {
    fn for_width(total_w: f32) -> Self {
        Self {
            show_action_labels: total_w >= LABELED_TOOLBAR_MIN_WIDTH,
            show_brand_text: total_w >= BRAND_TEXT_MIN_WIDTH,
            show_add_server_label: total_w >= ADD_SERVER_LABEL_MIN_WIDTH,
            session_combo_w: if total_w >= 1200.0 { 116.0 } else { 88.0 },
            server_combo_w: if total_w >= 1280.0 {
                165.0
            } else if total_w >= 1020.0 {
                130.0
            } else {
                104.0
            },
            schema_combo_w: if total_w >= 1280.0 {
                140.0
            } else if total_w >= 1020.0 {
                110.0
            } else {
                88.0
            },
        }
    }
}

impl CottonDataApp {
    pub(super) fn render_top_bar(&mut self, ctx: &egui::Context, palette: Palette) {
        let titlebar_height = window_titlebar_height(ctx);
        egui::TopBottomPanel::top("toolbar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(palette.bg_backdrop).inner_margin(Margin {
                left: GUTTER,
                right: GUTTER,
                top: if titlebar_height > 0.0 { 0 } else { GUTTER },
                bottom: 0,
            }))
            .show(ctx, |ui| {
                if titlebar_height > 0.0 {
                    render_titlebar_drag_region(ui, titlebar_height);
                }
                card_frame(palette)
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        let total_w = ui.available_width();
                        let layout = TopBarLayout::for_width(total_w);
                        let (row_rect, _) = ui.allocate_exact_size(egui::vec2(total_w, 28.0), egui::Sense::hover());

                        let mut left_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(row_rect)
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        left_ui.set_clip_rect(row_rect.intersect(ui.clip_rect()));
                        self.render_brand_group(&mut left_ui, palette, &layout);
                        toolbar_divider(&mut left_ui, palette);
                        self.render_transaction_group(&mut left_ui, palette, layout.show_action_labels);

                        let right_left = (left_ui.min_rect().right() + 8.0).min(row_rect.right());
                        let right_rect = egui::Rect::from_min_max(egui::pos2(right_left, row_rect.top()), row_rect.max);
                        if right_rect.width() > 24.0 {
                            let mut right_ui = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(right_rect)
                                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                            );
                            right_ui.set_clip_rect(right_rect.intersect(ui.clip_rect()));
                            self.render_connection_group(&mut right_ui, palette, &layout);
                        }
                    });
            });
    }

    fn render_brand_group(&mut self, ui: &mut egui::Ui, palette: Palette, layout: &TopBarLayout) {
        let brand_label = if layout.show_brand_text { "CottonData" } else { "" };
        ui.label(icon_text(
            icon::DATABASE,
            palette.accent,
            brand_label,
            palette.text_primary,
            14.0,
        ));
        toolbar_divider(ui, palette);
        self.render_session_picker(ui, palette, layout.session_combo_w);
        toolbar_divider(ui, palette);
        let console_label = if layout.show_action_labels { "Console" } else { "" };
        if ghost_button(ui, palette, icon::TERMINAL_WINDOW, console_label, true)
            .on_hover_text("Open a new SQL console")
            .clicked()
        {
            self.open_console_tab();
        }
        let switcher_hint = if cfg!(target_os = "macos") { "⌘K" } else { "Ctrl+K" };
        let switcher_label = if layout.show_brand_text { switcher_hint } else { "" };
        let switcher_tooltip = if cfg!(target_os = "macos") {
            "Quick Switcher (⌘K)"
        } else {
            "Quick Switcher (Ctrl+K)"
        };
        if ghost_button(ui, palette, icon::MAGNIFYING_GLASS, switcher_label, true)
            .on_hover_text(switcher_tooltip)
            .clicked()
        {
            self.open_quick_switcher();
        }
    }

    fn render_session_picker(&mut self, ui: &mut egui::Ui, palette: Palette, combo_w: f32) {
        let active_id = self.sessions_store.active_session_id.clone();
        let active_name = self.sessions_store.active_session().name.clone();
        let options: Vec<(String, String)> = self
            .sessions_store
            .sessions
            .iter()
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();

        let mut chosen_id = active_id.clone();
        let mut open_manager = false;
        egui::ComboBox::from_id_salt("active_session_combo")
            .truncate()
            .selected_text(icon_text(
                icon::STACK,
                palette.accent,
                &active_name,
                palette.text_primary,
                13.0,
            ))
            .width(combo_w)
            .show_ui(ui, |ui| {
                for (sid, sname) in &options {
                    ui.selectable_value(&mut chosen_id, sid.clone(), sname);
                }
                ui.separator();
                if ui.selectable_label(false, "Manage sessions…").clicked() {
                    open_manager = true;
                }
            });
        if icon_button(ui, palette, icon::FLOPPY_DISK, "Manage workspace sessions").clicked() {
            open_manager = true;
        }
        if chosen_id != active_id {
            self.switch_session(&chosen_id);
        }
        if open_manager {
            self.session_modal = Some(SessionModalState::new());
        }
    }

    fn render_connection_group(&mut self, ui: &mut egui::Ui, palette: Palette, layout: &TopBarLayout) {
        let add_label = if layout.show_add_server_label {
            format!("Server {}/{}", self.registry.count(), self.registry.max_servers())
        } else {
            format!("{}/{}", self.registry.count(), self.registry.max_servers())
        };
        if primary_button(ui, palette, icon::PLUS, &add_label, !self.registry.is_full())
            .on_hover_text("Add a server")
            .clicked()
        {
            self.modal = Some(ServerModalState::new_empty());
        }

        let theme_glyph = match self.theme {
            AppTheme::Light => icon::MOON,
            AppTheme::Dark => icon::SUN,
        };
        if icon_button(ui, palette, theme_glyph, "Switch theme").clicked() {
            self.toggle_theme(ui.ctx());
        }
        if icon_button(ui, palette, icon::GEAR_SIX, "Settings").clicked() {
            self.open_settings();
        }
        self.render_update_button(ui, palette);
        toolbar_divider(ui, palette);

        self.render_schema_picker(ui, palette, layout.schema_combo_w);
        self.render_server_picker(ui, palette, layout.server_combo_w);
        self.render_server_badges(ui, palette);
    }

    fn render_update_button(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let pill_label = match &self.update_modal.status {
            UpdateStatus::Available(info) => Some(format!("Update v{}", info.version)),
            UpdateStatus::Downloading { .. } | UpdateStatus::Applying(_) => Some("Updating…".to_string()),
            _ => None,
        };
        if let Some(label) = pill_label {
            let text = icon_text(icon::ARROW_CIRCLE_UP, palette.accent, &label, palette.accent, 12.0);
            let button = egui::Button::new(text)
                .fill(palette.accent.gamma_multiply(0.14))
                .stroke(Stroke::new(1.0, palette.accent.gamma_multiply(0.55)))
                .corner_radius(CornerRadius::same(255));
            if ui
                .add(button)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Open software update dialog")
                .clicked()
            {
                self.open_update_modal();
            }
        } else if icon_button(ui, palette, icon::ARROW_CIRCLE_UP, "Check for updates").clicked() {
            self.open_update_modal();
        }
    }

    fn render_server_picker(&mut self, ui: &mut egui::Ui, palette: Palette, combo_w: f32) {
        let selected_label = self
            .active_slot
            .and_then(|slot_id| {
                let slot = self.registry.get(slot_id)?;
                let display_num = self
                    .registry
                    .display_number_of(slot_id)
                    .unwrap_or_else(|| slot_id.display_number());
                Some(format!("#{display_num} {}", slot.profile.name))
            })
            .unwrap_or_else(|| "No server".into());
        let server_options: Vec<(ServerSlotId, String)> = self
            .registry
            .iter()
            .enumerate()
            .map(|(idx, slot)| (slot.slot_id, format!("#{} {}", idx + 1, slot.profile.name)))
            .collect();

        let mut chosen_slot = self.active_slot;
        egui::ComboBox::from_id_salt("active_server_combo")
            .truncate()
            .selected_text(icon_text(
                icon::DATABASE,
                palette.icon_schema,
                &selected_label,
                palette.text_primary,
                13.0,
            ))
            .width(combo_w)
            .show_ui(ui, |ui| {
                for (slot_id, label) in &server_options {
                    ui.selectable_value(&mut chosen_slot, Some(*slot_id), label);
                }
            });
        if chosen_slot != self.active_slot {
            self.active_slot = chosen_slot;
            self.mark_session_dirty();
        }
    }

    fn render_schema_picker(&mut self, ui: &mut egui::Ui, palette: Palette, combo_w: f32) {
        let Some(slot_id) = self.active_slot else {
            return;
        };
        let Some(current_schema) = self
            .registry
            .get(slot_id)
            .and_then(|slot| slot.runtime.as_ref())
            .map(|runtime| runtime.current_schema.clone().unwrap_or_else(|| NO_SCHEMA_LABEL.into()))
        else {
            return;
        };
        let schema_names: Vec<String> = self.catalogs[slot_id.index()]
            .as_ref()
            .map(|catalog| {
                catalog
                    .schemas()
                    .iter()
                    .filter(|schema| !schema.is_system || self.show_system_schemas)
                    .map(|schema| catalog.resolve(schema.name).to_string())
                    .collect()
            })
            .unwrap_or_default();

        let mut chosen_schema = current_schema.clone();
        let is_command_pending = self.slot_activity[slot_id.index()].is_session_command_pending;
        ui.add_enabled_ui(!is_command_pending, |ui| {
            egui::ComboBox::from_id_salt("active_schema_combo")
                .truncate()
                .selected_text(icon_text(
                    icon::STACK,
                    palette.icon_table,
                    &current_schema,
                    palette.text_primary,
                    13.0,
                ))
                .width(combo_w)
                .show_ui(ui, |ui| {
                    for schema_name in &schema_names {
                        ui.selectable_value(&mut chosen_schema, schema_name.clone(), schema_name);
                    }
                })
        });
        if chosen_schema != current_schema && chosen_schema != NO_SCHEMA_LABEL {
            self.send_session_command(slot_id, SlotCmd::SetSchema(chosen_schema));
        }
    }

    fn render_server_badges(&self, ui: &mut egui::Ui, palette: Palette) {
        let Some(slot) = self.active_slot.and_then(|slot_id| self.registry.get(slot_id)) else {
            return;
        };
        if slot.profile.read_only {
            pill(ui, "READ-ONLY", palette.warning);
        }
        let [red, green, blue] = slot.profile.env_display_rgb();
        pill(
            ui,
            slot.profile.env_display_label(),
            Color32::from_rgb(red, green, blue),
        );
        if slot.runtime.is_none() {
            ui.label(RichText::new("offline").size(11.5).color(palette.text_muted));
        }
    }
}

const MACOS_TITLEBAR_HEIGHT: f32 = 28.0;

fn window_titlebar_height(ctx: &egui::Context) -> f32 {
    let is_fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
    if cfg!(target_os = "macos") && !is_fullscreen {
        MACOS_TITLEBAR_HEIGHT
    } else {
        0.0
    }
}

fn render_titlebar_drag_region(ui: &mut egui::Ui, height: f32) {
    let (_, drag_response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click_and_drag());
    if drag_response.double_clicked() {
        let is_maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
    } else if drag_response.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}
