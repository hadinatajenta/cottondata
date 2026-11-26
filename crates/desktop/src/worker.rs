use crate::drivers::{kill_running_query, open_session};
use cottondata_core::{DatabaseEngine, ServerProfile, ServerRuntimeInfo, ServerSlotId, SqlDialect, SERVER_SLOT_CAPACITY};
use cottondata_driver::{DatabaseSession, DbError, ExplainResult, ProcessEntry, QueryOutcome, ServerHealthMetrics};
use cottondata_grid::{
    stream_import_batches, ColumnMapping, ExportFormat, ImportFormat, KeysetSeek, PageFilterSort, StreamExporter,
    TargetColumnSpec,
};
use cottondata_introspect::{compare_schema_snapshots, snapshot_from_catalog, SchemaCatalog, SchemaDiff, TableDetail};
use eframe::egui;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub enum SlotCmd {
    Connect {
        profile: ServerProfile,
        password: Option<String>,
    },
    Disconnect,
    Ping,
    LoadSchemaL1 {
        schema: String,
    },
    LoadTableL2 {
        schema: String,
        table: String,
        show_in_inspector: bool,
    },
    ExecuteSql {
        tab_id: u64,
        sql: String,
        max_rows: u32,
    },
    FetchMoreRows {
        tab_id: u64,
        sql: String,
        max_rows: u32,
    },
    ExecuteScript {
        tab_id: u64,
        statements: Vec<String>,
        max_rows: u32,
    },
    LoadTablePage {
        tab_id: u64,
        schema: String,
        table: String,
        filter_sort: PageFilterSort,
        default_order_column: Option<String>,
        keyset_seek: Option<KeysetSeek>,
    },
    CountTableRows {
        tab_id: u64,
        schema: String,
        table: String,
        filter_sort: PageFilterSort,
    },
    SubmitMutations {
        tab_id: u64,
        statements: Vec<String>,
    },
    FetchFullCell {
        tab_id: u64,
        row: usize,
        column: usize,
        schema: String,
        table: String,
        column_name: String,
        pk_where: String,
    },
    ExplainQuery {
        tab_id: u64,
        sql: String,
        analyze: bool,
    },
    SetSchema(String),
    SetAutocommit(bool),
    BeginTx,
    CommitTx,
    RollbackTx,
    FetchProcesslist,
    KillProcess(u64),
    FetchHealth,
}

pub enum WorkerEvent {
    Connected {
        slot: ServerSlotId,
        runtime: ServerRuntimeInfo,
        catalog: Arc<SchemaCatalog>,
    },
    Disconnected {
        slot: ServerSlotId,
    },
    ConnectionLost {
        slot: ServerSlotId,
    },
    Error {
        slot: ServerSlotId,
        tab_id: Option<u64>,
        error: DbError,
    },
    CatalogUpdated {
        slot: ServerSlotId,
        catalog: Arc<SchemaCatalog>,
    },
    TableDetailLoaded {
        slot: ServerSlotId,
        schema: String,
        table: String,
        detail: TableDetail,
        show_in_inspector: bool,
    },
    QueryFinished {
        slot: ServerSlotId,
        tab_id: u64,
        outcome: QueryOutcome,
        runtime: ServerRuntimeInfo,
    },
    MoreRowsLoaded {
        slot: ServerSlotId,
        tab_id: u64,
        outcome: QueryOutcome,
        runtime: ServerRuntimeInfo,
    },
    ScriptFinished {
        slot: ServerSlotId,
        tab_id: u64,
        outcomes: Vec<QueryOutcome>,
        runtime: ServerRuntimeInfo,
    },
    RowCountLoaded {
        tab_id: u64,
        total_rows: u64,
    },
    MutationsSubmitted {
        slot: ServerSlotId,
        tab_id: u64,
        statement_count: usize,
        elapsed_ms: u64,
    },
    FullCellLoaded {
        tab_id: u64,
        row: usize,
        column: usize,
        value: Option<String>,
    },
    ExplainFinished {
        tab_id: u64,
        result: ExplainResult,
    },
    RuntimeUpdated {
        slot: ServerSlotId,
        runtime: ServerRuntimeInfo,
    },
    ProcesslistLoaded {
        slot: ServerSlotId,
        entries: Vec<ProcessEntry>,
    },
    HealthLoaded {
        slot: ServerSlotId,
        metrics: ServerHealthMetrics,
    },
    TestConnectionFinished {
        request_id: u64,
        result: Result<String, String>,
        elapsed_ms: u64,
    },
    ExportProgress {
        export_id: u64,
        rows_written: u64,
    },
    ExportFinished {
        export_id: u64,
        path: PathBuf,
        result: Result<u64, String>,
        cancelled: bool,
    },
    ImportProgress {
        rows_imported: u64,
    },
    ImportFinished {
        slot: ServerSlotId,
        schema: String,
        table: String,
        result: Result<u64, String>,
        cancelled: bool,
    },
    SchemaCompared {
        result: Result<SchemaDiff, String>,
    },
    SchemaErLoaded {
        slot: ServerSlotId,
        schema: String,
        relations: Vec<(String, String, String, String, String)>,
    },
    UpdateCheckFinished(Result<crate::updater::ReleaseInfo, String>),
    UpdateProgress {
        downloaded: u64,
        total: u64,
    },
    UpdateReady {
        info: crate::updater::ReleaseInfo,
        file_path: PathBuf,
    },
    UpdateFailed(String),
}

