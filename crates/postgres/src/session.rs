use crate::cancel;
use crate::connection::first_value;
use crate::errors::postgres_error_for;
use crate::introspection::{self, Flavor};
use crate::metadata::MetadataCache;
use crate::monitoring;
use crate::table_detail;
use crate::query::{execute_statement, transaction_effect, ExecutionOptions, OverflowCanceller, TransactionEffect};
use crate::streaming::stream_statement;
use cottondata_core::{ServerRuntimeInfo, SqlDialect};
use cottondata_driver::{
    DatabaseSession, DbError, ExplainResult, ProcessEntry, QueryOutcome, SchemaObjects, SchemaSummary,
    ServerHealthMetrics, SshTunnel, TableDetail,
};
use cottondata_grid::ColumnMeta;
use cottondata_sql::{analyze_statement, WriteClass};
use std::time::Instant;
use tokio::runtime::Runtime;
use tokio_postgres::Client;

const BATCH_SAVEPOINT: &str = "cottondata_batch";

pub struct PostgresSession {
    pub(crate) executor: Runtime,
    pub(crate) client: Client,
    pub(crate) tunnel: Option<SshTunnel>,
    pub(crate) runtime: ServerRuntimeInfo,
    pub(crate) flavor: Flavor,
    pub(crate) profile_id: String,
    pub(crate) read_only: bool,
    pub(crate) is_cursor_supported: bool,
    pub(crate) metadata: MetadataCache,
    pub(crate) cockroach_query_ids: Vec<String>,
    pub(crate) overflow_canceller: Option<Box<OverflowCanceller>>,
}

impl PostgresSession {
    pub(crate) fn run_batch(&mut self, sql: &str) -> Result<(), DbError> {
        let Self { executor, client, .. } = self;
        executor
            .block_on(client.batch_execute(sql))
            .map_err(|error| postgres_error_for(error, Some(sql)))
    }

    fn ensure_writable(&self, sql: &str) -> Result<(), DbError> {
        if self.read_only && !analyze_statement(sql).allowed_in_read_only() {
            return Err(DbError::message(
                "Blocked by server Read-Only protection. Disable Read-Only on this server slot to execute writes.",
            ));
        }
        Ok(())
    }

    fn begin_implicit_transaction_if_needed(&mut self, sql: &str) -> Result<(), DbError> {
        if self.runtime.autocommit || self.runtime.in_transaction || transaction_effect(sql) != TransactionEffect::None {
            return Ok(());
        }
        self.run_batch("BEGIN")?;
        self.runtime.in_transaction = true;
        Ok(())
    }

    fn track_statement_effects(&mut self, sql: &str) {
        match transaction_effect(sql) {
            TransactionEffect::Begins => self.runtime.in_transaction = true,
            TransactionEffect::Ends => self.runtime.in_transaction = false,
            TransactionEffect::None => {}
        }
        let class = analyze_statement(sql).class;
        if matches!(class, WriteClass::SchemaMutation | WriteClass::Destructive) {
            self.metadata.clear();
        }
        let is_session_setting = sql.trim_start().get(..4).is_some_and(|head| head.eq_ignore_ascii_case("SET "));
        if is_session_setting {
            self.refresh_current_schema();
        }
    }

    fn refresh_current_schema(&mut self) {
        let Self {
            executor,
            client,
            runtime,
            ..
        } = self;
        if let Ok(messages) = executor.block_on(client.simple_query("SELECT current_schema()")) {
            runtime.current_schema = first_value(&messages);
        }
    }
}

impl PostgresSession {
    pub(crate) fn is_cockroach(&self) -> bool {
        self.flavor == Flavor::Cockroach
    }
}

impl Drop for PostgresSession {
    fn drop(&mut self) {
        cancel::unregister(&self.profile_id, self.runtime.connection_id);
    }
}

impl DatabaseSession for PostgresSession {
    fn dialect(&self) -> SqlDialect {
        self.flavor.dialect()
    }

    fn runtime(&self) -> &ServerRuntimeInfo {
        &self.runtime
    }

    fn ping(&mut self) -> Result<(), DbError> {
        if let Some(tunnel) = &mut self.tunnel {
            if !tunnel.is_alive() {
                return Err(DbError::message("SSH tunnel process terminated"));
            }
        }
        self.run_batch("SELECT 1")
    }

    fn set_schema(&mut self, schema: &str) -> Result<(), DbError> {
        let dialect = self.dialect();
        let search_path = if schema.eq_ignore_ascii_case("public") {
            dialect.quote_ident(schema)
        } else {
            format!("{}, public", dialect.quote_ident(schema))
        };
        self.run_batch(&format!("SET search_path TO {search_path}"))?;
        self.runtime.current_schema = Some(schema.to_string());
        Ok(())
    }

    fn set_autocommit(&mut self, enabled: bool) -> Result<(), DbError> {
        if enabled && self.runtime.in_transaction {
            self.run_batch("COMMIT")?;
            self.runtime.in_transaction = false;
        }
        self.runtime.autocommit = enabled;
        Ok(())
    }

    fn begin_transaction(&mut self) -> Result<(), DbError> {
        self.run_batch("BEGIN")?;
        self.runtime.in_transaction = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), DbError> {
        let committed = self.run_batch("COMMIT");
        self.runtime.in_transaction = false;
        committed
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        let rolled_back = self.run_batch("ROLLBACK");
        self.runtime.in_transaction = false;
        rolled_back
    }

