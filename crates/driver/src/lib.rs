mod catalog;
mod endpoint;
mod error;
mod tunnel;

pub use catalog::{
    ColumnSummary, ForeignKeyInfo, InboundForeignKeyInfo, IndexInfo, RoutineSummary, SchemaObjects, SchemaSummary,
    TableDetail, TableKind, TableSummary,
};
pub use endpoint::{resolve_tcp_target, TcpTarget};
pub use error::DbError;
pub use tunnel::SshTunnel;

use cottondata_core::{ServerRuntimeInfo, SqlDialect};
use cottondata_grid::{ColumnMeta, ColumnarPage};

#[derive(Debug, Clone)]
pub enum QueryOutcome {
    Rows(ColumnarPage),
    Affected {
        affected_rows: u64,
        last_insert_id: Option<u64>,
        warnings: u16,
        elapsed_ms: u64,
        executed_sql: String,
    },
}

#[derive(Debug, Clone)]
pub struct ExplainResult {
    pub is_analyze: bool,
    pub label: String,
    pub raw_output: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ProcessEntry {
    pub id: u64,
    pub user: String,
    pub host: String,
    pub db: Option<String>,
    pub command: String,
    pub time_secs: i64,
    pub state: Option<String>,
    pub info: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthMetric {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct ServerHealthMetrics {
    pub metrics: Vec<HealthMetric>,
}

impl ServerHealthMetrics {
    pub fn push(&mut self, label: impl Into<String>, value: impl Into<String>) {
        self.metrics.push(HealthMetric {
            label: label.into(),
            value: value.into(),
        });
    }
}

pub trait DatabaseSession: Send {
    fn dialect(&self) -> SqlDialect;
    fn runtime(&self) -> &ServerRuntimeInfo;
    fn ping(&mut self) -> Result<(), DbError>;
    fn set_schema(&mut self, schema: &str) -> Result<(), DbError>;
    fn set_autocommit(&mut self, enabled: bool) -> Result<(), DbError>;
    fn begin_transaction(&mut self) -> Result<(), DbError>;
    fn commit(&mut self) -> Result<(), DbError>;
    fn rollback(&mut self) -> Result<(), DbError>;
    fn execute_sql(&mut self, sql: &str, max_rows: u32) -> Result<QueryOutcome, DbError>;
    #[allow(clippy::type_complexity)]
    fn stream_query(
        &mut self,
        sql: &str,
        on_columns: &mut dyn FnMut(&[ColumnMeta]) -> Result<(), String>,
        on_row: &mut dyn FnMut(&[Option<String>]) -> Result<bool, String>,
    ) -> Result<u64, DbError>;
    fn execute_atomic_batch(&mut self, statements: &[String]) -> Result<u64, DbError>;
    fn explain(&mut self, sql: &str, analyze: bool) -> Result<ExplainResult, DbError>;
    fn fetch_processlist(&mut self) -> Result<Vec<ProcessEntry>, DbError>;
    fn kill_process(&mut self, process_id: u64) -> Result<(), DbError> {
        let Some(sql) = self.dialect().kill_process(process_id) else {
            return Err(DbError::message("This database does not support cancelling other sessions"));
        };
        self.execute_sql(&sql, 1).map(|_| ())
    }
    fn fetch_health_metrics(&mut self) -> Result<ServerHealthMetrics, DbError>;
    fn list_schemas(&mut self) -> Result<Vec<SchemaSummary>, DbError>;
    fn load_schema_objects(&mut self, schema: &str) -> Result<SchemaObjects, DbError>;
    fn load_table_detail(&mut self, schema: &str, table: &str) -> Result<TableDetail, DbError>;
}
