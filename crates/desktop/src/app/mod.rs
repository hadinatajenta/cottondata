mod activity;
mod data_grid;
mod events;
mod grid_table;
mod inspectors;
mod modals;
mod navigator;
mod query_actions;
mod query_tab;
mod result_table;
mod row_selection;
mod server_form;
mod servers_card;
mod sessions;
mod settings_modal;
mod sql_editor;
mod table_actions;
mod top_bar;
mod transaction_bar;
mod workspace;

use crate::theme::{install_fonts, AppTheme};
use crate::worker::{SlotCmd, WorkerHub};
use activity::SlotActivity;
use cottondata_core::{ConnectionStatus, MemoryBudget, ServerRegistry, ServerSlotId, SqlDialect, SERVER_SLOT_CAPACITY};
use cottondata_introspect::{SchemaCatalog, TableDetail};
use cottondata_driver::{ProcessEntry, ServerHealthMetrics};
use cottondata_storage::{AppStorage, ConnectedServerState, QueryHistoryItem, SessionsStore};
use eframe::egui;
use query_tab::{BottomDeckTab, QueryTab};
use server_form::ServerModalState;
use sessions::SessionModalState;
use settings_modal::SettingsModalState;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub struct CottonDataApp {
    theme: AppTheme,
    storage: AppStorage,
    registry: ServerRegistry,
    active_slot: Option<ServerSlotId>,
    hub: WorkerHub,
    catalogs: [Option<SchemaCatalog>; SERVER_SLOT_CAPACITY],
    processlists: [Vec<ProcessEntry>; SERVER_SLOT_CAPACITY],
    health: [Option<ServerHealthMetrics>; SERVER_SLOT_CAPACITY],
    ddl_view: Option<(String, String, TableDetail)>,
    tabs: Vec<QueryTab>,
    active_tab_idx: usize,
    next_tab_id: u64,
    bottom_tab: BottomDeckTab,
    explorer_filter: String,
    show_system_schemas: bool,
    memory_budget: MemoryBudget,
    history: Vec<QueryHistoryItem>,
    modal: Option<ServerModalState>,
    session_modal: Option<SessionModalState>,
    settings_modal: Option<SettingsModalState>,
    sql_preview_modal: Option<(u64, String, Vec<String>)>,
    toast_banner: Option<(String, bool)>,
    expanded_schemas: HashMap<(usize, String), bool>,
    sessions_store: SessionsStore,
    desired_connected_ids: HashSet<String>,
    pending_restore: [Option<ConnectedServerState>; SERVER_SLOT_CAPACITY],
    slot_activity: [SlotActivity; SERVER_SLOT_CAPACITY],
    session_dirty_since: Option<Instant>,
    last_ping_at: Instant,
}

