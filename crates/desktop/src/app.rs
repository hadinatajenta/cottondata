use crate::theme::{highlight_mysql_layout, GlassTheme, Palette};
use crate::worker::{SlotCmd, WorkerEvent, WorkerHub};
use cottondata_core::{
    ConnectionEndpoint, ConnectionStatus, EnvironmentTag, MemoryBudget, ServerProfile,
    ServerRegistry, ServerSlotId, SshTunnelConfig, TlsMode, MAX_SERVERS,
};
use cottondata_grid::{
    export_csv, export_json_array, export_json_lines, export_sql_inserts, CellValue, ColumnarPage,
    ExportFormat, PageFilterSort, SortDirection, StagedMutator,
};
use cottondata_introspect::{SchemaCatalog, TableDetail, TableKind};
use cottondata_mysql::{ExplainResult, ProcessEntry, QueryOutcome, ServerHealthMetrics};
use cottondata_sql::{
    analyze_statement, format_mysql, split_statements, statement_at_offset, CompletionProvider,
    WriteClass,
};
use cottondata_storage::{AppStorage, QueryHistoryItem};
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, RichText, Stroke};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomDeckTab {
    DataGrid,
    DdlInspector,
    ExplainPlan,
    Monitor,
    History,
}

#[derive(Clone)]
struct QueryTab {
    id: u64,
    title: String,
    slot: Option<ServerSlotId>,
    sql: String,
    cursor_byte: usize,
    bound_table: Option<(String, String)>,
    filter_sort: PageFilterSort,
    page: Option<ColumnarPage>,
    last_status_msg: String,
    is_running: bool,
    mutator: StagedMutator,
    selected_cell: Option<(usize, usize)>,
    cell_edit_buf: String,
    cell_edit_is_null: bool,
    explain: Option<ExplainResult>,
}

impl QueryTab {
    fn new_console(id: u64, slot: Option<ServerSlotId>) -> Self {
        Self {
            id,
            title: format!("Console {id}"),
            slot,
            sql: "SELECT VERSION() AS mysql_version, DATABASE() AS current_db, NOW() AS server_time;"
                .to_string(),
            cursor_byte: 0,
            bound_table: None,
            filter_sort: PageFilterSort {
                page_size: 500,
                ..Default::default()
            },
            page: None,
            last_status_msg: "Ready — press Cmd/Ctrl+Enter to run current statement".into(),
            is_running: false,
            mutator: StagedMutator::new(),
            selected_cell: None,
            cell_edit_buf: String::new(),
            cell_edit_is_null: false,
            explain: None,
        }
    }

    fn new_table(id: u64, slot: ServerSlotId, schema: String, table: String, page_size: u32) -> Self {
        let filter_sort = PageFilterSort {
            page_size,
            ..Default::default()
        };
        let sql = filter_sort.build_select_sql(&schema, &table);
        Self {
            id,
            title: format!("{schema}.{table}"),
            slot: Some(slot),
            sql,
            cursor_byte: 0,
            bound_table: Some((schema, table)),
            filter_sort,
            page: None,
            last_status_msg: "Loading table page…".into(),
            is_running: true,
            mutator: StagedMutator::new(),
            selected_cell: None,
            cell_edit_buf: String::new(),
            cell_edit_is_null: false,
            explain: None,
        }
    }
}

struct ServerModalState {
    editing_slot: Option<ServerSlotId>,
    name: String,
    use_unix_socket: bool,
    host: String,
    port: String,
    socket_path: String,
    username: String,
    password: String,
    save_in_keychain: bool,
    default_schema: String,
    tls: TlsMode,
    ca_cert_path: String,
    ssh_enabled: bool,
    ssh_host: String,
    ssh_port: String,
    ssh_user: String,
    ssh_key: String,
    environment: EnvironmentTag,
    read_only: bool,
    page_size: u32,
    error_msg: Option<String>,
}

impl ServerModalState {
    fn new_empty() -> Self {
        Self {
            editing_slot: None,
            name: String::new(),
            use_unix_socket: false,
            host: "127.0.0.1".into(),
            port: "3306".into(),
            socket_path: "/tmp/mysql.sock".into(),
            username: "root".into(),
            password: String::new(),
            save_in_keychain: cfg!(target_os = "macos"),
            default_schema: String::new(),
            tls: TlsMode::Preferred,
            ca_cert_path: String::new(),
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: "22".into(),
            ssh_user: String::new(),
            ssh_key: String::new(),
            environment: EnvironmentTag::Local,
            read_only: false,
            page_size: 500,
            error_msg: None,
        }
    }

    fn from_existing(slot: ServerSlotId, p: &ServerProfile, pw: Option<String>) -> Self {
        let (use_unix_socket, host, port, socket_path) = match &p.endpoint {
            ConnectionEndpoint::Tcp { host, port } => {
                (false, host.clone(), port.to_string(), "/tmp/mysql.sock".into())
            }
            ConnectionEndpoint::UnixSocket { path } => {
                (true, "127.0.0.1".into(), "3306".into(), path.clone())
            }
        };
        Self {
            editing_slot: Some(slot),
            name: p.name.clone(),
            use_unix_socket,
            host,
            port,
            socket_path,
            username: p.username.clone(),
            password: pw.unwrap_or_default(),
            save_in_keychain: cfg!(target_os = "macos"),
            default_schema: p.default_schema.clone().unwrap_or_default(),
            tls: p.tls,
            ca_cert_path: p.ca_cert_path.clone().unwrap_or_default(),
            ssh_enabled: p.ssh.enabled,
            ssh_host: p.ssh.host.clone(),
            ssh_port: p.ssh.port.max(22).to_string(),
            ssh_user: p.ssh.username.clone(),
            ssh_key: p.ssh.identity_file.clone().unwrap_or_default(),
            environment: p.environment,
            read_only: p.read_only,
            page_size: p.page_size,
            error_msg: None,
        }
    }

    fn build_profile(&self, existing_id: Option<String>) -> Result<ServerProfile, String> {
        let endpoint = if self.use_unix_socket {
            ConnectionEndpoint::UnixSocket {
                path: self.socket_path.trim().to_string(),
            }
        } else {
            let port = self
                .port
                .trim()
                .parse::<u16>()
                .map_err(|_| "Port must be a number between 1 and 65535".to_string())?;
            ConnectionEndpoint::Tcp {
                host: self.host.trim().to_string(),
                port,
            }
        };
        let ssh_port = self.ssh_port.trim().parse::<u16>().unwrap_or(22);
        let id = existing_id.unwrap_or_else(|| {
            let slug: String = self
                .name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
                .collect();
            format!("srv-{slug}-{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() % 100_000)
                .unwrap_or(1))
        });

        let profile = ServerProfile {
            id,
            name: self.name.trim().to_string(),
            endpoint,
            username: self.username.trim().to_string(),
            default_schema: {
                let s = self.default_schema.trim();
                if s.is_empty() { None } else { Some(s.to_string()) }
            },
            tls: self.tls,
            ca_cert_path: {
                let s = self.ca_cert_path.trim();
                if s.is_empty() { None } else { Some(s.to_string()) }
            },
            ssh: SshTunnelConfig {
                enabled: self.ssh_enabled,
                host: self.ssh_host.trim().to_string(),
                port: ssh_port,
                username: self.ssh_user.trim().to_string(),
                identity_file: {
                    let s = self.ssh_key.trim();
                    if s.is_empty() { None } else { Some(s.to_string()) }
                },
                local_bind_port: None,
            },
            environment: self.environment,
            read_only: self.read_only,
            page_size: self.page_size,
            connect_timeout_secs: 10,
            idle_disconnect_mins: 15,
            show_system_schemas: false,
        };
        profile.validate().map_err(|e| e.to_string())?;
        Ok(profile)
    }
}

pub struct CottonDataApp {
    theme: GlassTheme,
    storage: AppStorage,
    registry: ServerRegistry,
    active_slot: Option<ServerSlotId>,
    hub: WorkerHub,
    catalogs: [Option<SchemaCatalog>; MAX_SERVERS],
    processlists: [Vec<ProcessEntry>; MAX_SERVERS],
    health: [Option<ServerHealthMetrics>; MAX_SERVERS],
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
    sql_preview_modal: Option<(u64, String, Vec<String>)>,
    toast_banner: Option<(String, bool)>,
    expanded_schemas: HashMap<(usize, String), bool>,
}

