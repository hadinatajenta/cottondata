mod activity;
mod completion_popup;
mod data_grid;
mod er_diagram;
mod events;
mod grid_filter;
mod grid_table;
mod import_modal;
mod inspectors;
mod modals;
mod navigator;
mod params_modal;
mod query_actions;
mod query_tab;
mod quick_switcher;
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
mod update_modal;
mod workspace;

use crate::theme::{install_fonts, AppTheme};
use crate::worker::{SlotCmd, WorkerHub};
use activity::SlotActivity;
use cottondata_core::{ConnectionStatus, MemoryBudget, ServerRegistry, ServerSlotId, SqlDialect, SERVER_SLOT_CAPACITY};
use cottondata_driver::{ProcessEntry, ServerHealthMetrics};
use cottondata_grid::SortDirection;
use cottondata_introspect::{SchemaCatalog, TableDetail, TableKind};
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
use quick_switcher::{render_quick_switcher, QuickSwitcherItem, QuickSwitcherState, QuickSwitcherTarget};
use schema_compare::SchemaCompareState;
use server_form::ServerModalState;
use sessions::SessionModalState;
use settings_modal::SettingsModalState;
use snippets::SnippetModalState;
use structure_editor::StructureEditorState;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use update_modal::{render_update_modal, UpdateModalAction, UpdateModalState, UpdateStatus};

const MAX_TABLE_DETAILS_CACHE: usize = 64;

pub(super) struct TableDetailsCache {
    entries: HashMap<(usize, String, String), TableDetail>,
    order: VecDeque<(usize, String, String)>,
}

