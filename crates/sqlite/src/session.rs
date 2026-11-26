use crate::errors::sqlite_error;
use crate::interrupt;
use crate::introspection;
use crate::results::{collect_health_metrics, describe_columns, load_query_plan, read_page, render_query_plan};
use crate::values::value_text;
use cottondata_core::{ConnectionEndpoint, ServerProfile, ServerRuntimeInfo, SqlDialect};
use cottondata_driver::{
    DatabaseSession, DbError, ExplainResult, ProcessEntry, QueryOutcome, SchemaObjects, SchemaSummary,
    ServerHealthMetrics, TableDetail,
};
use cottondata_grid::{ColumnMeta, ColumnarPage};
use cottondata_sql::{analyze_statement, WriteClass};
use rusqlite::{Connection, OpenFlags};
use std::time::{Duration, Instant};

const MAIN_SCHEMA: &str = "main";
const BATCH_SAVEPOINT: &str = "cottondata_batch";
const EXPLAIN_SAVEPOINT: &str = "cottondata_explain";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SqliteSession {
    connection: Connection,
    runtime: ServerRuntimeInfo,
    profile_id: String,
    path: String,
    read_only: bool,
}

impl SqliteSession {
    pub fn connect(profile: &ServerProfile, _password: Option<&str>) -> Result<Self, DbError> {
        let ConnectionEndpoint::File { path } = &profile.endpoint else {
            return Err(DbError::message("SQLite connections need a database file path"));
        };
        let path = expand_home(path.trim());
        if path.is_empty() {
            return Err(DbError::message("SQLite connections need a database file path"));
        }
        let flags = if profile.read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
        };
        let connection = Connection::open_with_flags(&path, flags).map_err(sqlite_error)?;
        connection.busy_timeout(BUSY_TIMEOUT).map_err(sqlite_error)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON")
            .map_err(sqlite_error)?;
        let version: String = connection
            .query_row("SELECT sqlite_version()", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        let (version_major, version_minor, version_patch) = ServerRuntimeInfo::parse_version_tuple(&version);
        let connection_id = interrupt::register(&profile.id, connection.get_interrupt_handle());
        Ok(Self {
            connection,
            runtime: ServerRuntimeInfo {
                version,
                version_major,
                version_minor,
                version_patch,
                lower_case_table_names: 0,
                sql_mode: String::new(),
                time_zone: "UTC".into(),
                current_schema: Some(MAIN_SCHEMA.into()),
                connection_id,
                autocommit: true,
                in_transaction: false,
            },
            profile_id: profile.id.clone(),
            path,
            read_only: profile.read_only,
        })
    }

    pub fn interrupt_running_query(profile: &ServerProfile, connection_id: u32) -> Result<(), DbError> {
        if interrupt::interrupt(&profile.id, connection_id) {
            Ok(())
        } else {
            Err(DbError::message("The running SQLite statement was not found"))
        }
    }

    fn ensure_writable(&self, sql: &str) -> Result<(), DbError> {
        if self.read_only && !analyze_statement(sql).allowed_in_read_only() {
            return Err(DbError::message(
                "Blocked by Read-Only protection. Disable Read-Only on this connection to execute writes.",
            ));
        }
        Ok(())
    }

    fn sync_transaction_state(&mut self) {
        self.runtime.in_transaction = !self.connection.is_autocommit();
    }

    fn begin_implicit_transaction_if_needed(&mut self, sql: &str) -> Result<(), DbError> {
        if self.runtime.autocommit || !self.connection.is_autocommit() || is_transaction_control(sql) {
            return Ok(());
        }
        self.connection.execute_batch("BEGIN").map_err(sqlite_error)?;
        self.sync_transaction_state();
        Ok(())
    }

