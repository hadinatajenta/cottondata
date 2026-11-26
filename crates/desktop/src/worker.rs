use crate::drivers::{kill_running_query, open_session};
use cottondata_core::{ServerProfile, ServerRuntimeInfo, ServerSlotId, SERVER_SLOT_CAPACITY};
use cottondata_grid::PageFilterSort;
use cottondata_introspect::{SchemaCatalog, TableDetail};
use cottondata_driver::{DatabaseSession, DbError, ExplainResult, ProcessEntry, QueryOutcome, ServerHealthMetrics};
use eframe::egui;
use std::sync::mpsc::{self, Receiver, Sender};
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
    },
    ExecuteSql {
        tab_id: u64,
        sql: String,
        max_rows: u32,
    },
    LoadTablePage {
        tab_id: u64,
        schema: String,
        table: String,
        filter_sort: PageFilterSort,
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
        catalog: SchemaCatalog,
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
        catalog: SchemaCatalog,
    },
    TableDetailLoaded {
        schema: String,
        table: String,
        detail: TableDetail,
    },
    QueryFinished {
        slot: ServerSlotId,
        tab_id: u64,
        outcome: QueryOutcome,
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

    pub fn send(&mut self, slot: ServerSlotId, cmd: SlotCmd) {
        let idx = slot.index();
        if self.workers[idx].is_none() {
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
        if let Some(worker) = &self.workers[idx] {
            let _ = worker.tx.send(cmd);
        }
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
    let mut catalog = SchemaCatalog::new();

    while let Ok(cmd) = rx.recv() {
        match cmd {
            SlotCmd::Connect { profile, password } => {
                session = None;
                catalog = SchemaCatalog::new();
                match open_session(&profile, password.as_deref()) {
                    Ok(mut sess) => {
                        if let Err(err) = catalog.introspect_l0(sess.as_mut()) {
                            emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error: err });
                        }
                        if let Some(default_db) = sess.runtime().current_schema.clone() {
                            let _ = catalog.introspect_schema_l1(sess.as_mut(), &default_db);
                        }
                        let runtime = sess.runtime().clone();
                        let snapshot = catalog.snapshot();
                        session = Some(sess);
                        emit(
                            &tx,
                            &ctx,
                            WorkerEvent::Connected {
                                slot,
                                runtime,
                                catalog: snapshot,
                            },
                        );
                    }
                    Err(error) => {
                        emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error });
                    }
                }
            }
            SlotCmd::Disconnect => {
                session = None;
                catalog = SchemaCatalog::new();
                emit(&tx, &ctx, WorkerEvent::Disconnected { slot });
            }
            SlotCmd::Ping => {
                let is_dead = match session.as_mut() {
                    Some(sess) => sess.ping().is_err(),
                    None => false,
                };
                if is_dead {
                    session = None;
                    catalog = SchemaCatalog::new();
                    emit(&tx, &ctx, WorkerEvent::ConnectionLost { slot });
                }
            }
            SlotCmd::LoadSchemaL1 { schema } => {
                let Some(sess) = session.as_mut() else {
                    emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error: DbError::message("Not connected") });
                    continue;
                };
                match catalog.introspect_schema_l1(sess.as_mut(), &schema) {
                    Ok(()) => {
                        let snapshot = catalog.snapshot();
                        emit(&tx, &ctx, WorkerEvent::CatalogUpdated { slot, catalog: snapshot });
                    }
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
                }
            }
            SlotCmd::LoadTableL2 { schema, table } => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, None);
                    continue;
                };
                match catalog.introspect_table_l2(sess.as_mut(), &schema, &table) {
                    Ok(detail) => emit(
                        &tx,
                        &ctx,
                        WorkerEvent::TableDetailLoaded {
                            schema,
                            table,
                            detail,
                        },
                    ),
                    Err(error) => emit(&tx, &ctx, WorkerEvent::Error { slot, tab_id: None, error }),
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
                            session = None;
                            catalog = SchemaCatalog::new();
                            emit(&tx, &ctx, WorkerEvent::ConnectionLost { slot });
                        }
                    }
                }
            }
            SlotCmd::LoadTablePage {
                tab_id,
                schema,
                table,
                filter_sort,
            } => {
                let Some(sess) = session.as_mut() else {
                    emit_not_connected(&tx, &ctx, slot, Some(tab_id));
                    continue;
                };
                let sql = filter_sort.build_select_sql(sess.dialect(), &schema, &table);
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
                            session = None;
                            catalog = SchemaCatalog::new();
                            emit(&tx, &ctx, WorkerEvent::ConnectionLost { slot });
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
                        let _ = catalog.introspect_schema_l1(sess.as_mut(), &schema);
                        let runtime = sess.runtime().clone();
                        let snapshot = catalog.snapshot();
                        emit(&tx, &ctx, WorkerEvent::RuntimeUpdated { slot, runtime });
                        emit(&tx, &ctx, WorkerEvent::CatalogUpdated { slot, catalog: snapshot });
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
                let sql = sess.dialect().kill_process(pid);
                match sess.execute_sql(&sql, 1) {
                    Ok(_) => {
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