impl CottonDataApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let theme = GlassTheme::DarkGlass;
        theme.apply(&cc.egui_ctx);

        let storage = AppStorage::open_default().unwrap_or_else(|_| {
            AppStorage::open_at(std::env::temp_dir().join("cottondata")).expect("temp storage")
        });
        let registry = storage.load_registry().unwrap_or_default();
        let active_slot = registry.iter().next().map(|s| s.slot_id);
        let history = storage.load_history();
        let mut hub = WorkerHub::new();
        hub.set_egui_ctx(cc.egui_ctx.clone());

        Self {
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
            show_system_schemas: false,
            memory_budget: MemoryBudget::default(),
            history,
            modal: None,
            sql_preview_modal: None,
            toast_banner: None,
            expanded_schemas: HashMap::new(),
        }
    }

    fn process_worker_events(&mut self) {
        let events = self.hub.poll_events();
        for ev in events {
            match ev {
                WorkerEvent::Connected {
                    slot,
                    runtime,
                    catalog,
                } => {
                    if let Some(s) = self.registry.get_mut(slot) {
                        s.status = ConnectionStatus::Connected;
                        s.runtime = Some(runtime);
                    }
                    self.catalogs[slot.index()] = Some(catalog);
                    self.toast_banner = Some((format!("Connected to slot {slot}"), false));
                }
                WorkerEvent::Disconnected { slot } => {
                    if let Some(s) = self.registry.get_mut(slot) {
                        s.status = ConnectionStatus::Disconnected;
                        s.runtime = None;
                    }
                    self.catalogs[slot.index()] = None;
                    self.processlists[slot.index()].clear();
                    self.health[slot.index()] = None;
                    self.toast_banner = Some((format!("Disconnected slot {slot}"), false));
                }
                WorkerEvent::Error { slot, tab_id, error } => {
                    let msg = error.to_string();
                    if tab_id.is_none() {
                        if let Some(s) = self.registry.get_mut(slot) {
                            if matches!(s.status, ConnectionStatus::Connecting) {
                                s.status = ConnectionStatus::Error(msg.clone());
                            }
                        }
                    }
                    if let Some(tid) = tab_id {
                        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tid) {
                            tab.is_running = false;
                            tab.last_status_msg = format!("Error: {msg}");
                        }
                    }
                    self.toast_banner = Some((msg, true));
                }
                WorkerEvent::CatalogUpdated { slot, catalog } => {
                    self.catalogs[slot.index()] = Some(catalog);
                }
                WorkerEvent::TableDetailLoaded {
                    schema,
                    table,
                    detail,
                } => {
                    self.ddl_view = Some((schema, table, detail));
                    self.bottom_tab = BottomDeckTab::DdlInspector;
                }
                WorkerEvent::QueryFinished {
                    slot,
                    tab_id,
                    outcome,
                    runtime,
                } => {
                    let srv_name = self
                        .registry
                        .get(slot)
                        .map(|s| s.profile.name.clone())
                        .unwrap_or_else(|| slot.to_string());
                    let cur_schema = runtime.current_schema.clone();
                    if let Some(s) = self.registry.get_mut(slot) {
                        s.runtime = Some(runtime);
                    }
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.is_running = false;
                        tab.mutator.revert_all();
                        tab.selected_cell = None;
                        match outcome {
                            QueryOutcome::Rows(page) => {
                                let bytes = page.approx_bytes();
                                let more = if page.has_more_rows { "+" } else { "" };
                                tab.last_status_msg = format!(
                                    "{}{} rows · {} cols · {} ms · {:.1} KiB",
                                    page.row_count,
                                    more,
                                    page.columns.len(),
                                    page.elapsed_ms,
                                    bytes as f64 / 1024.0
                                );
                                self.history = self.storage.record_history(
                                    srv_name,
                                    cur_schema,
                                    page.executed_sql.clone(),
                                    page.elapsed_ms,
                                    true,
                                );
                                let evicted = self
                                    .memory_budget
                                    .record_allocation(format!("tab-{}", tab.id), bytes);
                                tab.page = Some(page);
                                self.evict_pages(&evicted);
                            }
                            QueryOutcome::Affected {
                                affected_rows,
                                last_insert_id,
                                warnings,
                                elapsed_ms,
                                executed_sql,
                            } => {
                                let lid = last_insert_id
                                    .filter(|&v| v > 0)
                                    .map(|v| format!(" · last_insert_id={v}"))
                                    .unwrap_or_default();
                                tab.last_status_msg = format!(
                                    "OK · {affected_rows} rows affected{lid} · {warnings} warnings · {elapsed_ms} ms"
                                );
                                self.history = self.storage.record_history(
                                    srv_name,
                                    cur_schema,
                                    executed_sql,
                                    elapsed_ms,
                                    true,
                                );
                            }
                        }
                    }
                    self.bottom_tab = BottomDeckTab::DataGrid;
                }
                WorkerEvent::RowCountLoaded {
                    tab_id,
                    total_rows,
                } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        if let Some(page) = tab.page.as_mut() {
                            page.total_rows_known = Some(total_rows);
                        }
                    }
                }
                WorkerEvent::MutationsSubmitted {
                    slot,
                    tab_id,
                    statement_count,
                    elapsed_ms,
                } => {
                    self.toast_banner = Some((
                        format!("Committed {statement_count} SQL mutations in {elapsed_ms} ms"),
                        false,
                    ));
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.mutator.revert_all();
                        if let Some((schema, table)) = tab.bound_table.clone() {
                            tab.is_running = true;
                            self.hub.send(
                                slot,
                                SlotCmd::LoadTablePage {
                                    tab_id,
                                    schema,
                                    table,
                                    filter_sort: tab.filter_sort.clone(),
                                },
                            );
                        }
                    }
                }
                WorkerEvent::ExplainFinished {
                    tab_id,
                    result,
                } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.is_running = false;
                        tab.explain = Some(result);
                    }
                    self.bottom_tab = BottomDeckTab::ExplainPlan;
                }
                WorkerEvent::RuntimeUpdated { slot, runtime } => {
                    if let Some(s) = self.registry.get_mut(slot) {
                        s.runtime = Some(runtime);
                    }
                }
                WorkerEvent::ProcesslistLoaded { slot, entries } => {
                    self.processlists[slot.index()] = entries;
                }
                WorkerEvent::HealthLoaded { slot, metrics } => {
                    self.health[slot.index()] = Some(metrics);
                }
            }
        }
    }

    fn evict_pages(&mut self, keys: &[String]) {
        for key in keys {
            if let Some(id_str) = key.strip_prefix("tab-") {
                if let Ok(tid) = id_str.parse::<u64>() {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tid) {
                        tab.page = None;
                        tab.last_status_msg =
                            "Page evicted by 32 MiB RAM budget — click Run or Reload to re-fetch".into();
                    }
                }
            }
        }
    }

    fn connect_slot(&mut self, slot: ServerSlotId) {
        let Some(profile) = self.registry.get(slot).map(|s| s.profile.clone()) else {
            return;
        };
        let password = self.storage.get_password(&profile.id);
        if let Some(s) = self.registry.get_mut(slot) {
            s.status = ConnectionStatus::Connecting;
        }
        self.active_slot = Some(slot);
        self.hub.send(slot, SlotCmd::Connect { profile, password });
    }

    fn disconnect_slot(&mut self, slot: ServerSlotId) {
        self.hub.send(slot, SlotCmd::Disconnect);
    }

    fn open_table_tab(&mut self, slot: ServerSlotId, schema: String, table: String) {
        let page_size = self
            .registry
            .get(slot)
            .map(|s| s.profile.page_size)
            .unwrap_or(500);
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let tab = QueryTab::new_table(id, slot, schema.clone(), table.clone(), page_size);
        let filter_sort = tab.filter_sort.clone();
        self.tabs.push(tab);
        self.active_tab_idx = self.tabs.len() - 1;
        self.hub.send(
            slot,
            SlotCmd::LoadTablePage {
                tab_id: id,
                schema,
                table,
                filter_sort,
            },
        );
    }

    fn execute_active_tab(&mut self, run_all: bool) {
        let Some(tab) = self.tabs.get_mut(self.active_tab_idx) else {
            return;
        };
        let slot = tab.slot.or(self.active_slot);
        let Some(slot) = slot else {
            self.toast_banner = Some(("Add and connect a MySQL server first.".into(), true));
            return;
        };
        tab.slot = Some(slot);
        let max_rows = self
            .registry
            .get(slot)
            .map(|s| s.profile.page_size)
            .unwrap_or(500);

        let sql_to_run = if run_all {
            let stmts = split_statements(&tab.sql);
            stmts
                .last()
                .map(|s| s.sql.to_string())
                .unwrap_or_else(|| tab.sql.trim().to_string())
        } else {
            statement_at_offset(&tab.sql, tab.cursor_byte)
                .map(|s| s.sql.to_string())
                .unwrap_or_else(|| tab.sql.trim().to_string())
        };

        if sql_to_run.trim().is_empty() {
            return;
        }
        tab.is_running = true;
        tab.last_status_msg = "Executing query…".into();
        let tab_id = tab.id;
        self.hub.send(
            slot,
            SlotCmd::ExecuteSql {
                tab_id,
                sql: sql_to_run,
                max_rows,
            },
        );
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
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
                            }
                        }
                    }
                }
            }
        });
    }

    fn render_top_bar(&mut self, ctx: &egui::Context, p: Palette) {
        egui::TopBottomPanel::top("liquid_glass_header")
            .frame(
                egui::Frame::new()
                    .fill(p.bg_panel)
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .stroke(Stroke::new(1.0, p.glass_border)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("CottonData")
                            .font(FontId::new(15.5, FontFamily::Proportional))
                            .strong()
                            .color(p.text_primary),
                    );
                    ui.label(
                        RichText::new("MySQL · Featherweight")
                            .size(11.5)
                            .color(p.accent),
                    );
                    ui.separator();

                    if let Some(slot_id) = self.active_slot {
                        if let Some(slot) = self.registry.get(slot_id) {
                            let [r, g, b] = slot.profile.environment.rgb();
                            let env_col = Color32::from_rgb(r, g, b);
                            egui::Frame::new()
                                .fill(Color32::from_rgba_unmultiplied(r, g, b, 34))
                                .stroke(Stroke::new(1.0, env_col))
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "{} {} · {}",
                                            slot.slot_id,
                                            slot.profile.name,
                                            slot.profile.environment.label()
                                        ))
                                        .size(11.5)
                                        .strong()
                                        .color(env_col),
                                    );
                                });

                            if slot.profile.read_only {
                                ui.label(
                                    RichText::new("READ-ONLY")
                                        .size(11.0)
                                        .strong()
                                        .color(p.warning),
                                );
                            }

                            if let Some(rt) = &slot.runtime {
                                ui.label(
                                    RichText::new(format!(
                                        "MySQL {} · #{}",
                                        rt.version, rt.connection_id
                                    ))
                                    .size(11.5)
                                    .color(p.text_muted),
                                );

                                let current_schema = rt
                                    .current_schema
                                    .clone()
                                    .unwrap_or_else(|| "(no schema)".into());
                                let schema_names: Vec<String> = self.catalogs[slot_id.index()]
                                    .as_ref()
                                    .map(|cat| {
                                        cat.schemas()
                                            .iter()
                                            .filter(|s| !s.is_system || self.show_system_schemas)
                                            .map(|s| cat.resolve(s.name).to_string())
                                            .collect()
                                    })
                                    .unwrap_or_default();

                                let mut chosen = current_schema.clone();
                                egui::ComboBox::from_id_salt("active_schema_combo")
                                    .selected_text(&current_schema)
                                    .width(135.0)
                                    .show_ui(ui, |ui| {
                                        for sname in &schema_names {
                                            ui.selectable_value(&mut chosen, sname.clone(), sname);
                                        }
                                    });
                                if chosen != current_schema && chosen != "(no schema)" {
                                    self.hub.send(slot_id, SlotCmd::SetSchema(chosen));
                                }

                                let tx_label = if rt.autocommit { "Tx: Auto" } else { "Tx: Manual" };
                                if ui.small_button(tx_label).clicked() {
                                    self.hub
                                        .send(slot_id, SlotCmd::SetAutocommit(!rt.autocommit));
                                }
                                if !rt.autocommit || rt.in_transaction {
                                    if ui.small_button("BEGIN").clicked() {
                                        self.hub.send(slot_id, SlotCmd::BeginTx);
                                    }
                                    if ui.small_button("COMMIT").clicked() {
                                        self.hub.send(slot_id, SlotCmd::CommitTx);
                                    }
                                    if ui.small_button("ROLLBACK").clicked() {
                                        self.hub.send(slot_id, SlotCmd::RollbackTx);
                                    }
                                }
                            }
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let theme_label = match self.theme {
                            GlassTheme::DarkGlass => "Light Glass",
                            GlassTheme::LightGlass => "Dark Glass",
                        };
                        if ui.small_button(theme_label).clicked() {
                            self.theme = match self.theme {
                                GlassTheme::DarkGlass => GlassTheme::LightGlass,
                                GlassTheme::LightGlass => GlassTheme::DarkGlass,
                            };
                            self.theme.apply(ctx);
                        }

                        let add_enabled = !self.registry.is_full();
                        let add_label = format!("+ Server ({}/{})", self.registry.count(), MAX_SERVERS);
                        if ui
                            .add_enabled(add_enabled, egui::Button::new(add_label))
                            .clicked()
                        {
                            self.modal = Some(ServerModalState::new_empty());
                        }

                        let stats = self.memory_budget.stats();
                        ui.label(
                            RichText::new(format!(
                                "RAM Budget: {:.2} / {} MiB",
                                stats.used_bytes as f64 / (1024.0 * 1024.0),
                                stats.limit_bytes / (1024 * 1024)
                            ))
                            .size(11.0)
                            .color(p.text_muted),
                        );
                    });
                });
            });
    }

    fn render_sidebar(&mut self, ctx: &egui::Context, p: Palette) {
        egui::SidePanel::left("servers_and_explorer")
            .resizable(true)
            .default_width(290.0)
            .min_width(230.0)
            .max_width(460.0)
            .frame(
                egui::Frame::new()
                    .fill(p.bg_panel)
                    .inner_margin(egui::Margin::same(10))
                    .stroke(Stroke::new(1.0, p.glass_border)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "MYSQL SERVERS ({}/{})",
                            self.registry.count(),
                            MAX_SERVERS
                        ))
                        .size(11.0)
                        .strong()
                        .color(p.text_muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("Cmd+1..0").size(10.0).color(p.text_muted));
                    });
                });
                ui.add_space(4.0);

                let slots_snapshot: Vec<_> = self.registry.iter().cloned().collect();
                let mut action_connect = None;
                let mut action_disconnect = None;
                let mut action_edit = None;
                let mut action_delete = None;

                if slots_snapshot.is_empty() {
                    egui::Frame::new()
                        .fill(p.bg_surface)
                        .stroke(Stroke::new(1.0, p.glass_border))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("No MySQL servers yet.")
                                    .strong()
                                    .color(p.text_primary),
                            );
                            ui.label(
                                RichText::new(
                                    "Add up to 10 MySQL servers with zero-JVM memory overhead.",
                                )
                                .size(11.5)
                                .color(p.text_muted),
                            );
                            ui.add_space(6.0);
                            if ui.button("+ Add First MySQL Server").clicked() {
                                self.modal = Some(ServerModalState::new_empty());
                            }
                        });
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("server_slots_scroll")
                        .max_height(240.0)
                        .show(ui, |ui| {
                            for slot in &slots_snapshot {
                                let selected = self.active_slot == Some(slot.slot_id);
                                let bg = if selected { p.bg_elevated } else { p.bg_surface };
                                let border = if selected { p.accent } else { p.glass_border };
                                let [r, g, b] = slot.profile.environment.rgb();
                                let env_color = Color32::from_rgb(r, g, b);

                                egui::Frame::new()
                                    .fill(bg)
                                    .stroke(Stroke::new(1.0, border))
                                    .corner_radius(CornerRadius::same(9))
                                    .inner_margin(egui::Margin::symmetric(8, 6))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let dot_color = match &slot.status {
                                                ConnectionStatus::Connected => p.success,
                                                ConnectionStatus::Connecting => p.warning,
                                                ConnectionStatus::Error(_) => p.danger,
                                                ConnectionStatus::Disconnected => p.text_muted,
                                            };
                                            ui.label(RichText::new("●").color(dot_color).size(10.0));
                                            if ui
                                                .selectable_label(
                                                    selected,
                                                    RichText::new(format!(
                                                        "{} {}",
                                                        slot.slot_id, slot.profile.name
                                                    ))
                                                    .strong(),
                                                )
                                                .clicked()
                                            {
                                                self.active_slot = Some(slot.slot_id);
                                            }
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(slot.profile.environment.label())
                                                            .size(9.5)
                                                            .strong()
                                                            .color(env_color),
                                                    );
                                                },
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{}@{}",
                                                    slot.profile.username,
                                                    slot.profile.endpoint.display_address()
                                                ))
                                                .size(10.5)
                                                .color(p.text_muted),
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            match &slot.status {
                                                ConnectionStatus::Connected => {
                                                    if ui.small_button("Disconnect").clicked() {
                                                        action_disconnect = Some(slot.slot_id);
                                                    }
                                                }
                                                ConnectionStatus::Connecting => {
                                                    ui.label(
                                                        RichText::new("Connecting…")
                                                            .size(10.5)
                                                            .color(p.warning),
                                                    );
                                                }
                                                _ => {
                                                    if ui.small_button("Connect").clicked() {
                                                        action_connect = Some(slot.slot_id);
                                                    }
                                                }
                                            }
                                            if ui.small_button("Edit").clicked() {
                                                action_edit = Some(slot.slot_id);
                                            }
                                            if ui.small_button("Remove").clicked() {
                                                action_delete = Some(slot.slot_id);
                                            }
                                        });
                                    });
                                ui.add_space(4.0);
                            }
                        });
                }

                if let Some(id) = action_connect {
                    self.connect_slot(id);
                }
                if let Some(id) = action_disconnect {
                    self.disconnect_slot(id);
                }
                if let Some(id) = action_edit {
                    if let Some(slot) = self.registry.get(id) {
                        let pw = self.storage.get_password(&slot.profile.id);
                        self.modal = Some(ServerModalState::from_existing(id, &slot.profile, pw));
                    }
                }
                if let Some(id) = action_delete {
                    self.disconnect_slot(id);
                    if let Ok(removed) = self.registry.remove(id) {
                        self.storage.delete_password(&removed.profile.id);
                        let _ = self.storage.save_registry(&self.registry);
                        if self.active_slot == Some(id) {
                            self.active_slot = self.registry.iter().next().map(|s| s.slot_id);
                        }
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("SCHEMA EXPLORER")
                            .size(11.0)
                            .strong()
                            .color(p.text_muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.checkbox(&mut self.show_system_schemas, "Sys");
                    });
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.explorer_filter)
                        .hint_text("Filter schemas / tables…")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(4.0);

                let Some(slot_id) = self.active_slot else {
                    return;
                };
                let filter_lower = self.explorer_filter.trim().to_ascii_lowercase();
                let mut open_table_req = None;
                let mut load_l1_req = None;
                let mut load_ddl_req = None;

                egui::ScrollArea::vertical()
                    .id_salt("schema_explorer_scroll")
                    .show(ui, |ui| {
                        if let Some(catalog) = &self.catalogs[slot_id.index()] {
                            for schema in catalog.schemas() {
                                if schema.is_system && !self.show_system_schemas {
                                    continue;
                                }
                                let sname = catalog.resolve(schema.name).to_string();
                                let schema_matches = filter_lower.is_empty()
                                    || sname.to_ascii_lowercase().contains(&filter_lower)
                                    || schema.tables.iter().any(|t| {
                                        catalog
                                            .resolve(t.name)
                                            .to_ascii_lowercase()
                                            .contains(&filter_lower)
                                    });
                                if !schema_matches {
                                    continue;
                                }

                                let header_label = if schema.l1_loaded {
                                    format!("{} ({})", sname, schema.tables.len())
                                } else {
                                    format!("{sname} · lazy")
                                };
                                let resp = egui::CollapsingHeader::new(
                                    RichText::new(header_label).strong().color(if schema.is_system {
                                        p.text_muted
                                    } else {
                                        p.text_primary
                                    }),
                                )
                                .id_salt(("schema", slot_id.index(), &sname))
                                .show(ui, |ui| {
                                    if !schema.l1_loaded {
                                        if ui.small_button("Load tables & columns (L1)").clicked() {
                                            load_l1_req = Some(sname.clone());
                                        }
                                        return;
                                    }
                                    for table in &schema.tables {
                                        let tname = catalog.resolve(table.name).to_string();
                                        if !filter_lower.is_empty()
                                            && !tname.to_ascii_lowercase().contains(&filter_lower)
                                            && !sname.to_ascii_lowercase().contains(&filter_lower)
                                        {
                                            continue;
                                        }
                                        let icon = match table.kind {
                                            TableKind::BaseTable => "▦",
                                            TableKind::View => "◈",
                                        };
                                        ui.horizontal(|ui| {
                                            if ui
                                                .selectable_label(
                                                    false,
                                                    format!("{icon} {tname}"),
                                                )
                                                .clicked()
                                            {
                                                open_table_req = Some((sname.clone(), tname.clone()));
                                            }
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if ui.small_button("DDL").clicked() {
                                                        load_ddl_req =
                                                            Some((sname.clone(), tname.clone()));
                                                    }
                                                },
                                            );
                                        });
                                        if !table.columns.is_empty() {
                                            egui::CollapsingHeader::new(
                                                RichText::new(format!(
                                                    "Columns ({})",
                                                    table.columns.len()
                                                ))
                                                .size(10.5)
                                                .color(p.text_muted),
                                            )
                                            .id_salt(("cols", slot_id.index(), &sname, &tname))
                                            .show(ui, |ui| {
                                                for col in &table.columns {
                                                    let cname = catalog.resolve(col.name);
                                                    let ctype = catalog.resolve(col.column_type);
                                                    let badge = if col.is_primary_key {
                                                        "PK "
                                                    } else {
                                                        ""
                                                    };
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "{badge}{cname}: {ctype}"
                                                        ))
                                                        .font(FontId::new(11.0, FontFamily::Monospace))
                                                        .color(if col.is_primary_key {
                                                            p.warning
                                                        } else {
                                                            p.text_muted
                                                        }),
                                                    );
                                                }
                                            });
                                        }
                                    }
                                });

                                let key = (slot_id.index(), sname.clone());
                                let was_open = self.expanded_schemas.get(&key).copied().unwrap_or(false);
                                let now_open = resp.body_returned.is_some();
                                if now_open && !was_open && !schema.l1_loaded {
                                    load_l1_req = Some(sname.clone());
                                }
                                self.expanded_schemas.insert(key, now_open);
                            }
                        } else {
                            ui.label(
                                RichText::new("Connect to a MySQL slot to browse schemas.")
                                    .size(11.5)
                                    .color(p.text_muted),
                            );
                        }
                    });

                if let Some(schema) = load_l1_req {
                    self.hub.send(slot_id, SlotCmd::LoadSchemaL1 { schema });
                }
                if let Some((schema, table)) = open_table_req {
                    self.open_table_tab(slot_id, schema, table);
                }
                if let Some((schema, table)) = load_ddl_req {
                    self.hub
                        .send(slot_id, SlotCmd::LoadTableL2 { schema, table });
                }
            });
    }

    fn render_workspace(&mut self, ctx: &egui::Context, p: Palette) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.bg_canvas)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ctx, |ui| {
                if let Some((msg, is_err)) = self.toast_banner.clone() {
                    let tint = if is_err { p.danger } else { p.success };
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 28))
                        .stroke(Stroke::new(1.0, tint))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(10, 5))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(msg).color(p.text_primary).size(12.0));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("✕").clicked() {
                                        self.toast_banner = None;
                                    }
                                });
                            });
                        });
                    ui.add_space(6.0);
                }

                let mut close_idx = None;
                ui.horizontal(|ui| {
                    for (idx, tab) in self.tabs.iter().enumerate() {
                        let is_active = idx == self.active_tab_idx;
                        let slot_tag = tab
                            .slot
                            .map(|s| format!("{s} "))
                            .unwrap_or_default();
                        let label = format!("{slot_tag}{}", tab.title);
                        if ui.selectable_label(is_active, label).clicked() {
                            self.active_tab_idx = idx;
                            if let Some(s) = tab.slot {
                                self.active_slot = Some(s);
                            }
                            self.memory_budget.touch(&format!("tab-{}", tab.id));
                        }
                        if self.tabs.len() > 1 && ui.small_button("×").clicked() {
                            close_idx = Some(idx);
                        }
                    }
                    if ui.small_button("+ Console").clicked() {
                        let id = self.next_tab_id;
                        self.next_tab_id += 1;
                        self.tabs.push(QueryTab::new_console(id, self.active_slot));
                        self.active_tab_idx = self.tabs.len() - 1;
                    }
                });
                if let Some(idx) = close_idx {
                    let removed = self.tabs.remove(idx);
                    self.memory_budget.remove(&format!("tab-{}", removed.id));
                    if self.active_tab_idx >= self.tabs.len() {
                        self.active_tab_idx = self.tabs.len().saturating_sub(1);
                    }
                }
                ui.separator();

                self.render_sql_editor_section(ui, p);
                ui.add_space(6.0);
                self.render_bottom_deck(ui, p);
            });
    }

    fn render_sql_editor_section(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut run_current = false;
        let mut run_all = false;
        let mut stop_query = false;
        let mut do_explain = None;
        let mut do_format = false;

        let (is_running, safety_warning, current_sql, cursor_byte, slot_for_tab) = {
            let tab = &self.tabs[self.active_tab_idx];
            let stmt = statement_at_offset(&tab.sql, tab.cursor_byte)
                .map(|s| s.sql)
                .unwrap_or(&tab.sql);
            let safety = analyze_statement(stmt);
            (
                tab.is_running,
                if matches!(safety.class, WriteClass::Destructive) {
                    safety.warning
                } else {
                    None
                },
                tab.sql.clone(),
                tab.cursor_byte,
                tab.slot.or(self.active_slot),
            )
        };

        ui.horizontal(|ui| {
            if ui
                .add_enabled(!is_running, egui::Button::new("▶ Run (Cmd+Enter)"))
                .clicked()
            {
                run_current = true;
            }
            if ui
                .add_enabled(!is_running, egui::Button::new("▶▶ Run Last"))
                .clicked()
            {
                run_all = true;
            }
            if ui
                .add_enabled(is_running, egui::Button::new("■ Stop (KILL)"))
                .clicked()
            {
                stop_query = true;
            }
            if ui.add_enabled(!is_running, egui::Button::new("Explain")).clicked() {
                do_explain = Some(false);
            }
            if ui
                .add_enabled(!is_running, egui::Button::new("Explain Analyze"))
                .clicked()
            {
                do_explain = Some(true);
            }
            if ui.button("Format SQL").clicked() {
                do_format = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(&self.tabs[self.active_tab_idx].last_status_msg)
                        .size(11.5)
                        .color(p.text_muted),
                );
            });
        });

        if let Some(warn) = safety_warning {
            ui.label(
                RichText::new(format!("⚠ Safety Guard: {warn}"))
                    .size(11.5)
                    .strong()
                    .color(p.warning),
            );
        }

        let palette_copy = p;
        let mut layouter = move |ui: &egui::Ui, string: &str, wrap_width: f32| {
            highlight_mysql_layout(ui, string, wrap_width, palette_copy)
        };

        egui::Frame::new()
            .fill(p.bg_surface)
            .stroke(Stroke::new(1.0, p.glass_border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("sql_editor_scroll")
                    .max_height(190.0)
                    .show(ui, |ui| {
                        let tab = &mut self.tabs[self.active_tab_idx];
                        let out = egui::TextEdit::multiline(&mut tab.sql)
                            .desired_rows(7)
                            .desired_width(f32::INFINITY)
                            .code_editor()
                            .frame(false)
                            .layouter(&mut layouter)
                            .show(ui);
                        if let Some(range) = out.cursor_range {
                            let char_idx = range.primary.ccursor.index;
                            tab.cursor_byte = tab
                                .sql
                                .char_indices()
                                .nth(char_idx)
                                .map(|(b, _)| b)
                                .unwrap_or(tab.sql.len());
                        }
                    });
            });

        let hints = slot_for_tab
            .and_then(|s| {
                let active_db = self
                    .registry
                    .get(s)
                    .and_then(|sl| sl.runtime.as_ref())
                    .and_then(|r| r.current_schema.as_deref());
                self.catalogs[s.index()]
                    .as_ref()
                    .map(|cat| cat.completion_hints(active_db))
            })
            .unwrap_or_default();

        let suggestions = CompletionProvider::suggest(&current_sql, cursor_byte, &hints, 6);
        if !suggestions.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Complete:").size(10.5).color(p.text_muted));
                for item in &suggestions {
                    if ui
                        .small_button(format!("{} ({})", item.label, item.detail))
                        .clicked()
                    {
                        let tab = &mut self.tabs[self.active_tab_idx];
                        insert_completion(&mut tab.sql, tab.cursor_byte, &item.label);
                    }
                }
            });
        }

        if run_current {
            self.execute_active_tab(false);
        }
        if run_all {
            self.execute_active_tab(true);
        }
        if stop_query {
            if let Some(slot_id) = slot_for_tab {
                if let Some(slot) = self.registry.get(slot_id) {
                    if let Some(rt) = &slot.runtime {
                        let pw = self.storage.get_password(&slot.profile.id);
                        self.hub.cancel_query_async(
                            slot_id,
                            slot.profile.clone(),
                            pw,
                            rt.connection_id,
                        );
                    }
                }
            }
        }
        if let Some(analyze) = do_explain {
            if let Some(slot_id) = slot_for_tab {
                let tab = &mut self.tabs[self.active_tab_idx];
                let sql = statement_at_offset(&tab.sql, tab.cursor_byte)
                    .map(|s| s.sql.to_string())
                    .unwrap_or_else(|| tab.sql.trim().to_string());
                if !sql.is_empty() {
                    tab.is_running = true;
                    let tid = tab.id;
                    self.hub.send(
                        slot_id,
                        SlotCmd::ExplainQuery {
                            tab_id: tid,
                            sql,
                            analyze,
                        },
                    );
                }
            }
        }
        if do_format {
            let tab = &mut self.tabs[self.active_tab_idx];
            tab.sql = format_mysql(&tab.sql);
        }
    }

    fn render_bottom_deck(&mut self, ui: &mut egui::Ui, p: Palette) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.bottom_tab, BottomDeckTab::DataGrid, "Data Grid");
            ui.selectable_value(&mut self.bottom_tab, BottomDeckTab::DdlInspector, "DDL & Indexes");
            ui.selectable_value(&mut self.bottom_tab, BottomDeckTab::ExplainPlan, "Explain Plan");
            if ui
                .selectable_label(self.bottom_tab == BottomDeckTab::Monitor, "Processlist & Health")
                .clicked()
            {
                self.bottom_tab = BottomDeckTab::Monitor;
                if let Some(slot) = self.active_slot {
                    self.hub.send(slot, SlotCmd::FetchProcesslist);
                    self.hub.send(slot, SlotCmd::FetchHealth);
                }
            }
            ui.selectable_value(
                &mut self.bottom_tab,
                BottomDeckTab::History,
                format!("History ({})", self.history.len()),
            );
        });
        ui.separator();

        match self.bottom_tab {
            BottomDeckTab::DataGrid => self.render_data_grid(ui, p),
            BottomDeckTab::DdlInspector => self.render_ddl_view(ui, p),
            BottomDeckTab::ExplainPlan => self.render_explain_view(ui, p),
            BottomDeckTab::Monitor => self.render_monitor_view(ui, p),
            BottomDeckTab::History => self.render_history_view(ui, p),
        }
    }

    fn render_data_grid(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut reload_page = false;
        let mut count_rows_req = false;
        let mut export_req = None;
        let mut preview_req = false;

        let active_slot = self.tabs[self.active_tab_idx].slot.or(self.active_slot);
        let is_read_only = active_slot
            .and_then(|s| self.registry.get(s))
            .map(|sl| sl.profile.read_only)
            .unwrap_or(false);

        let bound_table = self.tabs[self.active_tab_idx].bound_table.clone();

        ui.horizontal_wrapped(|ui| {
            let tab = &mut self.tabs[self.active_tab_idx];
            if bound_table.is_some() {
                ui.label(RichText::new("WHERE").size(11.0).color(p.accent));
                let filter_resp = ui.add(
                    egui::TextEdit::singleline(&mut tab.filter_sort.where_clause)
                        .hint_text("e.g. status = 'active' AND id > 100")
                        .desired_width(240.0),
                );
                if filter_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    tab.filter_sort.page_index = 0;
                    reload_page = true;
                }
                if ui.small_button("Apply").clicked() {
                    tab.filter_sort.page_index = 0;
                    reload_page = true;
                }
                ui.separator();
                if ui
                    .add_enabled(tab.filter_sort.page_index > 0, egui::Button::new("◀ Prev"))
                    .clicked()
                {
                    tab.filter_sort.page_index -= 1;
                    reload_page = true;
                }
                ui.label(
                    RichText::new(format!("Page {}", tab.filter_sort.page_index + 1))
                        .size(11.5)
                        .color(p.text_muted),
                );
                let has_more = tab.page.as_ref().map(|pg| pg.has_more_rows).unwrap_or(false);
                if ui
                    .add_enabled(has_more, egui::Button::new("Next ▶"))
                    .clicked()
                {
                    tab.filter_sort.page_index += 1;
                    reload_page = true;
                }
                let total_label = tab
                    .page
                    .as_ref()
                    .and_then(|pg| pg.total_rows_known)
                    .map(|n| format!("Total: {n}"))
                    .unwrap_or_else(|| "Count Rows".into());
                if ui.small_button(total_label).clicked() {
                    count_rows_req = true;
                }
            }

            if tab.page.is_some() {
                ui.separator();
                if bound_table.is_some() && !is_read_only {
                    if ui.small_button("+ Row").clicked() {
                        if let Some(pg) = &tab.page {
                            tab.mutator.insert_blank_row(pg);
                        }
                    }
                    if let Some((sel_r, _)) = tab.selected_cell {
                        if ui.small_button("Clone Row").clicked() {
                            if let Some(pg) = &tab.page {
                                tab.mutator.clone_row(pg, sel_r);
                            }
                        }
                        if ui.small_button("Toggle Delete").clicked() {
                            tab.mutator.toggle_delete_row(sel_r);
                        }
                    }
                    if tab.mutator.has_changes() {
                        let (d, u, ins) = tab.mutator.pending_summary();
                        if ui
                            .button(
                                RichText::new(format!("Preview & Submit (D:{d} U:{u} I:{ins})"))
                                    .color(p.success),
                            )
                            .clicked()
                        {
                            preview_req = true;
                        }
                        if ui.small_button("Revert All").clicked() {
                            tab.mutator.revert_all();
                        }
                    }
                    ui.separator();
                }

                egui::ComboBox::from_id_salt("export_menu")
                    .selected_text("Export…")
                    .width(95.0)
                    .show_ui(ui, |ui| {
                        for fmt in [
                            ExportFormat::Csv,
                            ExportFormat::JsonLines,
                            ExportFormat::JsonArray,
                            ExportFormat::SqlInserts,
                        ] {
                            if ui.selectable_label(false, fmt.label()).clicked() {
                                export_req = Some(fmt);
                            }
                        }
                    });
            }
        });

        if reload_page {
            if let (Some(slot), Some((schema, table))) = (active_slot, bound_table.clone()) {
                let tab = &mut self.tabs[self.active_tab_idx];
                tab.sql = tab.filter_sort.build_select_sql(&schema, &table);
                tab.is_running = true;
                let tid = tab.id;
                let fs = tab.filter_sort.clone();
                self.hub.send(
                    slot,
                    SlotCmd::LoadTablePage {
                        tab_id: tid,
                        schema,
                        table,
                        filter_sort: fs,
                    },
                );
            }
        }

        if count_rows_req {
            if let (Some(slot), Some((schema, table))) = (active_slot, bound_table.clone()) {
                let tab = &self.tabs[self.active_tab_idx];
                self.hub.send(
                    slot,
                    SlotCmd::CountTableRows {
                        tab_id: tab.id,
                        schema,
                        table,
                        filter_sort: tab.filter_sort.clone(),
                    },
                );
            }
        }

        if let Some(fmt) = export_req {
            self.export_active_page(fmt);
        }

        if preview_req {
            if let (Some(slot), Some((schema, table))) = (active_slot, bound_table.clone()) {
                let pks = self.catalogs[slot.index()]
                    .as_ref()
                    .map(|cat| cat.primary_keys_for(&schema, &table))
                    .unwrap_or_default();
                let tab = &self.tabs[self.active_tab_idx];
                if let Some(page) = &tab.page {
                    let fallback_pks: Vec<String> = if pks.is_empty() {
                        page.columns
                            .iter()
                            .filter(|c| c.is_primary_key)
                            .map(|c| c.name.clone())
                            .collect()
                    } else {
                        pks
                    };
                    match tab
                        .mutator
                        .build_pending_sql(page, &schema, &table, &fallback_pks)
                    {
                        Ok(batch) if !batch.is_empty() => {
                            self.sql_preview_modal =
                                Some((tab.id, batch.as_script(), batch.statements));
                        }
                        Ok(_) => {}
                        Err(err) => self.toast_banner = Some((err, true)),
                    }
                }
            }
        }

        ui.add_space(4.0);

        let mut clicked_sort_col: Option<String> = None;
        let available_h = ui.available_height().max(160.0);

        let has_selected = self.tabs[self.active_tab_idx].selected_cell.is_some();
        let inspector_w = if has_selected { 290.0 } else { 0.0 };
        let grid_w = (ui.available_width() - inspector_w - 8.0).max(300.0);

        ui.horizontal_top(|ui| {
            ui.allocate_ui(egui::vec2(grid_w, available_h), |ui| {
                let tab = &mut self.tabs[self.active_tab_idx];
                let Some(page) = &tab.page else {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new("Run a MySQL query or click any table in the explorer.")
                                .color(p.text_muted),
                        );
                    });
                    return;
                };

                let col_w = 155.0;
                let row_h = 24.0;
                let total_rows = page.row_count + tab.mutator.inserted_rows().len();

                egui::ScrollArea::horizontal()
                    .id_salt(("grid_h", tab.id))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized([46.0, row_h], egui::Label::new(RichText::new("#").color(p.text_muted)));
                            for col in &page.columns {
                                let sort_arrow = match &tab.filter_sort.order_by {
                                    Some((c, SortDirection::Asc)) if c == &col.name => " ▲",
                                    Some((c, SortDirection::Desc)) if c == &col.name => " ▼",
                                    _ => "",
                                };
                                let pk = if col.is_primary_key { "🔑 " } else { "" };
                                let hdr = format!("{pk}{}{sort_arrow}", col.name);
                                let resp = ui.add_sized(
                                    [col_w, row_h],
                                    egui::Button::new(RichText::new(hdr).strong().size(11.5))
                                        .fill(p.bg_elevated),
                                );
                                if resp.clicked() && bound_table.is_some() {
                                    clicked_sort_col = Some(col.name.clone());
                                }
                            }
                        });

                        egui::ScrollArea::vertical()
                            .id_salt(("grid_v", tab.id))
                            .show_rows(ui, row_h, total_rows, |ui, row_range| {
                                for r in row_range {
                                    let is_inserted = r >= page.row_count;
                                    let is_deleted = !is_inserted && tab.mutator.is_deleted(r);
                                    ui.horizontal(|ui| {
                                        let row_badge = if is_inserted {
                                            format!("+{}", r - page.row_count + 1)
                                        } else if is_deleted {
                                            format!("-{}", r + 1)
                                        } else {
                                            format!("{}", r + 1)
                                        };
                                        ui.add_sized(
                                            [46.0, row_h],
                                            egui::Label::new(
                                                RichText::new(row_badge)
                                                    .size(11.0)
                                                    .color(p.text_muted),
                                            ),
                                        );

                                        for c in 0..page.columns.len() {
                                            let is_selected = tab.selected_cell == Some((r, c));
                                            let (display_text, is_null, cell_bg) = if is_inserted {
                                                let ins_idx = r - page.row_count;
                                                let val = &tab.mutator.inserted_rows()[ins_idx][c];
                                                (
                                                    truncate_display(val.as_display_str(), 28),
                                                    val.is_null(),
                                                    p.cell_inserted,
                                                )
                                            } else if let Some(modified) = tab.mutator.get_modified(r, c) {
                                                (
                                                    truncate_display(modified.as_display_str(), 28),
                                                    modified.is_null(),
                                                    p.cell_modified,
                                                )
                                            } else {
                                                let base_bg = if is_deleted {
                                                    p.cell_deleted
                                                } else if r % 2 == 1 {
                                                    p.row_alt
                                                } else {
                                                    Color32::TRANSPARENT
                                                };
                                                match page.cell_str(r, c) {
                                                    None => ("NULL".into(), true, base_bg),
                                                    Some(s) => (truncate_display(s, 28), false, base_bg),
                                                }
                                            };

                                            let text_color = if is_null {
                                                p.text_muted
                                            } else {
                                                p.text_primary
                                            };
                                            let fill = if is_selected { p.accent_soft } else { cell_bg };
                                            let btn = egui::Button::new(
                                                RichText::new(display_text)
                                                    .font(FontId::new(11.5, FontFamily::Monospace))
                                                    .color(text_color),
                                            )
                                            .fill(fill)
                                            .stroke(if is_selected {
                                                Stroke::new(1.0, p.accent)
                                            } else {
                                                Stroke::new(0.5, p.glass_border)
                                            });
                                            if ui.add_sized([col_w, row_h], btn).clicked() {
                                                tab.selected_cell = Some((r, c));
                                                let current_val = if is_inserted {
                                                    tab.mutator.inserted_rows()[r - page.row_count][c].clone()
                                                } else {
                                                    tab.mutator
                                                        .get_modified(r, c)
                                                        .cloned()
                                                        .unwrap_or_else(|| page.cell_value(r, c))
                                                };
                                                tab.cell_edit_is_null = current_val.is_null();
                                                tab.cell_edit_buf = if current_val.is_null() {
                                                    String::new()
                                                } else {
                                                    current_val.as_display_str().to_string()
                                                };
                                            }
                                        }
                                    });
                                }
                            });
                    });
            });

            if has_selected {
                ui.allocate_ui(egui::vec2(inspector_w, available_h), |ui| {
                    egui::Frame::new()
                        .fill(p.bg_surface)
                        .stroke(Stroke::new(1.0, p.glass_border))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            let tab = &mut self.tabs[self.active_tab_idx];
                            if let (Some((r, c)), Some(page)) = (tab.selected_cell, &tab.page) {
                                let col = &page.columns[c];
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("Cell [{}:{}]", r + 1, col.name))
                                            .strong(),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("✕").clicked() {
                                                tab.selected_cell = None;
                                            }
                                        },
                                    );
                                });
                                ui.label(
                                    RichText::new(format!("Type: {}", col.type_name))
                                        .size(11.0)
                                        .color(p.text_muted),
                                );
                                if r < page.row_count {
                                    if let Some(tot) = page.cell_truncated_bytes(r, c) {
                                        ui.label(
                                            RichText::new(format!(
                                                "Preview capped at 4 KiB (total {tot} B)"
                                            ))
                                            .size(10.5)
                                            .color(p.warning),
                                        );
                                    }
                                }
                                ui.separator();
                                ui.checkbox(&mut tab.cell_edit_is_null, "SQL NULL");
                                egui::ScrollArea::vertical()
                                    .max_height((available_h - 140.0).max(80.0))
                                    .show(ui, |ui| {
                                        ui.add_enabled(
                                            !tab.cell_edit_is_null,
                                            egui::TextEdit::multiline(&mut tab.cell_edit_buf)
                                                .code_editor()
                                                .desired_width(f32::INFINITY)
                                                .desired_rows(6),
                                        );
                                    });
                                ui.add_space(6.0);
                                if bound_table.is_some() && !is_read_only {
                                    ui.horizontal(|ui| {
                                        if ui.button("Stage Edit").clicked() {
                                            let new_val = if tab.cell_edit_is_null {
                                                CellValue::Null
                                            } else {
                                                CellValue::Text(tab.cell_edit_buf.clone())
                                            };
                                            if r >= page.row_count {
                                                tab.mutator
                                                    .set_inserted_cell(r - page.row_count, c, new_val);
                                            } else {
                                                tab.mutator.set_cell(page, r, c, new_val);
                                            }
                                        }
                                        if r < page.row_count && ui.small_button("Revert").clicked() {
                                            tab.mutator.revert_cell(r, c);
                                        }
                                    });
                                }
                            }
                        });
                });
            }
        });

        if let Some(col_name) = clicked_sort_col {
            if let (Some(slot), Some((schema, table))) = (active_slot, bound_table) {
                let tab = &mut self.tabs[self.active_tab_idx];
                let next_dir = match &tab.filter_sort.order_by {
                    Some((c, SortDirection::Asc)) if c == &col_name => Some(SortDirection::Desc),
                    Some((c, SortDirection::Desc)) if c == &col_name => None,
                    _ => Some(SortDirection::Asc),
                };
                tab.filter_sort.order_by = next_dir.map(|d| (col_name, d));
                tab.filter_sort.page_index = 0;
                tab.sql = tab.filter_sort.build_select_sql(&schema, &table);
                tab.is_running = true;
                let tid = tab.id;
                let fs = tab.filter_sort.clone();
                self.hub.send(
                    slot,
                    SlotCmd::LoadTablePage {
                        tab_id: tid,
                        schema,
                        table,
                        filter_sort: fs,
                    },
                );
            }
        }
    }

    fn export_active_page(&mut self, fmt: ExportFormat) {
        let Some(page) = &self.tabs[self.active_tab_idx].page else {
            return;
        };
        let default_name = format!(
            "{}.{}",
            self.tabs[self.active_tab_idx]
                .title
                .replace(['.', ' ', '/'], "_"),
            fmt.extension()
        );
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };
        let result = File::create(&path).and_then(|file| {
            let mut writer = BufWriter::new(file);
            match fmt {
                ExportFormat::Csv => export_csv(page, &mut writer),
                ExportFormat::JsonLines => export_json_lines(page, &mut writer),
                ExportFormat::JsonArray => export_json_array(page, &mut writer),
                ExportFormat::SqlInserts => {
                    let tbl = self.tabs[self.active_tab_idx]
                        .bound_table
                        .as_ref()
                        .map(|(_, t)| t.as_str())
                        .unwrap_or("exported_rows");
                    export_sql_inserts(page, tbl, &mut writer)
                }
            }
        });
        match result {
            Ok(()) => {
                self.toast_banner = Some((format!("Exported to {}", path.display()), false));
            }
            Err(err) => {
                self.toast_banner = Some((format!("Export failed: {err}"), true));
            }
        }
    }

    fn render_ddl_view(&self, ui: &mut egui::Ui, p: Palette) {
        let Some((schema, table, detail)) = &self.ddl_view else {
            ui.label(
                RichText::new("Click 'DDL' next to any table in the Schema Explorer to inspect CREATE TABLE, indexes, and foreign keys.")
                    .color(p.text_muted),
            );
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{schema}.{table}")).strong());
            if let Some(eng) = &detail.engine {
                ui.label(RichText::new(format!("Engine: {eng}")).color(p.text_muted));
            }
            if let Some(col) = &detail.collation {
                ui.label(RichText::new(format!("Collation: {col}")).color(p.text_muted));
            }
            if let Some(rows) = detail.row_estimate {
                ui.label(RichText::new(format!("~{rows} rows")).color(p.text_muted));
            }
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut ddl_str = detail.ddl.clone();
            ui.add(
                egui::TextEdit::multiline(&mut ddl_str)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(10),
            );
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Indexes ({})", detail.indexes.len())).strong());
            for idx in &detail.indexes {
                let uniq = if idx.non_unique { "INDEX" } else { "UNIQUE" };
                ui.label(
                    RichText::new(format!(
                        "  {} [{} · {}] ({})",
                        idx.name,
                        uniq,
                        idx.index_type,
                        idx.columns.join(", ")
                    ))
                    .font(FontId::new(11.5, FontFamily::Monospace)),
                );
            }
            if !detail.foreign_keys.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("Foreign Keys ({})", detail.foreign_keys.len())).strong(),
                );
                for fk in &detail.foreign_keys {
                    ui.label(
                        RichText::new(format!(
                            "  {} : {} -> {}.{}.{} (ON UPDATE {}, ON DELETE {})",
                            fk.constraint_name,
                            fk.column_name,
                            fk.ref_schema,
                            fk.ref_table,
                            fk.ref_column,
                            fk.update_rule,
                            fk.delete_rule
                        ))
                        .font(FontId::new(11.5, FontFamily::Monospace)),
                    );
                }
            }
        });
    }

    fn render_explain_view(&self, ui: &mut egui::Ui, p: Palette) {
        let Some(exp) = &self.tabs[self.active_tab_idx].explain else {
            ui.label(
                RichText::new("Click 'Explain' or 'Explain Analyze' in the SQL toolbar to inspect MySQL execution plans.")
                    .color(p.text_muted),
            );
            return;
        };
        let has_full_scan = exp.raw_output.contains("\"access_type\": \"ALL\"")
            || exp.raw_output.contains("Table scan");
        ui.horizontal(|ui| {
            let kind = if exp.is_analyze { "EXPLAIN ANALYZE" } else { "EXPLAIN FORMAT=JSON" };
            ui.label(RichText::new(format!("{kind} · {} ms", exp.elapsed_ms)).strong());
            if has_full_scan {
                ui.label(
                    RichText::new("⚠ Full Table Scan detected — consider adding an index")
                        .color(p.warning)
                        .strong(),
                );
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut text = exp.raw_output.clone();
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(14),
            );
        });
    }

    fn render_monitor_view(&mut self, ui: &mut egui::Ui, p: Palette) {
        let Some(slot) = self.active_slot else {
            ui.label("Select a connected server slot.");
            return;
        };
        ui.horizontal(|ui| {
            if ui.small_button("↻ Refresh Processlist & Metrics").clicked() {
                self.hub.send(slot, SlotCmd::FetchProcesslist);
                self.hub.send(slot, SlotCmd::FetchHealth);
            }
            if let Some(m) = &self.health[slot.index()] {
                let hit = m
                    .innodb_hit_ratio_pct
                    .map(|v| format!("{v:.2}%"))
                    .unwrap_or_else(|| "n/a".into());
                ui.label(
                    RichText::new(format!(
                        "Uptime: {}s · Threads: {}/{} running · Questions: {} · InnoDB Hit: {}",
                        m.uptime_secs,
                        m.threads_running,
                        m.threads_connected,
                        m.questions,
                        hit
                    ))
                    .size(11.5)
                    .color(p.accent),
                );
            }
        });
        ui.add_space(4.0);

        let mut kill_pid = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for proc in &self.processlists[slot.index()] {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "#{} {}@{} [{}] {}s · {} · {}",
                            proc.id,
                            proc.user,
                            proc.host,
                            proc.db.as_deref().unwrap_or("-"),
                            proc.time_secs,
                            proc.command,
                            proc.info.as_deref().unwrap_or("").chars().take(90).collect::<String>()
                        ))
                        .font(FontId::new(11.0, FontFamily::Monospace)),
                    );
                    if ui.small_button("Kill Query").clicked() {
                        kill_pid = Some(proc.id);
                    }
                });
            }
        });
        if let Some(pid) = kill_pid {
            self.hub.send(slot, SlotCmd::KillProcess(pid));
        }
    }

    fn render_history_view(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut load_sql = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in &self.history {
                ui.horizontal(|ui| {
                    if ui.small_button("Load").clicked() {
                        load_sql = Some(item.sql.clone());
                    }
                    ui.label(
                        RichText::new(format!(
                            "[{} · {} ms] {}",
                            item.server_name,
                            item.elapsed_ms,
                            truncate_display(&item.sql.replace('\n', " "), 110)
                        ))
                        .font(FontId::new(11.5, FontFamily::Monospace))
                        .color(p.text_primary),
                    );
                });
            }
        });
        if let Some(sql) = load_sql {
            self.tabs[self.active_tab_idx].sql = sql;
        }
    }

    fn render_modals(&mut self, ctx: &egui::Context, p: Palette) {
        if let Some((tab_id, script, statements)) = self.sql_preview_modal.clone() {
            let mut open = true;
            let mut submit_now = false;
            egui::Window::new("Preview Staged MySQL Transaction")
                .open(&mut open)
                .resizable(true)
                .default_width(620.0)
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("All changes execute atomically in order: DELETE -> UPDATE -> INSERT.")
                            .size(11.5)
                            .color(p.text_muted),
                    );
                    let mut preview_buf = script.clone();
                    egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut preview_buf)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(12),
                        );
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Commit Transaction").clicked() {
                            submit_now = true;
                        }
                        if ui.button("Cancel").clicked() {
                            self.sql_preview_modal = None;
                        }
                    });
                });
            if submit_now {
                if let Some(slot) = self.tabs[self.active_tab_idx].slot.or(self.active_slot) {
                    self.hub
                        .send(slot, SlotCmd::SubmitMutations { tab_id, statements });
                }
                self.sql_preview_modal = None;
            } else if !open {
                self.sql_preview_modal = None;
            }
        }

        let Some(mut state) = self.modal.take() else {
            return;
        };
        let mut keep_open = true;
        let mut save_clicked = false;
        let title = if let Some(slot) = state.editing_slot {
            format!("Edit MySQL Server ({slot})")
        } else {
            format!(
                "Add MySQL Server (Slot {}/{})",
                self.registry.count() + 1,
                MAX_SERVERS
            )
        };

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .default_width(470.0)
            .show(ctx, |ui| {
                if let Some(err) = &state.error_msg {
                    ui.label(RichText::new(err).color(p.danger));
                }

                egui::Grid::new("server_form_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Server Name");
                        ui.text_edit_singleline(&mut state.name);
                        ui.end_row();

                        ui.label("Transport");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut state.use_unix_socket, false, "TCP/IP");
                            ui.radio_value(&mut state.use_unix_socket, true, "Unix Socket");
                        });
                        ui.end_row();

                        if state.use_unix_socket {
                            ui.label("Socket Path");
                            ui.text_edit_singleline(&mut state.socket_path);
                            ui.end_row();
                        } else {
                            ui.label("Host & Port");
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut state.host).desired_width(210.0));
                                ui.add(egui::TextEdit::singleline(&mut state.port).desired_width(64.0));
                            });
                            ui.end_row();
                        }

                        ui.label("Username");
                        ui.text_edit_singleline(&mut state.username);
                        ui.end_row();

                        ui.label("Password");
                        ui.add(egui::TextEdit::singleline(&mut state.password).password(true));
                        ui.end_row();

                        ui.label("Credential Vault");
                        ui.checkbox(
                            &mut state.save_in_keychain,
                            if cfg!(target_os = "macos") {
                                "Store in macOS Keychain"
                            } else {
                                "Keep in memory for session"
                            },
                        );
                        ui.end_row();

                        ui.label("Default Schema");
                        ui.text_edit_singleline(&mut state.default_schema);
                        ui.end_row();

                        ui.label("TLS Mode");
                        egui::ComboBox::from_id_salt("modal_tls")
                            .selected_text(state.tls.label())
                            .show_ui(ui, |ui| {
                                for &m in TlsMode::all() {
                                    ui.selectable_value(&mut state.tls, m, m.label());
                                }
                            });
                        ui.end_row();

                        if matches!(state.tls, TlsMode::VerifyCa | TlsMode::VerifyIdentity) {
                            ui.label("CA Cert PEM");
                            ui.horizontal(|ui| {
                                ui.text_edit_singleline(&mut state.ca_cert_path);
                                if ui.small_button("Browse").clicked() {
                                    if let Some(f) = rfd::FileDialog::new().pick_file() {
                                        state.ca_cert_path = f.display().to_string();
                                    }
                                }
                            });
                            ui.end_row();
                        }

                        ui.label("Environment");
                        ui.horizontal(|ui| {
                            for env in [
                                EnvironmentTag::Local,
                                EnvironmentTag::Development,
                                EnvironmentTag::Staging,
                                EnvironmentTag::Production,
                            ] {
                                ui.selectable_value(&mut state.environment, env, env.label());
                            }
                        });
                        ui.end_row();

                        ui.label("Safety & Paging");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut state.read_only, "Read-Only Guard");
                            egui::ComboBox::from_id_salt("modal_page_size")
                                .selected_text(format!("{} rows/page", state.page_size))
                                .show_ui(ui, |ui| {
                                    for sz in [100u32, 500, 1000, 2000] {
                                        ui.selectable_value(
                                            &mut state.page_size,
                                            sz,
                                            format!("{sz} rows"),
                                        );
                                    }
                                });
                        });
                        ui.end_row();
                    });

                ui.add_space(4.0);
                ui.checkbox(&mut state.ssh_enabled, "Connect through SSH Tunnel");
                if state.ssh_enabled {
                    egui::Grid::new("ssh_form_grid")
                        .num_columns(2)
                        .spacing([12.0, 5.0])
                        .show(ui, |ui| {
                            ui.label("SSH Host & Port");
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut state.ssh_host).desired_width(200.0));
                                ui.add(egui::TextEdit::singleline(&mut state.ssh_port).desired_width(60.0));
                            });
                            ui.end_row();

                            ui.label("SSH User");
                            ui.text_edit_singleline(&mut state.ssh_user);
                            ui.end_row();

                            ui.label("Identity Key");
                            ui.horizontal(|ui| {
                                ui.text_edit_singleline(&mut state.ssh_key);
                                if ui.small_button("Browse").clicked() {
                                    if let Some(f) = rfd::FileDialog::new().pick_file() {
                                        state.ssh_key = f.display().to_string();
                                    }
                                }
                            });
                            ui.end_row();
                        });
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Save MySQL Server").clicked() {
                        save_clicked = true;
                    }
                    if ui.button("Cancel").clicked() {
                        keep_open = false;
                    }
                });
            });

        if save_clicked {
            let existing_id = state
                .editing_slot
                .and_then(|s| self.registry.get(s))
                .map(|sl| sl.profile.id.clone());
            match state.build_profile(existing_id) {
                Ok(profile) => {
                    let profile_id = profile.id.clone();
                    let res = if let Some(slot_id) = state.editing_slot {
                        self.registry.update(slot_id, profile).map(|_| slot_id)
                    } else {
                        self.registry.add(profile)
                    };
                    match res {
                        Ok(slot_id) => {
                            if !state.password.is_empty() {
                                self.storage.save_password(
                                    &profile_id,
                                    &state.password,
                                    state.save_in_keychain,
                                );
                            }
                            let _ = self.storage.save_registry(&self.registry);
                            self.active_slot = Some(slot_id);
                            keep_open = false;
                        }
                        Err(err) => {
                            state.error_msg = Some(err.to_string());
                        }
                    }
                }
                Err(err) => {
                    state.error_msg = Some(err);
                }
            }
        }

        if keep_open {
            self.modal = Some(state);
        }
    }
}

impl eframe::App for CottonDataApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.hub.set_egui_ctx(ctx.clone());
        self.process_worker_events();
        self.handle_shortcuts(ctx);

        let p = self.theme.palette();
        self.render_top_bar(ctx, p);
        self.render_sidebar(ctx, p);
        self.render_workspace(ctx, p);
        self.render_modals(ctx, p);
    }
}

fn truncate_display(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn insert_completion(sql: &mut String, cursor_byte: usize, completion: &str) {
    let clamped = cursor_byte.min(sql.len());
    let before = &sql[..clamped];
    let replace_start = before
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '`')))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    sql.replace_range(replace_start..clamped, completion);
}