impl CottonDataApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);

        let storage = AppStorage::open_default()
            .unwrap_or_else(|_| AppStorage::open_at(std::env::temp_dir().join("cottondata")).expect("temp storage"));
        let registry = storage.load_registry().unwrap_or_default();
        let sessions_store = storage.load_sessions();
        let theme = if sessions_store.dark_theme {
            AppTheme::Dark
        } else {
            AppTheme::Light
        };
        theme.apply(&cc.egui_ctx);
        let show_system_schemas = sessions_store.show_system_schemas;
        let active_slot = registry.iter().next().map(|s| s.slot_id);
        let history = storage.load_history();
        let mut hub = WorkerHub::new();
        hub.set_egui_ctx(cc.egui_ctx.clone());

        let mut app = Self {
            theme,
            storage,
            registry,
            active_slot,
            hub,
            catalogs: std::array::from_fn(|_| None),
            processlists: std::array::from_fn(|_| Vec::new()),
            health: std::array::from_fn(|_| None),
            ddl_view: None,
            tabs: vec![QueryTab::new_console(1, active_slot)],
            active_tab_idx: 0,
            next_tab_id: 2,
            bottom_tab: BottomDeckTab::DataGrid,
            explorer_filter: String::new(),
            show_system_schemas,
            memory_budget: MemoryBudget::default(),
            history,
            modal: None,
            session_modal: None,
            settings_modal: None,
            sql_preview_modal: None,
            toast_banner: None,
            expanded_schemas: HashMap::new(),
            sessions_store,
            desired_connected_ids: HashSet::new(),
            pending_restore: std::array::from_fn(|_| None),
            slot_activity: std::array::from_fn(|_| SlotActivity::default()),
            session_dirty_since: None,
            last_ping_at: Instant::now(),
        };
        app.restore_active_session(true);
        app
    }

    fn slot_dialect(&self, slot: Option<ServerSlotId>) -> SqlDialect {
        slot.and_then(|slot_id| self.registry.get(slot_id))
            .map(|slot| slot.profile.engine.dialect())
            .unwrap_or_default()
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.theme = self.theme.toggled();
        self.theme.apply(ctx);
        self.persist_sessions_now();
    }

    fn open_console_tab(&mut self) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(QueryTab::new_console(id, self.active_slot));
        self.active_tab_idx = self.tabs.len() - 1;
        self.persist_sessions_now();
    }

    fn activate_tab(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get(idx) else {
            return;
        };
        self.active_tab_idx = idx;
        if let Some(slot) = tab.slot {
            self.active_slot = Some(slot);
        }
        self.memory_budget.touch(&format!("tab-{}", tab.id));
        self.mark_session_dirty();
    }

    fn close_tab(&mut self, idx: usize) {
        if self.tabs.len() <= 1 || idx >= self.tabs.len() {
            return;
        }
        let removed = self.tabs.remove(idx);
        self.memory_budget.remove(&format!("tab-{}", removed.id));
        if idx < self.active_tab_idx || self.active_tab_idx >= self.tabs.len() {
            self.active_tab_idx = self.active_tab_idx.saturating_sub(1);
        }
        self.persist_sessions_now();
    }

    pub(super) fn connect_slot(&mut self, slot: ServerSlotId) {
        let is_busy_or_connected = self
            .registry
            .get(slot)
            .is_some_and(|s| matches!(s.status, ConnectionStatus::Connecting | ConnectionStatus::Connected));
        if is_busy_or_connected {
            return;
        }
        self.connect_slot_internal(slot, true);
    }

    pub(super) fn connect_slot_internal(&mut self, slot: ServerSlotId, select_slot: bool) {
        let Some(profile) = self.registry.get(slot).map(|s| s.profile.clone()) else {
            return;
        };
        let password = self.storage.get_password(&profile.id);
        self.desired_connected_ids.insert(profile.id.clone());
        if let Some(s) = self.registry.get_mut(slot) {
            s.status = ConnectionStatus::Connecting;
        }
        if select_slot {
            self.active_slot = Some(slot);
        }
        self.hub.send(slot, SlotCmd::Connect { profile, password });
        self.persist_sessions_now();
    }

    pub(super) fn disconnect_slot(&mut self, slot: ServerSlotId) {
        if let Some(profile_id) = self.server_id_for_slot(slot) {
            self.desired_connected_ids.remove(&profile_id);
        }
        self.pending_restore[slot.index()] = None;
        if let Some(s) = self.registry.get_mut(slot) {
            s.status = ConnectionStatus::Disconnected;
            s.runtime = None;
        }
        self.hub.send(slot, SlotCmd::Disconnect);
        self.persist_sessions_now();
    }

    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let keys = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
            egui::Key::Num0,
        ];
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(egui::Key::Enter) {
                self.execute_active_tab(false);
            }
            if i.modifiers.command {
                for (idx, key) in keys.iter().enumerate() {
                    if i.key_pressed(*key) {
                        if let Some(slot_id) = ServerSlotId::new(idx) {
                            if self.registry.get(slot_id).is_some() {
                                self.active_slot = Some(slot_id);
                                self.mark_session_dirty();
                            }
                        }
                    }
                }
            }
        });
    }
}

impl eframe::App for CottonDataApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.hub.set_egui_ctx(ctx.clone());
        self.process_worker_events();
        self.handle_shortcuts(ctx);
        self.flush_debounced_session(ctx);
        self.tick_keepalive(ctx);

        let palette = self.theme.palette();
        self.render_top_bar(ctx, palette);
        self.render_status_bar(ctx, palette);
        self.render_sidebar(ctx, palette);
        self.render_workspace(ctx, palette);
        self.render_modals(ctx, palette);
        self.render_settings_modal(ctx, palette);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.persist_sessions_now();
    }
}

pub(super) fn truncate_display(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

pub(super) fn insert_completion(sql: &mut String, cursor_byte: usize, completion: &str) {
    let clamped = cursor_byte.min(sql.len());
    let before = &sql[..clamped];
    let replace_start = before
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '`')))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    sql.replace_range(replace_start..clamped, completion);
}