impl TableDetailsCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(super) fn get(&self, key: &(usize, String, String)) -> Option<&TableDetail> {
        self.entries.get(key)
    }

    pub(super) fn contains_key(&self, key: &(usize, String, String)) -> bool {
        self.entries.contains_key(key)
    }

    pub(super) fn insert(&mut self, key: (usize, String, String), detail: TableDetail) {
        if let Some(pos) = self.order.iter().position(|k| k == &key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, detail);
        while self.entries.len() > MAX_TABLE_DETAILS_CACHE {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }

    pub(super) fn remove(&mut self, key: &(usize, String, String)) {
        self.entries.remove(key);
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
    }

    pub(super) fn retain_slot_ne(&mut self, slot_idx: usize) {
        self.entries.retain(|(s, _, _), _| *s != slot_idx);
        self.order.retain(|(s, _, _)| *s != slot_idx);
    }
}

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
    ChangeTablePageSize(u32),
    SortColumnExplicit {
        col_idx: usize,
        col_name: String,
        direction: Option<SortDirection>,
    },
    RunQuery(bool),
    RunQueryUnlimited,
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
    pub(super) dragging_server: Option<ServerSlotId>,
    hub: WorkerHub,
    catalogs: [Option<Arc<SchemaCatalog>>; SERVER_SLOT_CAPACITY],
    table_details: TableDetailsCache,
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
    pub(super) update_modal: UpdateModalState,
    silent_update_check: bool,
    quick_switcher: QuickSwitcherState,
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
        let auto_check_updates = sessions_store.auto_check_updates;
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
            dragging_server: None,
            hub,
            catalogs: std::array::from_fn(|_| None),
            table_details: TableDetailsCache::new(),
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
            update_modal: UpdateModalState::default(),
            silent_update_check: false,
            quick_switcher: QuickSwitcherState::default(),
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
        if auto_check_updates {
            app.silent_update_check = true;
            app.hub.check_for_updates_delayed_async(Duration::from_secs(3));
        }
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
        let removed_id = removed.id;
        let saved_state = removed.to_session_state(removed.slot.and_then(|s| self.server_id_for_slot(s)));
        drop(removed);
        self.closed_tabs.push(saved_state);
        if self.closed_tabs.len() > 10 {
            self.closed_tabs.remove(0);
        }
        self.memory_budget.remove(&format!("tab-{removed_id}"));
        crate::purge_unused_memory();
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
        let num = self
            .registry
            .display_number_of(slot)
            .unwrap_or_else(|| slot.display_number());
        self.registry
            .get(slot)
            .map(|s| format!("#{num} {}", s.profile.name))
            .unwrap_or_else(|| format!("#{num}"))
    }

    pub(super) fn trigger_update_check(&mut self, open_modal: bool) {
        if open_modal {
            self.update_modal.open = true;
            self.silent_update_check = false;
        } else {
            self.silent_update_check = true;
        }
        if matches!(
            self.update_modal.status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. } | UpdateStatus::Applying(_)
        ) {
            return;
        }
        if open_modal
            || matches!(
                self.update_modal.status,
                UpdateStatus::Idle | UpdateStatus::UpToDate { .. } | UpdateStatus::Failed(_)
            )
        {
            self.update_modal.status = UpdateStatus::Checking;
        }
        self.hub.check_for_updates_async();
    }

    pub(super) fn open_update_modal(&mut self) {
        self.update_modal.open = true;
        if matches!(self.update_modal.status, UpdateStatus::Idle | UpdateStatus::Failed(_)) {
            self.trigger_update_check(true);
        }
    }

    pub(super) fn open_quick_switcher(&mut self) {
        self.quick_switcher.open_fresh();
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
        self.catalogs[slot.index()] = None;
        self.table_details.retain_slot_ne(slot.index());
        self.table_details_loading.retain(|(s, _, _)| *s != slot.index());
        self.processlists[slot.index()].clear();
        self.health[slot.index()] = None;
        self.hub.disconnect_slot(slot);
        crate::purge_unused_memory();
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
            || self.update_modal.open
            || self.quick_switcher.open
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
        let other_modal_open = self.is_any_modal_open() && !self.quick_switcher.open;
        if !other_modal_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K))
        {
            if self.quick_switcher.open {
                self.quick_switcher.close();
            } else {
                self.open_quick_switcher();
            }
            return;
        }

        let modal_open = self.is_any_modal_open();
        let is_editing_cell = self.tabs[self.active_tab_idx].inline_editor.is_some();
        let wants_kb = ctx.wants_keyboard_input();
        let mut run_query = false;
        let mut run_all_queries = false;
        let mut new_console = false;
        let mut reopen_tab = false;
        let mut close_active = false;
        let mut open_find: Option<bool> = None;
        let mut toggle_grid_filter = false;
        let mut refresh_active = false;
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
                if i.modifiers.shift {
                    toggle_grid_filter = true;
                } else {
                    open_find = Some(false);
                }
            }
            if !modal_open && !is_editing_cell && i.modifiers.command && i.key_pressed(egui::Key::H) {
                open_find = Some(true);
            }
            if !modal_open
                && !is_editing_cell
                && !wants_kb
                && i.modifiers.command
                && !i.modifiers.shift
                && i.key_pressed(egui::Key::R)
            {
                refresh_active = true;
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
                        if let Some(slot_id) = self.registry.slot_at_position(idx) {
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
        if toggle_grid_filter {
            let tab = &mut self.tabs[self.active_tab_idx];
            if tab.page.is_some() {
                tab.grid_filter.open = true;
                tab.grid_filter.focus_input = true;
                self.bottom_tab = BottomDeckTab::DataGrid;
            }
        } else if let Some(show_replace) = open_find {
            self.open_sql_find(show_replace);
        }
        if refresh_active {
            let tab = &self.tabs[self.active_tab_idx];
            if tab.bound_table.is_some() {
                let page_index = tab.filter_sort.page_index;
                if tab.mutator.has_changes() {
                    self.unsaved_guard_modal = Some(PendingDiscardAction::ReloadTablePage { page_index });
                } else {
                    self.send_reload_active_table_page();
                }
            } else if tab.page.is_some() {
                self.request_execute_active_tab(false);
            }
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

    fn build_quick_switcher_items(&self, palette: crate::theme::Palette) -> Vec<QuickSwitcherItem> {
        let mut items = Vec::new();
        let schema_rgb = [
            palette.icon_schema.r(),
            palette.icon_schema.g(),
            palette.icon_schema.b(),
        ];
        let table_rgb = [
            palette.icon_table.r(),
            palette.icon_table.g(),
            palette.icon_table.b(),
        ];
        let view_rgb = [
            palette.icon_view.r(),
            palette.icon_view.g(),
            palette.icon_view.b(),
        ];

        for (pos, slot) in self.registry.iter().enumerate() {
            let display_num = pos + 1;
            let status_hint = match &slot.status {
                ConnectionStatus::Connected => "Connected",
                ConnectionStatus::Connecting => "Connecting…",
                ConnectionStatus::Disconnected => "Disconnected",
                ConnectionStatus::Error(_) => "Error",
            };
            items.push(QuickSwitcherItem {
                title: format!("#{display_num} {}", slot.profile.name),
                subtitle: format!(
                    "{} · {} · {status_hint}",
                    slot.profile.engine.label(),
                    slot.profile.endpoint.display_address()
                ),
                badge: slot.profile.env_display_label().to_string(),
                badge_rgb: slot.profile.env_display_rgb(),
                target: QuickSwitcherTarget::Server(slot.slot_id),
            });

            if let Some(catalog) = &self.catalogs[slot.slot_id.index()] {
                for schema in catalog.schemas() {
                    if schema.is_system && !self.show_system_schemas {
                        continue;
                    }
                    let schema_name = catalog.resolve(schema.name);
                    items.push(QuickSwitcherItem {
                        title: schema_name.to_string(),
                        subtitle: format!("#{display_num} {}", slot.profile.name),
                        badge: "Schema".to_string(),
                        badge_rgb: schema_rgb,
                        target: QuickSwitcherTarget::Schema {
                            slot_id: slot.slot_id,
                            schema: schema_name.to_string(),
                        },
                    });
                    for table in &schema.tables {
                        let table_name = catalog.resolve(table.name);
                        let (badge, rgb) = match table.kind {
                            TableKind::BaseTable => ("Table", table_rgb),
                            TableKind::View => ("View", view_rgb),
                        };
                        items.push(QuickSwitcherItem {
                            title: table_name.to_string(),
                            subtitle: format!("#{display_num} {} · {schema_name}", slot.profile.name),
                            badge: badge.to_string(),
                            badge_rgb: rgb,
                            target: QuickSwitcherTarget::Table {
                                slot_id: slot.slot_id,
                                schema: schema_name.to_string(),
                                table: table_name.to_string(),
                            },
                        });
                    }
                }
            }
        }
        items
    }

    fn render_update_and_switcher_overlays(&mut self, ctx: &egui::Context, palette: crate::theme::Palette) {
        if self.update_modal.open {
            match render_update_modal(ctx, &mut self.update_modal, &palette) {
                UpdateModalAction::None => {}
                UpdateModalAction::CheckNow => {
                    self.trigger_update_check(true);
                }
                UpdateModalAction::StartDownload(info) => {
                    let info = *info;
                    self.update_modal.cancel_flag.store(false, Ordering::Relaxed);
                    self.update_modal.status = UpdateStatus::Downloading {
                        info: info.clone(),
                        downloaded: 0,
                        total: info.asset_size,
                    };
                    self.hub
                        .download_update_async(info, Arc::clone(&self.update_modal.cancel_flag));
                }
                UpdateModalAction::CancelDownload => {
                    self.update_modal.cancel_flag.store(true, Ordering::Relaxed);
                }
                UpdateModalAction::OpenUrl(url) => {
                    ctx.open_url(egui::OpenUrl::new_tab(url));
                }
                UpdateModalAction::CopyCommand(cmd) => {
                    ctx.copy_text(cmd);
                    self.notify_success("Copied install command to clipboard.");
                }
            }
        }

        if self.quick_switcher.open {
            let items = self.build_quick_switcher_items(palette);
            if let Some(target) = render_quick_switcher(ctx, &mut self.quick_switcher, &items, &palette) {
                match target {
                    QuickSwitcherTarget::Server(slot_id) => {
                        self.active_slot = Some(slot_id);
                        self.expanded_servers[slot_id.index()] = true;
                        if self
                            .registry
                            .get(slot_id)
                            .is_some_and(|s| matches!(s.status, ConnectionStatus::Disconnected | ConnectionStatus::Error(_)))
                        {
                            self.connect_slot(slot_id);
                        }
                        self.mark_session_dirty();
                    }
                    QuickSwitcherTarget::Schema { slot_id, schema } => {
                        self.active_slot = Some(slot_id);
                        self.expanded_servers[slot_id.index()] = true;
                        self.expanded_schemas
                            .insert((slot_id.index(), schema.clone()), true);
                        let needs_l1 = self.catalogs[slot_id.index()].as_ref().is_some_and(|cat| {
                            cat.schemas()
                                .iter()
                                .find(|s| cat.resolve(s.name).eq_ignore_ascii_case(&schema))
                                .is_some_and(|s| !s.l1_loaded)
                        });
                        if needs_l1 {
                            let activity = &mut self.slot_activity[slot_id.index()];
                            if !activity.loading_schemas.contains(&schema) {
                                activity.loading_schemas.insert(schema.clone());
                                self.hub.send(slot_id, SlotCmd::LoadSchemaL1 { schema });
                            }
                        }
                        self.mark_session_dirty();
                    }
                    QuickSwitcherTarget::Table {
                        slot_id,
                        schema,
                        table,
                    } => {
                        self.active_slot = Some(slot_id);
                        self.open_table_tab(slot_id, schema, table);
                    }
                }
            }
        }
    }
}

impl eframe::App for CottonDataApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            self.persist_sessions_now();
        }
        self.hub.set_egui_ctx(ctx.clone());
        self.process_worker_events(ctx);
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
        self.render_update_and_switcher_overlays(ctx, palette);
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

pub(super) fn insert_completion(sql: &mut String, cursor_byte: usize, completion: &str) -> usize {
    let clamped = cursor_byte.min(sql.len());
    let replace_start = completion_popup::word_prefix_start(&sql[..clamped]);
    sql.replace_range(replace_start..clamped, completion);
    replace_start + completion.len()
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

    #[test]
    fn table_details_cache_enforces_lru_cap_and_slot_purge() {
        let mut cache = TableDetailsCache::new();
        let make_detail = |ddl: &str| TableDetail {
            ddl: ddl.into(),
            engine: None,
            collation: None,
            row_estimate: None,
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            inbound_foreign_keys: Vec::new(),
        };
        for i in 0..(MAX_TABLE_DETAILS_CACHE + 5) {
            cache.insert((0, "app".into(), format!("t_{i}")), make_detail(&format!("ddl_{i}")));
        }
        assert_eq!(cache.entries.len(), MAX_TABLE_DETAILS_CACHE);
        assert!(!cache.contains_key(&(0, "app".into(), "t_0".into())));
        assert!(!cache.contains_key(&(0, "app".into(), "t_4".into())));
        assert!(cache.contains_key(&(0, "app".into(), "t_5".into())));

        cache.insert((1, "app".into(), "other".into()), make_detail("other"));
        cache.retain_slot_ne(0);
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.contains_key(&(1, "app".into(), "other".into())));
    }
}