    fn run_statement(&mut self, sql: &str, max_rows: u32) -> Result<(Option<ColumnarPage>, u64), DbError> {
        let statement = match self.connection.prepare(sql) {
            Ok(statement) => statement,
            Err(rusqlite::Error::MultipleStatement) => {
                let before = self.connection.total_changes();
                self.connection.execute_batch(sql).map_err(sqlite_error)?;
                return Ok((None, self.connection.total_changes().saturating_sub(before)));
            }
            Err(error) => return Err(sqlite_error(error)),
        };
        if statement.column_count() == 0 {
            let mut statement = statement;
            let affected = statement.raw_execute().map_err(sqlite_error)?;
            return Ok((None, affected as u64));
        }
        let must_drain = !matches!(analyze_statement(sql).class, WriteClass::ReadOnly);
        read_page(statement, sql, max_rows, must_drain).map(|page| (Some(page), 0))
    }
}

impl Drop for SqliteSession {
    fn drop(&mut self) {
        interrupt::unregister(&self.profile_id, self.runtime.connection_id);
    }
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => std::path::Path::new(&home).join(rest).to_string_lossy().into_owned(),
        _ => path.to_string(),
    }
}

fn is_transaction_control(sql: &str) -> bool {
    let head = sql.split_whitespace().next().unwrap_or_default().to_ascii_uppercase();
    matches!(
        head.as_str(),
        "BEGIN" | "COMMIT" | "END" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "PRAGMA" | "VACUUM" | "ATTACH" | "DETACH"
    )
}