    fn execute_sql(&mut self, sql: &str, max_rows: u32) -> Result<QueryOutcome, DbError> {
        self.ensure_writable(sql)?;
        self.begin_implicit_transaction_if_needed(sql)?;
        let start = Instant::now();
        let outcome = {
            let Self {
                executor,
                client,
                metadata,
                runtime,
                is_cursor_supported,
                overflow_canceller,
                ..
            } = self;
            let options = ExecutionOptions {
                max_rows,
                is_in_transaction: runtime.in_transaction,
                is_cursor_supported: *is_cursor_supported,
                overflow_canceller: overflow_canceller.as_deref(),
            };
            executor.block_on(execute_statement(client, metadata, sql, &options))
        };
        if outcome.is_err() && transaction_effect(sql) == TransactionEffect::Ends {
            self.runtime.in_transaction = false;
        }
        let execution = outcome?;
        self.track_statement_effects(sql);
        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(match execution.page {
            Some(mut page) => {
                page.elapsed_ms = elapsed_ms;
                QueryOutcome::Rows(page)
            }
            None => QueryOutcome::Affected {
                affected_rows: execution.affected_rows,
                last_insert_id: None,
                warnings: 0,
                elapsed_ms,
                executed_sql: sql.to_string(),
            },
        })
    }

    #[allow(clippy::type_complexity)]
    fn stream_query(
        &mut self,
        sql: &str,
        on_columns: &mut dyn FnMut(&[ColumnMeta]) -> Result<(), String>,
        on_row: &mut dyn FnMut(&[Option<String>]) -> Result<bool, String>,
    ) -> Result<u64, DbError> {
        self.ensure_writable(sql)?;
        let Self {
            executor,
            client,
            metadata,
            runtime,
            is_cursor_supported,
            ..
        } = self;
        executor.block_on(stream_statement(
            client,
            metadata,
            sql,
            runtime.in_transaction,
            *is_cursor_supported,
            on_columns,
            on_row,
        ))
    }

    fn execute_atomic_batch(&mut self, statements: &[String]) -> Result<u64, DbError> {
        if self.read_only {
            return Err(DbError::message(
                "Cannot submit changes while Read-Only protection is enabled.",
            ));
        }
        if statements.is_empty() {
            return Ok(0);
        }
        let start = Instant::now();
        let is_nested = self.runtime.in_transaction;
        let (open, close, undo) = if is_nested {
            (
                format!("SAVEPOINT {BATCH_SAVEPOINT}"),
                format!("RELEASE SAVEPOINT {BATCH_SAVEPOINT}"),
                format!("ROLLBACK TO SAVEPOINT {BATCH_SAVEPOINT}; RELEASE SAVEPOINT {BATCH_SAVEPOINT}"),
            )
        } else {
            ("BEGIN".to_string(), "COMMIT".to_string(), "ROLLBACK".to_string())
        };
        self.run_batch(&open)?;
        for statement in statements {
            if let Err(error) = self.run_batch(statement) {
                let _ = self.run_batch(&undo);
                return Err(error);
            }
        }
        self.run_batch(&close)?;
        self.metadata.clear();
        Ok(start.elapsed().as_millis() as u64)
    }

    fn explain(&mut self, sql: &str, analyze: bool) -> Result<ExplainResult, DbError> {
        if analyze {
            self.ensure_writable(sql)?;
        }
        let Self {
            executor,
            client,
            flavor,
            runtime,
            ..
        } = self;
        executor.block_on(monitoring::explain(client, *flavor, sql, analyze, runtime.in_transaction))
    }

    fn fetch_processlist(&mut self) -> Result<Vec<ProcessEntry>, DbError> {
        let Self {
            executor,
            client,
            flavor,
            cockroach_query_ids,
            ..
        } = self;
        executor.block_on(monitoring::fetch_processlist(client, *flavor, cockroach_query_ids))
    }

    fn kill_process(&mut self, process_id: u64) -> Result<(), DbError> {
        match self.flavor {
            Flavor::Postgres => self.run_batch(&format!("SELECT pg_cancel_backend({process_id})")),
            Flavor::Cockroach => {
                let query_id = usize::try_from(process_id)
                    .ok()
                    .and_then(|position| position.checked_sub(1))
                    .and_then(|index| self.cockroach_query_ids.get(index).cloned())
                    .ok_or_else(|| DbError::message("That query already finished. Refresh the process list."))?;
                self.run_batch(&format!(
                    "CANCEL QUERY IF EXISTS {}",
                    SqlDialect::CockroachDb.string_literal(&query_id)
                ))
            }
        }
    }

    fn fetch_health_metrics(&mut self) -> Result<ServerHealthMetrics, DbError> {
        let Self {
            executor,
            client,
            flavor,
            ..
        } = self;
        executor.block_on(monitoring::fetch_health_metrics(client, *flavor))
    }

    fn list_schemas(&mut self) -> Result<Vec<SchemaSummary>, DbError> {
        let Self { executor, client, .. } = self;
        executor.block_on(introspection::list_schemas(client))
    }

    fn load_schema_objects(&mut self, schema: &str) -> Result<SchemaObjects, DbError> {
        let Self {
            executor,
            client,
            flavor,
            ..
        } = self;
        executor.block_on(introspection::load_schema_objects(client, *flavor, schema))
    }

    fn load_table_detail(&mut self, schema: &str, table: &str) -> Result<TableDetail, DbError> {
        let Self {
            executor,
            client,
            flavor,
            ..
        } = self;
        executor.block_on(table_detail::load_table_detail(client, *flavor, schema, table))
    }
}
