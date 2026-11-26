use crate::connection::{load_runtime_info, open_client};
use crate::errors::mssql_error;
use crate::introspection;
use crate::monitoring;
use crate::results::{read_first_result_set, stream_first_result_set};
use crate::rows::{int, optional_text, MssqlClient};
use cottondata_core::{ServerProfile, ServerRuntimeInfo, SqlDialect};
use cottondata_driver::{
    resolve_tcp_target, DatabaseSession, DbError, ExplainResult, ProcessEntry, QueryOutcome, SchemaObjects,
    SchemaSummary, ServerHealthMetrics, SshTunnel, TableDetail,
};
use cottondata_grid::ColumnMeta;
use cottondata_sql::{analyze_statement, WriteClass};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

pub struct MssqlSession {
    pub(crate) executor: Runtime,
    pub(crate) client: MssqlClient,
    tunnel: Option<SshTunnel>,
    runtime: ServerRuntimeInfo,
    read_only: bool,
}

impl MssqlSession {
    pub fn connect(profile: &ServerProfile, password: Option<&str>) -> Result<Self, DbError> {
        let timeout = Duration::from_secs(u64::from(profile.connect_timeout_secs.max(3)));
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| DbError::message(format!("Could not start the SQL Server runtime: {err}")))?;
        let target = resolve_tcp_target(profile, timeout)?;
        let mut client = executor.block_on(open_client(profile, password, &target.host, target.port, timeout))?;
        let runtime = executor.block_on(load_runtime_info(&mut client))?;
        Ok(Self {
            executor,
            client,
            tunnel: target.tunnel,
            runtime,
            read_only: profile.read_only,
        })
    }

    pub fn kill_session_via_aux(
        profile: &ServerProfile,
        password: Option<&str>,
        session_id: u32,
    ) -> Result<(), DbError> {
        let mut aux = Self::connect(profile, password)?;
        aux.run_command(&format!("KILL {session_id}")).map(|_| ())
    }

    pub(crate) fn run_command(&mut self, sql: &str) -> Result<u64, DbError> {
        let Self { executor, client, .. } = self;
        executor.block_on(async {
            let result = client.execute(sql, &[]).await.map_err(mssql_error)?;
            Ok(result.rows_affected().iter().sum())
        })
    }

    fn refresh_session_state(&mut self) {
        let Self {
            executor,
            client,
            runtime,
            ..
        } = self;
        let state = executor.block_on(async {
            client
                .simple_query("SELECT @@TRANCOUNT, DB_NAME(), CAST(@@OPTIONS & 2 AS int)")
                .await?
                .into_row()
                .await
        });
        if let Ok(Some(row)) = state {
            runtime.in_transaction = int(&row, 0) > 0;
            runtime.current_schema = optional_text(&row, 1);
            runtime.autocommit = int(&row, 2) == 0;
        }
    }

    fn ensure_writable(&self, sql: &str) -> Result<(), DbError> {
        if self.read_only && !analyze_statement(sql).allowed_in_read_only() {
            return Err(DbError::message(
                "Blocked by server Read-Only protection. Disable Read-Only on this server slot to execute writes.",
            ));
        }
        Ok(())
    }
}

impl DatabaseSession for MssqlSession {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::SqlServer
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
        self.run_command("SELECT 1").map(|_| ())
    }

    fn set_schema(&mut self, schema: &str) -> Result<(), DbError> {
        self.run_command(&format!("USE {}", SqlDialect::SqlServer.quote_ident(schema)))?;
        self.runtime.current_schema = Some(schema.to_string());
        Ok(())
    }

    fn set_autocommit(&mut self, enabled: bool) -> Result<(), DbError> {
        let setting = if enabled { "OFF" } else { "ON" };
        self.run_command(&format!("SET IMPLICIT_TRANSACTIONS {setting}"))?;
        self.runtime.autocommit = enabled;
        Ok(())
    }

    fn begin_transaction(&mut self) -> Result<(), DbError> {
        self.run_command("BEGIN TRANSACTION")?;
        self.runtime.in_transaction = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), DbError> {
        self.run_command("IF @@TRANCOUNT > 0 COMMIT TRANSACTION")?;
        self.runtime.in_transaction = false;
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        self.run_command("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")?;
        self.runtime.in_transaction = false;
        Ok(())
    }

    fn execute_sql(&mut self, sql: &str, max_rows: u32) -> Result<QueryOutcome, DbError> {
        self.ensure_writable(sql)?;
        let start = Instant::now();
        let is_write = matches!(
            analyze_statement(sql).class,
            WriteClass::DataMutation | WriteClass::SchemaMutation | WriteClass::Destructive
        );
        let page = if is_write {
            None
        } else {
            let Self { executor, client, .. } = self;
            executor.block_on(read_first_result_set(client, sql, max_rows))?
        };
        let affected_rows = if is_write { self.run_command(sql)? } else { 0 };
        self.refresh_session_state();
        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(match page {
            Some(mut page) => {
                page.elapsed_ms = elapsed_ms;
                QueryOutcome::Rows(page)
            }
            None => QueryOutcome::Affected {
                affected_rows,
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
        let Self { executor, client, .. } = self;
        let (rows, completed_stream) =
            executor.block_on(stream_first_result_set(client, sql, on_columns, on_row))?;
        if completed_stream {
            self.refresh_session_state();
        }
        Ok(rows)
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
        self.run_command("BEGIN TRANSACTION")?;
        for statement in statements {
            if let Err(err) = self.run_command(statement) {
                let _ = self.run_command("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION");
                return Err(err);
            }
        }
        self.run_command("COMMIT TRANSACTION")?;
        self.runtime.in_transaction = false;
        Ok(start.elapsed().as_millis() as u64)
    }

    fn explain(&mut self, sql: &str, analyze: bool) -> Result<ExplainResult, DbError> {
        if analyze {
            self.ensure_writable(sql)?;
        }
        monitoring::explain(self, sql, analyze)
    }

    fn fetch_processlist(&mut self) -> Result<Vec<ProcessEntry>, DbError> {
        let Self { executor, client, .. } = self;
        executor.block_on(monitoring::fetch_processlist(client))
    }

    fn fetch_health_metrics(&mut self) -> Result<ServerHealthMetrics, DbError> {
        let Self { executor, client, .. } = self;
        executor.block_on(monitoring::fetch_health_metrics(client))
    }

    fn list_schemas(&mut self) -> Result<Vec<SchemaSummary>, DbError> {
        let Self { executor, client, .. } = self;
        executor.block_on(introspection::list_schemas(client))
    }

    fn load_schema_objects(&mut self, schema: &str) -> Result<SchemaObjects, DbError> {
        let Self { executor, client, .. } = self;
        executor.block_on(introspection::load_schema_objects(client, schema))
    }

    fn load_table_detail(&mut self, schema: &str, table: &str) -> Result<TableDetail, DbError> {
        let Self { executor, client, .. } = self;
        executor.block_on(introspection::load_table_detail(client, schema, table))
    }
}