impl DatabaseSession for SqliteSession {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Sqlite
    }

    fn runtime(&self) -> &ServerRuntimeInfo {
        &self.runtime
    }

    fn ping(&mut self) -> Result<(), DbError> {
        self.connection
            .query_row("SELECT 1", [], |_| Ok(()))
            .map_err(sqlite_error)
    }

    fn set_schema(&mut self, schema: &str) -> Result<(), DbError> {
        let exists = introspection::list_schemas(&self.connection)?
            .iter()
            .any(|summary| summary.name.eq_ignore_ascii_case(schema));
        if !exists {
            return Err(DbError::message(format!("Database '{schema}' is not attached")));
        }
        self.runtime.current_schema = Some(schema.to_string());
        Ok(())
    }

    fn set_autocommit(&mut self, enabled: bool) -> Result<(), DbError> {
        if enabled && !self.connection.is_autocommit() {
            self.connection.execute_batch("COMMIT").map_err(sqlite_error)?;
        }
        self.runtime.autocommit = enabled;
        self.sync_transaction_state();
        Ok(())
    }

    fn begin_transaction(&mut self) -> Result<(), DbError> {
        self.connection.execute_batch("BEGIN").map_err(sqlite_error)?;
        self.sync_transaction_state();
        Ok(())
    }

    fn commit(&mut self) -> Result<(), DbError> {
        let committed = if self.connection.is_autocommit() {
            Ok(())
        } else {
            self.connection.execute_batch("COMMIT").map_err(sqlite_error)
        };
        self.sync_transaction_state();
        committed
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        let rolled_back = if self.connection.is_autocommit() {
            Ok(())
        } else {
            self.connection.execute_batch("ROLLBACK").map_err(sqlite_error)
        };
        self.sync_transaction_state();
        rolled_back
    }

    fn execute_sql(&mut self, sql: &str, max_rows: u32) -> Result<QueryOutcome, DbError> {
        self.ensure_writable(sql)?;
        self.begin_implicit_transaction_if_needed(sql)?;
        let start = Instant::now();
        let outcome = self.run_statement(sql, max_rows);
        self.sync_transaction_state();
        let (page, affected_rows) = outcome?;
        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(match page {
            Some(mut page) => {
                page.elapsed_ms = elapsed_ms;
                QueryOutcome::Rows(page)
            }
            None => QueryOutcome::Affected {
                affected_rows,
                last_insert_id: u64::try_from(self.connection.last_insert_rowid()).ok().filter(|_| affected_rows > 0),
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
        let mut statement = self.connection.prepare(sql).map_err(sqlite_error)?;
        let columns = describe_columns(&statement);
        on_columns(&columns).map_err(DbError::message)?;
        if columns.is_empty() {
            statement.raw_execute().map_err(sqlite_error)?;
            return Ok(0);
        }
        let column_count = columns.len();
        let mut rows = statement.raw_query();
        let mut streamed = 0u64;
        let mut row_buffer: Vec<Option<String>> = Vec::with_capacity(column_count);
        while let Some(row) = rows.next().map_err(sqlite_error)? {
            row_buffer.clear();
            for index in 0..column_count {
                row_buffer.push(value_text(row.get_ref(index).map_err(sqlite_error)?));
            }
            streamed += 1;
            if !on_row(&row_buffer).map_err(DbError::message)? {
                break;
            }
        }
        Ok(streamed)
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
        let is_nested = !self.connection.is_autocommit();
        let (open, close, undo) = if is_nested {
            (
                format!("SAVEPOINT {BATCH_SAVEPOINT}"),
                format!("RELEASE SAVEPOINT {BATCH_SAVEPOINT}"),
                format!("ROLLBACK TO SAVEPOINT {BATCH_SAVEPOINT}; RELEASE SAVEPOINT {BATCH_SAVEPOINT}"),
            )
        } else {
            ("BEGIN".to_string(), "COMMIT".to_string(), "ROLLBACK".to_string())
        };
        self.connection.execute_batch(&open).map_err(sqlite_error)?;
        for statement in statements {
            if let Err(error) = self.connection.execute_batch(statement) {
                let _ = self.connection.execute_batch(&undo);
                self.sync_transaction_state();
                return Err(sqlite_error(error));
            }
        }
        self.connection.execute_batch(&close).map_err(sqlite_error)?;
        self.sync_transaction_state();
        Ok(start.elapsed().as_millis() as u64)
    }

    fn explain(&mut self, sql: &str, analyze: bool) -> Result<ExplainResult, DbError> {
        let start = Instant::now();
        let body = sql.trim().trim_end_matches(|c: char| c == ';' || c.is_whitespace());
        let plan_rows = load_query_plan(&self.connection, body)?;
        let mut raw_output = render_query_plan(&plan_rows);
        if analyze {
            self.ensure_writable(sql)?;
            self.connection
                .execute_batch(&format!("SAVEPOINT {EXPLAIN_SAVEPOINT}"))
                .map_err(sqlite_error)?;
            let run_start = Instant::now();
            let measured = self.run_statement(body, u32::MAX);
            let run_ms = run_start.elapsed().as_millis();
            let _ = self.connection.execute_batch(&format!(
                "ROLLBACK TO SAVEPOINT {EXPLAIN_SAVEPOINT}; RELEASE SAVEPOINT {EXPLAIN_SAVEPOINT}"
            ));
            self.sync_transaction_state();
            let (page, affected) = measured?;
            let produced = page.map(|page| page.row_count as u64).unwrap_or(affected);
            raw_output.push_str(&format!(
                "\n\nExecuted in {run_ms} ms · {produced} row(s) · changes rolled back"
            ));
        }
        Ok(ExplainResult {
            is_analyze: analyze,
            label: if analyze {
                "EXPLAIN QUERY PLAN + timed run (rolled back)".into()
            } else {
                "EXPLAIN QUERY PLAN".into()
            },
            raw_output,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }

    fn fetch_processlist(&mut self) -> Result<Vec<ProcessEntry>, DbError> {
        Ok(Vec::new())
    }

    fn fetch_health_metrics(&mut self) -> Result<ServerHealthMetrics, DbError> {
        Ok(collect_health_metrics(&self.connection, &self.path, &self.runtime.version))
    }

    fn list_schemas(&mut self) -> Result<Vec<SchemaSummary>, DbError> {
        introspection::list_schemas(&self.connection)
    }

    fn load_schema_objects(&mut self, schema: &str) -> Result<SchemaObjects, DbError> {
        introspection::load_schema_objects(&self.connection, schema)
    }

    fn load_table_detail(&mut self, schema: &str, table: &str) -> Result<TableDetail, DbError> {
        introspection::load_table_detail(&self.connection, schema, table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_control_detection_skips_implicit_begin() {
        assert!(is_transaction_control("  commit"));
        assert!(is_transaction_control("PRAGMA foreign_keys = OFF"));
        assert!(!is_transaction_control("UPDATE t SET a = 1"));
        assert_eq!(expand_home("/tmp/a.db"), "/tmp/a.db");
    }
}