struct SlotWorker {
    tx: Sender<SlotCmd>,
    _handle: JoinHandle<()>,
}

pub struct WorkerHub {
    workers: [Option<SlotWorker>; SERVER_SLOT_CAPACITY],
    event_tx: Sender<WorkerEvent>,
    event_rx: Receiver<WorkerEvent>,
    repaint_ctx: Option<egui::Context>,
}

impl WorkerHub {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            workers: std::array::from_fn(|_| None),
            event_tx,
            event_rx,
            repaint_ctx: None,
        }
    }

    pub fn set_egui_ctx(&mut self, ctx: egui::Context) {
        if self.repaint_ctx.is_none() {
            self.repaint_ctx = Some(ctx);
        }
    }

    fn spawn_slot_worker(&mut self, slot: ServerSlotId) {
        let idx = slot.index();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let handle = thread::Builder::new()
            .name(format!("cottondata-slot-{}", slot.display_number()))
            .stack_size(1024 * 1024)
            .spawn(move || slot_worker_loop(slot, cmd_rx, ev_tx, ctx))
            .expect("spawn database slot worker");
        self.workers[idx] = Some(SlotWorker {
            tx: cmd_tx,
            _handle: handle,
        });
    }

    pub fn disconnect_slot(&mut self, slot: ServerSlotId) {
        if let Some(worker) = self.workers[slot.index()].take() {
            let _ = worker.tx.send(SlotCmd::Disconnect);
        }
    }

    pub fn send(&mut self, slot: ServerSlotId, cmd: SlotCmd) {
        if matches!(cmd, SlotCmd::Disconnect) {
            self.disconnect_slot(slot);
            return;
        }
        let idx = slot.index();
        if self.workers[idx].is_none() {
            self.spawn_slot_worker(slot);
        }
        let send_failed = match &self.workers[idx] {
            Some(worker) => worker.tx.send(cmd).err(),
            None => None,
        };
        if let Some(mpsc::SendError(unsent)) = send_failed {
            self.workers[idx] = None;
            self.spawn_slot_worker(slot);
            if let Some(worker) = &self.workers[idx] {
                let _ = worker.tx.send(unsent);
            }
        }
    }

    pub fn check_for_updates_async(&self) {
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let _ = thread::Builder::new()
            .name("cottondata-update-check".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                let result = crate::updater::check_latest_release();
                emit(&ev_tx, &ctx, WorkerEvent::UpdateCheckFinished(result));
            });
    }

    pub fn check_for_updates_delayed_async(&self, delay: std::time::Duration) {
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let _ = thread::Builder::new()
            .name("cottondata-update-check".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                thread::sleep(delay);
                let result = crate::updater::check_latest_release();
                emit(&ev_tx, &ctx, WorkerEvent::UpdateCheckFinished(result));
            });
    }

    pub fn download_update_async(&self, info: crate::updater::ReleaseInfo, cancel: Arc<AtomicBool>) {
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let _ = thread::Builder::new()
            .name("cottondata-update-dl".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let mut last_emit = std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_millis(100))
                    .unwrap_or_else(std::time::Instant::now);
                let res = crate::updater::download_and_verify_asset(&info, &cancel, |downloaded, total| {
                    let is_edge = downloaded == 0 || (total > 0 && downloaded >= total);
                    if is_edge || last_emit.elapsed() >= std::time::Duration::from_millis(50) {
                        last_emit = std::time::Instant::now();
                        emit(
                            &ev_tx,
                            &ctx,
                            WorkerEvent::UpdateProgress { downloaded, total },
                        );
                    }
                });
                match res {
                    Ok(file_path) => emit(&ev_tx, &ctx, WorkerEvent::UpdateReady { info, file_path }),
                    Err(err) => emit(&ev_tx, &ctx, WorkerEvent::UpdateFailed(err)),
                }
            });
    }

    pub fn cancel_query_async(
        &self,
        slot: ServerSlotId,
        profile: ServerProfile,
        password: Option<String>,
        target_conn_id: u32,
    ) {
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let _ = thread::Builder::new()
            .name("cottondata-kill".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                if let Err(err) = kill_running_query(&profile, password.as_deref(), target_conn_id) {
                    let _ = ev_tx.send(WorkerEvent::Error {
                        slot,
                        tab_id: None,
                        error: err,
                    });
                }
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
    }

    pub fn test_connection_async(&self, request_id: u64, profile: ServerProfile, password: Option<String>) {
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let _ = thread::Builder::new()
            .name("cottondata-test-conn".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let start = std::time::Instant::now();
                let result = match open_session(&profile, password.as_deref()) {
                    Ok(mut session) => {
                        let summary = format!("{} {}", profile.engine.label(), session.runtime().version);
                        let _ = session.ping();
                        Ok(summary)
                    }
                    Err(err) => Err(err.to_string()),
                };
                let elapsed_ms = start.elapsed().as_millis() as u64;
                emit(
                    &ev_tx,
                    &ctx,
                    WorkerEvent::TestConnectionFinished {
                        request_id,
                        result,
                        elapsed_ms,
                    },
                );
            });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stream_export_async(
        &self,
        export_id: u64,
        profile: ServerProfile,
        password: Option<String>,
        active_schema: Option<String>,
        sql: String,
        format: ExportFormat,
        table_name: String,
        path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
    ) {
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let _ = thread::Builder::new()
            .name("cottondata-stream-export".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let dialect = profile.engine.dialect();
                let mut cancelled = false;
                let result: Result<u64, String> = (|| {
                    let mut session = open_session(&profile, password.as_deref()).map_err(|e| e.to_string())?;
                    if let Some(schema) = active_schema.as_deref().filter(|s| !s.is_empty()) {
                        session.set_schema(schema).map_err(|e| e.to_string())?;
                    }
                    let file = File::create(&path).map_err(|e| format!("Could not create export file: {e}"))?;
                    let mut writer_slot = Some(BufWriter::with_capacity(64 * 1024, file));
                    let exporter: std::cell::RefCell<Option<StreamExporter<BufWriter<File>>>> =
                        std::cell::RefCell::new(None);
                    let mut last_progress = std::time::Instant::now();

                    session
                        .stream_query(
                            &sql,
                            &mut |columns| {
                                let w = writer_slot
                                    .take()
                                    .ok_or_else(|| "Export writer already initialized".to_string())?;
                                let exp = StreamExporter::new(
                                    w,
                                    format,
                                    columns.to_vec(),
                                    dialect,
                                    table_name.clone(),
                                )
                                .map_err(|e| e.to_string())?;
                                *exporter.borrow_mut() = Some(exp);
                                Ok(())
                            },
                            &mut |row| {
                                if cancel_flag.load(Ordering::Relaxed) {
                                    cancelled = true;
                                    return Ok(false);
                                }
                                let mut guard = exporter.borrow_mut();
                                let Some(exp) = guard.as_mut() else {
                                    return Err("Missing column metadata before rows".to_string());
                                };
                                exp.write_row(row).map_err(|e| e.to_string())?;
                                let written = exp.rows_written();
                                if written.is_multiple_of(2000)
                                    || last_progress.elapsed() >= std::time::Duration::from_millis(150)
                                {
                                    last_progress = std::time::Instant::now();
                                    emit(
                                        &ev_tx,
                                        &ctx,
                                        WorkerEvent::ExportProgress {
                                            export_id,
                                            rows_written: written,
                                        },
                                    );
                                }
                                Ok(true)
                            },
                        )
                        .map_err(|e| e.to_string())?;

                    match exporter.into_inner() {
                        Some(exp) => exp.finish().map_err(|e| e.to_string()),
                        None => Ok(0),
                    }
                })();

                emit(
                    &ev_tx,
                    &ctx,
                    WorkerEvent::ExportFinished {
                        export_id,
                        path,
                        result,
                        cancelled,
                    },
                );
            });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn import_file_async(
        &self,
        slot: ServerSlotId,
        profile: ServerProfile,
        password: Option<String>,
        schema: String,
        table: String,
        path: PathBuf,
        format: ImportFormat,
        mappings: Vec<ColumnMapping>,
        target_columns: Vec<TargetColumnSpec>,
        batch_size: usize,
        cancel_flag: Arc<AtomicBool>,
    ) {
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let _ = thread::Builder::new()
            .name("cottondata-import".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let dialect = profile.engine.dialect();
                let mut cancelled = false;
                let result: Result<u64, String> = (|| {
                    let mut session = open_session(&profile, password.as_deref()).map_err(|e| e.to_string())?;
                    if !schema.is_empty() {
                        session.set_schema(&schema).map_err(|e| e.to_string())?;
                    }
                    session.begin_transaction().map_err(|e| e.to_string())?;
                    let mut total = 0u64;
                    let stream_res = stream_import_batches(
                        &path,
                        format,
                        dialect,
                        &schema,
                        &table,
                        &mappings,
                        &target_columns,
                        batch_size,
                        |sql, batch_rows| {
                            if cancel_flag.load(Ordering::Relaxed) {
                                cancelled = true;
                                return Ok(false);
                            }
                            session.execute_sql(&sql, 1).map_err(|e| e.to_string())?;
                            total += batch_rows as u64;
                            emit(&ev_tx, &ctx, WorkerEvent::ImportProgress { rows_imported: total });
                            Ok(true)
                        },
                    );
                    match stream_res {
                        Ok(_) if cancelled => {
                            let _ = session.rollback();
                            Ok(total)
                        }
                        Ok(_) => {
                            session.commit().map_err(|e| e.to_string())?;
                            Ok(total)
                        }
                        Err(err) => {
                            let _ = session.rollback();
                            Err(err)
                        }
                    }
                })();
                emit(
                    &ev_tx,
                    &ctx,
                    WorkerEvent::ImportFinished {
                        slot,
                        schema,
                        table,
                        result,
                        cancelled,
                    },
                );
            });
    }

    pub fn compare_schemas_async(
        &self,
        src_profile: ServerProfile,
        src_password: Option<String>,
        src_schema: String,
        dst_profile: ServerProfile,
        dst_password: Option<String>,
        dst_schema: String,
    ) {
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let _ = thread::Builder::new()
            .name("cottondata-schema-compare".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let result = (|| -> Result<SchemaDiff, String> {
                    let src_label = format!("{} / {}", src_profile.name, src_schema);
                    let dst_label = format!("{} / {}", dst_profile.name, dst_schema);
                    let mut src_sess = open_session(&src_profile, src_password.as_deref())
                        .map_err(|e| format!("Source connection failed: {e}"))?;
                    let mut dst_sess = open_session(&dst_profile, dst_password.as_deref())
                        .map_err(|e| format!("Target connection failed: {e}"))?;
                    let mut src_cat = SchemaCatalog::new();
                    let mut dst_cat = SchemaCatalog::new();
                    src_cat.introspect_l0(src_sess.as_mut()).map_err(|e| e.to_string())?;
                    src_cat
                        .introspect_schema_l1(src_sess.as_mut(), &src_schema)
                        .map_err(|e| e.to_string())?;
                    dst_cat.introspect_l0(dst_sess.as_mut()).map_err(|e| e.to_string())?;
                    dst_cat
                        .introspect_schema_l1(dst_sess.as_mut(), &dst_schema)
                        .map_err(|e| e.to_string())?;

                    let src_table_names: Vec<String> = snapshot_from_catalog(&src_cat, &src_schema)
                        .into_iter()
                        .map(|t| t.name)
                        .collect();
                    let dst_table_names: Vec<String> = snapshot_from_catalog(&dst_cat, &dst_schema)
                        .into_iter()
                        .map(|t| t.name)
                        .collect();
                    if src_table_names.len() <= 120 && dst_table_names.len() <= 120 {
                        for name in &src_table_names {
                            let _ = src_cat.introspect_table_l2(src_sess.as_mut(), &src_schema, name);
                        }
                        for name in &dst_table_names {
                            let _ = dst_cat.introspect_table_l2(dst_sess.as_mut(), &dst_schema, name);
                        }
                    }

                    let src_snap = snapshot_from_catalog(&src_cat, &src_schema);
                    let dst_snap = snapshot_from_catalog(&dst_cat, &dst_schema);
                    Ok(compare_schema_snapshots(src_label, dst_label, &src_snap, &dst_snap))
                })();
                emit(&ev_tx, &ctx, WorkerEvent::SchemaCompared { result });
            });
    }

    pub fn load_schema_er_async(
        &self,
        slot: ServerSlotId,
        profile: ServerProfile,
        password: Option<String>,
        schema: String,
    ) {
        let ev_tx = self.event_tx.clone();
        let ctx = self.repaint_ctx.clone();
        let _ = thread::Builder::new()
            .name("cottondata-er".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let mut relations = Vec::new();
                if let Ok(mut session) = open_session(&profile, password.as_deref()) {
                    let sql = foreign_key_relations_sql(profile.engine, &schema);
                    if let Ok(QueryOutcome::Rows(page)) = session.execute_sql(&sql, 4000) {
                        for row in 0..page.row_count {
                            if let (Some(c), Some(st), Some(sc), Some(rt), Some(rc)) = (
                                page.cell_str(row, 0),
                                page.cell_str(row, 1),
                                page.cell_str(row, 2),
                                page.cell_str(row, 3),
                                page.cell_str(row, 4),
                            ) {
                                relations.push((
                                    c.to_string(),
                                    st.to_string(),
                                    sc.to_string(),
                                    rt.to_string(),
                                    rc.to_string(),
                                ));
                            }
                        }
                    }
                }
                emit(
                    &ev_tx,
                    &ctx,
                    WorkerEvent::SchemaErLoaded {
                        slot,
                        schema,
                        relations,
                    },
                );
            });
    }

    pub fn poll_events(&self) -> Vec<WorkerEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

