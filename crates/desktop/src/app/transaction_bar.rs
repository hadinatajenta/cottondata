use super::CottonDataApp;
use crate::theme::Palette;
use crate::ui_kit::{busy_indicator, ghost_button, icon, tinted_ghost_button};
use crate::worker::SlotCmd;
use cottondata_core::ServerSlotId;
use eframe::egui;

impl CottonDataApp {
    pub(super) fn render_transaction_group(&mut self, ui: &mut egui::Ui, palette: Palette, show_labels: bool) {
        let active = self
            .active_slot
            .and_then(|slot_id| self.registry.get(slot_id))
            .and_then(|slot| {
                slot.runtime
                    .as_ref()
                    .map(|runtime| (slot.slot_id, runtime.autocommit, runtime.in_transaction))
            });
        let is_command_pending =
            active.is_some_and(|(slot_id, _, _)| self.slot_activity[slot_id.index()].is_session_command_pending);
        let has_runtime = active.is_some() && !is_command_pending;
        let (is_autocommit, is_in_transaction) = active.map(|(_, auto, in_tx)| (auto, in_tx)).unwrap_or((true, false));
        let can_control_transaction = has_runtime && (!is_autocommit || is_in_transaction);
        let label_for = |text: &'static str| if show_labels { text } else { "" };

        let autocommit_label = if is_autocommit { "Auto-commit" } else { "Manual commit" };
        let autocommit_glyph = if is_autocommit {
            icon::LIGHTNING
        } else {
            icon::HAND_PALM
        };
        let toggle_clicked = ghost_button(ui, palette, autocommit_glyph, label_for(autocommit_label), has_runtime)
            .on_hover_text("Toggle autocommit for this session")
            .clicked();
        let begin_clicked = ghost_button(
            ui,
            palette,
            icon::FLOW_ARROW,
            label_for("Begin"),
            can_control_transaction,
        )
        .on_hover_text("BEGIN")
        .clicked();
        let commit_clicked = tinted_ghost_button(
            ui,
            palette.success,
            icon::CHECK_CIRCLE,
            label_for("Commit"),
            can_control_transaction,
        )
        .on_hover_text("COMMIT")
        .clicked();
        let rollback_clicked = tinted_ghost_button(
            ui,
            palette.danger,
            icon::ARROW_COUNTER_CLOCKWISE,
            label_for("Rollback"),
            can_control_transaction,
        )
        .on_hover_text("ROLLBACK")
        .clicked();
        if is_command_pending {
            busy_indicator(ui, palette, "");
        }

        let Some((slot_id, _, _)) = active else {
            return;
        };
        if toggle_clicked {
            self.send_session_command(slot_id, SlotCmd::SetAutocommit(!is_autocommit));
        }
        if begin_clicked {
            self.send_session_command(slot_id, SlotCmd::BeginTx);
        }
        if commit_clicked {
            self.send_session_command(slot_id, SlotCmd::CommitTx);
        }
        if rollback_clicked {
            self.send_session_command(slot_id, SlotCmd::RollbackTx);
        }
    }

    pub(super) fn send_session_command(&mut self, slot_id: ServerSlotId, command: SlotCmd) {
        self.slot_activity[slot_id.index()].is_session_command_pending = true;
        self.hub.send(slot_id, command);
    }
}
