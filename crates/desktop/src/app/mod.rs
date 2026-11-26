mod activity;
mod data_grid;
mod er_diagram;
mod events;
mod grid_table;
mod import_modal;
mod inspectors;
mod modals;
mod navigator;
mod params_modal;
mod query_actions;
mod query_tab;
mod record_view;
mod result_table;
mod row_selection;
mod schema_compare;
mod server_form;
mod servers_card;
mod sessions;
mod settings_modal;
mod snippets;
mod sql_editor;
mod structure_editor;
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
use cottondata_sql::ParamBinding;
use cottondata_storage::{
    AppStorage, ConnectedServerState, QueryHistoryItem, SessionTabState, SessionsStore, SnippetScopeFilter,
    SnippetsStore,
};
use eframe::egui;
use er_diagram::ErDiagramState;
use import_modal::ImportModalState;
use params_modal::ParamModalState;
use query_tab::{BottomDeckTab, QueryTab};
use schema_compare::SchemaCompareState;
use server_form::ServerModalState;
use sessions::SessionModalState;
use settings_modal::SettingsModalState;
use snippets::SnippetModalState;
use structure_editor::StructureEditorState;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(super) struct ActiveStreamExport {
    pub export_id: u64,
    pub path_label: String,
    pub rows_written: u64,
    pub cancel_flag: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryStatusFilter {
    All,
    Succeeded,
    Failed,
}

#[derive(Clone)]
pub(super) enum PendingDiscardAction {
    CloseTab(usize),
    ReloadTablePage { page_index: u64 },
    SortColumn(String),
    RunQuery(bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

#[derive(Clone)]
pub(super) struct ToastBanner {
    pub message: String,
    pub kind: ToastKind,
    pub created_at: Instant,
    pub ttl: Duration,
}

impl ToastBanner {
    pub(super) fn new(kind: ToastKind, message: impl Into<String>) -> Self {
        let ttl = match kind {
            ToastKind::Success | ToastKind::Info => Duration::from_secs(4),
            ToastKind::Warning => Duration::from_secs(6),
            ToastKind::Error => Duration::from_secs(10),
        };
        Self {
            message: message.into(),
            kind,
            created_at: Instant::now(),
            ttl,
        }
    }

    pub(super) fn remaining(&self, now: Instant) -> Option<Duration> {
        self.ttl.checked_sub(now.saturating_duration_since(self.created_at))
    }
}

pub struct CottonDataApp {
    theme: AppTheme,
    storage: AppStorage,
    registry: ServerRegistry,
    active_slot: Option<ServerSlotId>,
    hub: WorkerHub,
    catalogs: [Option<SchemaCatalog>; SERVER_SLOT_CAPACITY],
    table_details: HashMap<(usize, String, String), TableDetail>,
    table_details_loading: HashSet<(usize, String, String)>,
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
    history_search: String,
    history_status_filter: HistoryStatusFilter,
    snippets_store: SnippetsStore,
    snippet_search: String,
    snippet_folder_filter: Option<String>,
    snippet_scope_filter: SnippetScopeFilter,
    snippet_modal: Option<SnippetModalState>,
    param_modal: Option<ParamModalState>,
    param_history: HashMap<u64, HashMap<String, ParamBinding>>,
    structure_editor: Option<StructureEditorState>,
    import_modal: Option<ImportModalState>,
    schema_compare: Option<SchemaCompareState>,
    er_diagram: Option<ErDiagramState>,
    modal: Option<ServerModalState>,
    session_modal: Option<SessionModalState>,
    settings_modal: Option<SettingsModalState>,
    sql_preview_modal: Option<(u64, String, Vec<String>)>,
    unsaved_guard_modal: Option<PendingDiscardAction>,
    after_save_action: Option<PendingDiscardAction>,
    editor_height: f32,
    toast_banner: Option<ToastBanner>,
    expanded_servers: [bool; SERVER_SLOT_CAPACITY],
    expanded_schemas: HashMap<(usize, String), bool>,
    sessions_store: SessionsStore,
    desired_connected_ids: HashSet<String>,
    pending_restore: [Option<ConnectedServerState>; SERVER_SLOT_CAPACITY],
    slot_activity: [SlotActivity; SERVER_SLOT_CAPACITY],
    session_dirty_since: Option<Instant>,
    last_ping_at: Instant,
    closed_tabs: Vec<SessionTabState>,
    test_conn_seq: u64,
    export_seq: u64,
    active_export: Option<ActiveStreamExport>,
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
        let snippets_store = storage.load_snippets();
        let mut hub = WorkerHub::new();
        hub.set_egui_ctx(cc.egui_ctx.clone());

        let mut app = Self {
            theme,
            storage,
            registry,
            active_slot,
            hub,
            catalogs: std::array::from_fn(|_| None),
            table_details: HashMap::new(),
            table_details_loading: HashSet::new(),
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
            history_search: String::new(),
            history_status_filter: HistoryStatusFilter::All,
            snippets_store,
            snippet_search: String::new(),
            snippet_folder_filter: None,
            snippet_scope_filter: SnippetScopeFilter::All,
            snippet_modal: None,
            param_modal: None,
            param_history: HashMap::new(),
            structure_editor: None,
            import_modal: None,
            schema_compare: None,
            er_diagram: None,
            modal: None,
            session_modal: None,
            settings_modal: None,
            sql_preview_modal: None,
            unsaved_guard_modal: None,
            after_save_action: None,
            editor_height: 220.0,
            toast_banner: None,
            expanded_servers: [false; SERVER_SLOT_CAPACITY],
            expanded_schemas: HashMap::new(),
            sessions_store,
            desired_connected_ids: HashSet::new(),
            pending_restore: std::array::from_fn(|_| None),
            slot_activity: std::array::from_fn(|_| SlotActivity::default()),
            session_dirty_since: None,
            last_ping_at: Instant::now(),
            closed_tabs: Vec::new(),
            test_conn_seq: 0,
            export_seq: 0,
            active_export: None,
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

    pub(super) fn open_console_tab(&mut self) {
        self.open_console_tab_for_slot(self.active_slot, None);
    }

    pub(super) fn open_console_tab_for_slot(&mut self, slot: Option<ServerSlotId>, initial_sql: Option<String>) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = QueryTab::new_console(id, slot);
        if let Some(sql) = initial_sql {
            tab.sql = sql;
        }
        if let Some(s) = slot {
            self.active_slot = Some(s);
        }
        self.tabs.push(tab);
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
        if self.tabs[idx].mutator.has_changes() {
            self.activate_tab(idx);
            self.unsaved_guard_modal = Some(PendingDiscardAction::CloseTab(idx));
            return;
        }
        self.force_close_tab(idx);
    }

    pub(super) fn force_close_tab(&mut self, idx: usize) {
        if self.tabs.len() <= 1 || idx >= self.tabs.len() {
            return;
        }
        let removed = self.tabs.remove(idx);
        let saved_state = removed.to_session_state(removed.slot.and_then(|s| self.server_id_for_slot(s)));
        self.closed_tabs.push(saved_state);
        if self.closed_tabs.len() > 10 {
            self.closed_tabs.remove(0);
        }
        self.memory_budget.remove(&format!("tab-{}", removed.id));
        if idx < self.active_tab_idx || self.active_tab_idx >= self.tabs.len() {
            self.active_tab_idx = self.active_tab_idx.saturating_sub(1);
        }
        self.persist_sessions_now();
    }

    pub(super) fn reopen_closed_tab(&mut self) {
        let Some(mut saved) = self.closed_tabs.pop() else {
            return;
        };
        let slot = saved
            .server_id
            .as_deref()
            .and_then(|id| self.slot_for_server_id(id))
            .or(self.active_slot);
        saved.id = self.next_tab_id;
        self.next_tab_id += 1;
        let title = saved.title.clone();
        let tab = QueryTab::from_session_state(&saved, slot);
        if let Some(s) = slot {
            self.active_slot = Some(s);
        }
        self.tabs.push(tab);
        self.active_tab_idx = self.tabs.len() - 1;
        self.persist_sessions_now();
        self.notify_info(format!("Reopened `{title}`"));
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

    pub(super) fn slot_display_name(&self, slot: ServerSlotId) -> String {
        self.registry
            .get(slot)
            .map(|s| format!("{slot} {}", s.profile.name))
            .unwrap_or_else(|| slot.to_string())
    }

    pub(super) fn notify_success(&mut self, message: impl Into<String>) {
        self.toast_banner = Some(ToastBanner::new(ToastKind::Success, message));
    }

    pub(super) fn notify_info(&mut self, message: impl Into<String>) {
        self.toast_banner = Some(ToastBanner::new(ToastKind::Info, message));
    }

    pub(super) fn notify_warning(&mut self, message: impl Into<String>) {
        self.toast_banner = Some(ToastBanner::new(ToastKind::Warning, message));
    }

    pub(super) fn notify_error(&mut self, message: impl Into<String>) {
        self.toast_banner = Some(ToastBanner::new(ToastKind::Error, message));
    }

    pub(super) fn clear_toast(&mut self) {
        self.toast_banner = None;
    }

    pub(super) fn clear_error_toast(&mut self) {
        if self
            .toast_banner
            .as_ref()
            .is_some_and(|toast| toast.kind == ToastKind::Error)
        {
            self.toast_banner = None;
        }
    }

    pub(super) fn connect_slot_internal(&mut self, slot: ServerSlotId, select_slot: bool) {
        let Some(profile) = self.registry.get(slot).map(|s| s.profile.clone()) else {
            return;
        };
        let label = format!("{slot} {}", profile.name);
        let password = self.storage.get_password(&profile.id);
        self.desired_connected_ids.insert(profile.id.clone());
        if let Some(s) = self.registry.get_mut(slot) {
            s.status = ConnectionStatus::Connecting;
        }
        if select_slot {
            self.active_slot = Some(slot);
            self.notify_info(format!("Connecting to {label}…"));
        }
        self.hub.send(slot, SlotCmd::Connect { profile, password });
        self.persist_sessions_now();
    }

    pub(super) fn disconnect_slot(&mut self, slot: ServerSlotId) {
        let label = self.slot_display_name(slot);
        if let Some(profile_id) = self.server_id_for_slot(slot) {
            self.desired_connected_ids.remove(&profile_id);
        }
        self.pending_restore[slot.index()] = None;
        if let Some(s) = self.registry.get_mut(slot) {
            s.status = ConnectionStatus::Disconnected;
            s.runtime = None;
        }
        self.hub.send(slot, SlotCmd::Disconnect);
        self.notify_info(format!("Disconnected from {label}"));
        self.persist_sessions_now();
    }

    pub(super) fn cached_table_detail(&self, slot: ServerSlotId, schema: &str, table: &str) -> Option<&TableDetail> {
        let key = (slot.index(), schema.to_ascii_lowercase(), table.to_ascii_lowercase());
        self.table_details.get(&key)
    }

    pub(super) fn ensure_table_detail_loaded(&mut self, slot: ServerSlotId, schema: &str, table: &str) {
        if schema.is_empty() || table.is_empty() {
            return;
        }
        let key = (slot.index(), schema.to_ascii_lowercase(), table.to_ascii_lowercase());
        if self.table_details.contains_key(&key) || self.table_details_loading.contains(&key) {
            return;
        }
        let is_connected = self
            .registry
            .get(slot)
            .is_some_and(|s| matches!(s.status, ConnectionStatus::Connected));
        if !is_connected {
            return;
        }
        self.table_details_loading.insert(key);
        self.hub.send(
            slot,
            SlotCmd::LoadTableL2 {
                schema: schema.to_string(),
                table: table.to_string(),
                show_in_inspector: false,
            },
        );
    }

    pub(super) fn is_any_modal_open(&self) -> bool {
        self.modal.is_some()
            || self.session_modal.is_some()
            || self.settings_modal.is_some()
            || self.snippet_modal.is_some()
            || self.param_modal.is_some()
            || self.structure_editor.is_some()
            || self.import_modal.is_some()
            || self.schema_compare.is_some()
            || self.er_diagram.is_some()
            || self.sql_preview_modal.is_some()
            || self.unsaved_guard_modal.is_some()
            || self.tabs[self.active_tab_idx].diff_rows.is_some()
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
        let modal_open = self.is_any_modal_open();
        let is_editing_cell = self.tabs[self.active_tab_idx].inline_editor.is_some();
        let mut run_query = false;
        let mut run_all_queries = false;
        let mut new_console = false;
        let mut reopen_tab = false;
        let mut close_active = false;
        let mut open_find: Option<bool> = None;
        let mut save_snippet = false;
        let mut chosen_slot = None;
        ctx.input(|i| {
            if !modal_open && !is_editing_cell && i.modifiers.command && i.key_pressed(egui::Key::Enter) {
                if i.modifiers.shift {
                    run_all_queries = true;
                } else {
                    run_query = true;
                }
            }
            if !modal_open && i.modifiers.command && i.key_pressed(egui::Key::T) {
                if i.modifiers.shift {
                    reopen_tab = true;
                } else {
                    new_console = true;
                }
            }
            if !modal_open && i.modifiers.command && i.key_pressed(egui::Key::W) {
                close_active = true;
            }
            if !modal_open && !is_editing_cell && i.modifiers.command && i.key_pressed(egui::Key::F) {
                open_find = Some(false);
            }
            if !modal_open && !is_editing_cell && i.modifiers.command && i.key_pressed(egui::Key::H) {
                open_find = Some(true);
            }
            if !modal_open
                && !is_editing_cell
                && i.modifiers.command
                && i.modifiers.shift
                && i.key_pressed(egui::Key::S)
            {
                save_snippet = true;
            }
            if !modal_open && i.modifiers.command {
                for (idx, key) in keys.iter().enumerate() {
                    if i.key_pressed(*key) {
                        if let Some(slot_id) = ServerSlotId::new(idx) {
                            if self.registry.get(slot_id).is_some() {
                                chosen_slot = Some(slot_id);
                            }
                        }
                    }
                }
            }
        });
        if run_all_queries {
            self.request_execute_active_tab(true);
        } else if run_query {
            self.request_execute_active_tab(false);
        }
        if let Some(show_replace) = open_find {
            self.open_sql_find(show_replace);
        }
        if save_snippet {
            let sql = self.tabs[self.active_tab_idx].sql.clone();
            self.open_save_snippet_modal(sql);
        }
        if reopen_tab {
            self.reopen_closed_tab();
        } else if new_console {
            self.open_console_tab();
        }
        if close_active {
            self.close_tab(self.active_tab_idx);
        }
        if let Some(slot_id) = chosen_slot {
            self.active_slot = Some(slot_id);
            self.mark_session_dirty();
        }
    }
}

impl eframe::App for CottonDataApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            self.persist_sessions_now();
        }
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

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.palette().bg_backdrop.to_normalized_gamma_f32()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_banner_expires_after_ttl_by_kind() {
        let toast = ToastBanner::new(ToastKind::Success, "Connected to #1 Local");
        assert_eq!(toast.ttl, Duration::from_secs(4));
        assert!(toast.remaining(toast.created_at + Duration::from_secs(1)).is_some());
        assert_eq!(toast.remaining(toast.created_at + Duration::from_secs(4)), Some(Duration::ZERO));
        assert_eq!(toast.remaining(toast.created_at + Duration::from_secs(5)), None);

        let err_toast = ToastBanner::new(ToastKind::Error, "Syntax error");
        assert_eq!(err_toast.ttl, Duration::from_secs(10));
        assert!(err_toast.remaining(err_toast.created_at + Duration::from_secs(9)).is_some());
        assert_eq!(err_toast.remaining(err_toast.created_at + Duration::from_secs(11)), None);
    }
}