fn emit(tx: &Sender<WorkerEvent>, ctx: &Option<egui::Context>, ev: WorkerEvent) {
    let _ = tx.send(ev);
    if let Some(c) = ctx {
        c.request_repaint();
    }
}

fn emit_not_connected(tx: &Sender<WorkerEvent>, ctx: &Option<egui::Context>, slot: ServerSlotId, tab_id: Option<u64>) {
    emit(
        tx,
        ctx,
        WorkerEvent::Error {
            slot,
            tab_id,
            error: DbError::message("Not connected to this server"),
        },
    );
}

fn slot_worker_loop(
    slot: ServerSlotId,
    rx: Receiver<SlotCmd>,
    tx: Sender<WorkerEvent>,
    ctx: Option<egui::Context>,
) {
    let mut session: Option<Box<dyn DatabaseSession>> = None;
    let mut catalog = Arc::new(SchemaCatalog::new());

    while let Ok(cmd) = rx.recv() {
        match cmd {
            SlotCmd::Connect { profile, password } => {
                session = None;
                catalog = Arc::new(SchemaCatalog::new());
                match open_session(&profile, password.as_deref()) {
                    Ok(mut sess) => {
                        let mut fresh_catalog = SchemaCatalog::new();
                        if let Err(err) = fresh_catalog.introspect_l0(sess.as_mut()) {
                            emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error: err });
                        }
                        if let Some(default_db) = sess.runtime().current_schema.clone() {
                            let _ = fresh_catalog.introspect_schema_l1(sess.as_mut(), &default_db);
                        }
                        let runtime = sess.runtime().clone();
                        let shared = Arc::new(fresh_catalog);
                        catalog = Arc::clone(&shared);
                        session = Some(sess);
                        emit(
                            &tx,
                            &ctx,
                            WorkerEvent::Connected {
                                slot,
                                runtime,
                                catalog: shared,
                            },
                        );
                    }
                    Err(error) => {
                        emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error });
                    }
                }
            }
            SlotCmd::Disconnect => {
                drop(session);
                drop(catalog);
                emit(&tx, &ctx, WorkerEvent::Disconnected { slot });
                break;
            }
            SlotCmd::Ping => {
                let is_dead = match session.as_mut() {
                    Some(sess) => sess.ping().is_err(),
                    None => false,
                };
                if is_dead {
                    drop(session);
                    drop(catalog);
                    emit(&tx, &ctx, WorkerEvent::ConnectionLost { slot });
                    break;
                }
            }
            SlotCmd::LoadSchemaL1 { schema } => {
                let Some(sess) = session.as_mut() else {
                    emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error: DbError::message("Not connected") });
                    continue;
                };
                match sess.load_schema_objects(&schema) {
                    Ok(objects) => {
                        let mut updated = Arc::try_unwrap(catalog).unwrap_or_else(|arc| arc.snapshot());
                        updated.apply_schema_objects(&schema, objects);
                        let shared = Arc::new(updated);
                        catalog = Arc::clone(&shared);
                        emit(&tx, &ctx, WorkerEvent::CatalogUpdated { slot, catalog: shared });
                    }
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
            SlotCmd::LoadTableL2 {
                schema,
                table,
                show_in_inspector,
            } => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match sess.load_table_detail(&schema, &table) {
                    Ok(detail) => emit(
                        &tx,
                        &ctx,
                        WorkerEvent::TableDetailLoaded {
                            slot,
                            schema,
                            table,
                            detail,
                            show_in_inspector,
                        },
                    ),
                    Err(error) => {
                        if show_in_inspector {
                            emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error });
                        }
                    }
                }
            }
            SlotCmd::ExecuteSql { tab_id, sql, max_rows } => {
                let Some(sess) = session.as_mut() else {
                    emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: Some(tab_id), error: DbError::message("Connect to this server first.") });
                    continue;
                };
                match sess.execute_sql(&sql, max_rows) {
                    Ok(outcome) => {
                        let runtime = sess.runtime().clone();
                        emit(
                            &tx,
                            &ctx,
                            WorkerEvent::QueryFinished {
                                slot,
                                tab_id,
                                outcome,
                                runtime,
                            },
                        );
                    }
                    Err(error) => {
                        let lost = error.is_connection_lost;
                        emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: Some(tab_id), error });
                        if lost {
                            drop(session);
                            drop(catalog);
                            emit(&tx, &ctx, WorkerEvent::ConnectionLost { slot });
                            break;
                        }
                    }
                }
            }
            SlotCmd::FetchMoreRows { tab_id, sql, max_rows } => {
                let Some(sess) = session.as_mut() else {
                    emit(
                        &tx,
                        &ctx,
                        WorkerEvent::Error {
                            slot,
                            tab_id: Some(tab_id),
                            error: DbError::message("Connect to this server first."),
                        },
                    );
                    continue;
                };
                match sess.execute_sql(&sql, max_rows) {
                    Ok(outcome) => {
                        let runtime = sess.runtime().clone();
                        emit(
                            &tx,
                            &ctx,
                            WorkerEvent::MoreRowsLoaded {
                                slot,
                                tab_id,
                                outcome,
                                runtime,
                            },
                        );
                    }
                    Err(error) => {
                        let lost = error.is_connection_lost;
                        emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: Some(tab_id), error });
                        if lost {
                            drop(session);
                            drop(catalog);
                            emit(&tx, &ctx, WorkerEvent::ConnectionLost { slot });
                            break;
                        }
                    }
                }
            }
            SlotCmd::ExecuteScript {
                tab_id,
                statements,
                max_rows,
            } => {
                let Some(sess) = session.as_mut() else {
                    emit(
                        &tx,
                        &ctx,
                        WorkerEvent::Error {
                            slot,
                            tab_id: Some(tab_id),
                            error: DbError::message("Connect to this server first."),
                        },
                    );
                    continue;
                };
                let mut outcomes = Vec::with_capacity(statements.len());
                let mut script_error = None;
                for sql in statements {
                    let stmt_limit = if max_rows != cottondata_core::UNLIMITED_ROWS
                        && cottondata_sql::has_explicit_row_limit(&sql)
                    {
                        cottondata_core::UNLIMITED_ROWS
                    } else {
                        max_rows
                    };
                    match sess.execute_sql(&sql, stmt_limit) {
                        Ok(outcome) => outcomes.push(outcome),
                        Err(error) => {
                            script_error = Some(error);
                            break;
                        }
                    }
                }
                let runtime = sess.runtime().clone();
                if !outcomes.is_empty() {
                    emit(
                        &tx,
                        &ctx,
                        WorkerEvent::ScriptFinished {
                            slot,
                            tab_id,
                            outcomes,
                            runtime,
                        },
                    );
                }
                if let Some(error) = script_error {
                    let lost = error.is_connection_lost;
                    emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: Some(tab_id), error });
                    if lost {
                        drop(session);
                        drop(catalog);
                        emit(&tx, &ctx, WorkerEvent::ConnectionLost { slot });
                        break;
                    }
                }
            }
            SlotCmd::LoadTablePage {
                tab_id,
                schema,
                table,
                filter_sort,
                default_order_column,
                keyset_seek,
            } => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, Some(tab_id));
                    continue;
                };
                let fetch_limit = filter_sort.page_size.max(1).saturating_add(1);
                let sql = match keyset_seek.as_ref() {
                    Some(seek) => filter_sort.build_keyset_select_sql(sess.dialect(), &schema, &table, seek, fetch_limit),
                    None => filter_sort.build_select_sql_with_default_order(
                        sess.dialect(),
                        &schema,
                        &table,
                        default_order_column.as_deref(),
                    ),
                };
                match sess.execute_sql(&sql, filter_sort.page_size) {
                    Ok(outcome) => {
                        let runtime = sess.runtime().clone();
                        emit(
                            &tx,
                            &ctx,
                            WorkerEvent::QueryFinished {
                                slot,
                                tab_id,
                                outcome,
                                runtime,
                            },
                        );
                    }
                    Err(error) => {
                        let lost = error.is_connection_lost;
                        emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: Some(tab_id), error });
                        if lost {
                            drop(session);
                            drop(catalog);
                            emit(&tx, &ctx, WorkerEvent::ConnectionLost { slot });
                            break;
                        }
                    }
                }
            }
            SlotCmd::CountTableRows {
                tab_id,
                schema,
                table,
                filter_sort,
            } => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, Some(tab_id));
                    continue;
                };
                let sql = filter_sort.build_count_sql(sess.dialect(), &schema, &table);
                match sess.execute_sql(&sql, 1) {
                    Ok(QueryOutcome::Rows(page)) => {
                        let count = page
                            .cell_str(0, 0)
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);
                        emit(&tx, &ctx, WorkerEvent::RowCountLoaded { tab_id, total_rows: count });
                    }
                    Ok(_) => emit(
                        &tx,
                        &ctx,
                        WorkerEvent::Error {
                            slot,
                            tab_id: Some(tab_id),
                            error: DbError::message("Row count query returned no rows"),
                        },
                    ),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: Some(tab_id), error }),
                }
            }
            SlotCmd::SubmitMutations { tab_id, statements } => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, Some(tab_id));
                    continue;
                };
                let count = statements.len();
                match sess.execute_atomic_batch(&statements) {
                    Ok(elapsed_ms) => emit(
                        &tx,
                        &ctx,
                        WorkerEvent::MutationsSubmitted {
                            slot,
                            tab_id,
                            statement_count: count,
                            elapsed_ms,
                        },
                    ),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: Some(tab_id), error }),
                }
            }
            SlotCmd::FetchFullCell {
                tab_id,
                row,
                column,
                schema,
                table,
                column_name,
                pk_where,
            } => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, Some(tab_id));
                    continue;
                };
                let dialect = sess.dialect();
                let fq_table = dialect.qualified_table(&schema, &table);
                let col_ident = dialect.quote_ident(&column_name);
                let sql = dialect.select_single_value(&col_ident, &fq_table, &pk_where);
                match sess.execute_sql(&sql, 1) {
                    Ok(QueryOutcome::Rows(page)) => {
                        let value = page.cell_str(0, 0).map(ToOwned::to_owned);
                        emit(
                            &tx,
                            &ctx,
                            WorkerEvent::FullCellLoaded {
                                tab_id,
                                row,
                                column,
                                value,
                            },
                        );
                    }
                    Ok(_) => emit(
                        &tx,
                        &ctx,
                        WorkerEvent::Error {
                            slot,
                            tab_id: Some(tab_id),
                            error: DbError::message("Row not found when loading full cell value"),
                        },
                    ),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: Some(tab_id), error }),
                }
            }
            SlotCmd::ExplainQuery { tab_id, sql, analyze } => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, Some(tab_id));
                    continue;
                };
                match sess.explain(&sql, analyze) {
                    Ok(result) => emit(&tx, &ctx, WorkerEvent::ExplainFinished { tab_id, result }),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: Some(tab_id), error }),
                }
            }
            SlotCmd::SetSchema(schema) => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match sess.set_schema(&schema) {
                    Ok(()) => {
                        let objects = sess.load_schema_objects(&schema).ok();
                        let runtime = sess.runtime().clone();
                        let shared = if let Some(objects) = objects {
                            let mut updated = Arc::try_unwrap(catalog).unwrap_or_else(|arc| arc.snapshot());
                            updated.apply_schema_objects(&schema, objects);
                            let arc = Arc::new(updated);
                            catalog = Arc::clone(&arc);
                            arc
                        } else {
                            Arc::clone(&catalog)
                        };
                        emit(&tx, &ctx, WorkerEvent::RuntimeUpdated { slot, runtime });
                        emit(&tx, &ctx, WorkerEvent::CatalogUpdated { slot, catalog: shared });
                    }
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
            SlotCmd::SetAutocommit(enabled) => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match sess.set_autocommit(enabled) {
                    Ok(()) => emit(&tx, &ctx, WorkerEvent::RuntimeUpdated { slot, runtime: sess.runtime().clone() }),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
            SlotCmd::BeginTx => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match sess.begin_transaction() {
                    Ok(()) => emit(&tx, &ctx, WorkerEvent::RuntimeUpdated { slot, runtime: sess.runtime().clone() }),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
            SlotCmd::CommitTx => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match sess.commit() {
                    Ok(()) => emit(&tx, &ctx, WorkerEvent::RuntimeUpdated { slot, runtime: sess.runtime().clone() }),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
            SlotCmd::RollbackTx => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match sess.rollback() {
                    Ok(()) => emit(&tx, &ctx, WorkerEvent::RuntimeUpdated { slot, runtime: sess.runtime().clone() }),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
            SlotCmd::FetchProcesslist => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match sess.fetch_processlist() {
                    Ok(entries) => emit(&tx, &ctx, WorkerEvent::ProcesslistLoaded { slot, entries }),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
            SlotCmd::KillProcess(pid) => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match sess.kill_process(pid) {
                    Ok(()) => {
                        if let Ok(entries) = sess.fetch_processlist() {
                            emit(&tx, &ctx, WorkerEvent::ProcesslistLoaded { slot, entries });
                        }
                    }
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
            SlotCmd::FetchHealth => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match sess.fetch_health_metrics() {
                    Ok(metrics) => emit(&tx, &ctx, WorkerEvent::HealthLoaded { slot, metrics }),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
        }
    }
}

fn foreign_key_relations_sql(engine: DatabaseEngine, schema: &str) -> String {
    match engine {
        DatabaseEngine::MySql => {
            let escaped = schema.replace('\'', "''");
            format!(
                "SELECT constraint_name, table_name, column_name, \
                        referenced_table_name, referenced_column_name \
                 FROM information_schema.key_column_usage \
                 WHERE table_schema = '{escaped}' \
                   AND referenced_table_schema = '{escaped}' \
                   AND referenced_table_name IS NOT NULL \
                 ORDER BY table_name, constraint_name, ordinal_position"
            )
        }
        DatabaseEngine::SqlServer => {
            let db = format!("[{}]", schema.replace(']', "]]"));
            format!(
                "SELECT fk.name, ps.name + '.' + pt.name, pc.name, \
                        rs.name + '.' + rt.name, rc.name \
                 FROM {db}.sys.foreign_keys fk \
                 JOIN {db}.sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
                 JOIN {db}.sys.objects pt ON pt.object_id = fkc.parent_object_id \
                 JOIN {db}.sys.schemas ps ON ps.schema_id = pt.schema_id \
                 JOIN {db}.sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
                 JOIN {db}.sys.objects rt ON rt.object_id = fkc.referenced_object_id \
                 JOIN {db}.sys.schemas rs ON rs.schema_id = rt.schema_id \
                 JOIN {db}.sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
                 ORDER BY pt.name, fk.name"
            )
        }
        DatabaseEngine::PostgreSql => {
            let literal = SqlDialect::PostgreSql.string_literal(schema);
            format!(
                "SELECT con.conname::text, sc.relname::text, a.attname::text, fc.relname::text, fa.attname::text \
                 FROM pg_constraint con \
                 CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(local_attnum, foreign_attnum, ordinal) \
                 JOIN pg_class sc ON sc.oid = con.conrelid \
                 JOIN pg_namespace sn ON sn.oid = sc.relnamespace \
                 JOIN pg_class fc ON fc.oid = con.confrelid \
                 JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
                 JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.local_attnum \
                 JOIN pg_attribute fa ON fa.attrelid = con.confrelid AND fa.attnum = k.foreign_attnum \
                 WHERE con.contype = 'f' AND sn.nspname = {literal} AND fn.nspname = {literal} \
                 ORDER BY sc.relname, con.conname, k.ordinal"
            )
        }
        DatabaseEngine::CockroachDb => {
            let literal = SqlDialect::CockroachDb.string_literal(schema);
            format!(
                "SELECT rc.constraint_name, kcu.table_name, kcu.column_name, pk.table_name, pk.column_name \
                 FROM information_schema.referential_constraints rc \
                 JOIN information_schema.key_column_usage kcu \
                   ON kcu.constraint_schema = rc.constraint_schema AND kcu.constraint_name = rc.constraint_name \
                 JOIN information_schema.key_column_usage pk \
                   ON pk.constraint_schema = rc.unique_constraint_schema \
                  AND pk.constraint_name = rc.unique_constraint_name \
                  AND pk.ordinal_position = kcu.position_in_unique_constraint \
                 WHERE kcu.table_schema = {literal} AND pk.table_schema = {literal} \
                 ORDER BY kcu.table_name, rc.constraint_name, kcu.ordinal_position"
            )
        }
        DatabaseEngine::Sqlite => {
            let schema_ref = SqlDialect::Sqlite.quote_ident(if schema.is_empty() { "main" } else { schema });
            let schema_literal = SqlDialect::Sqlite.string_literal(if schema.is_empty() { "main" } else { schema });
            format!(
                "SELECT 'fk_' || m.name || '_' || f.id, m.name, f.\"from\", f.\"table\", f.\"to\" \
                 FROM {schema_ref}.sqlite_master m \
                 JOIN pragma_foreign_key_list(m.name, {schema_literal}) f \
                 WHERE m.type = 'table' \
                 ORDER BY m.name, f.id, f.seq"
            )
        }
    }
}
