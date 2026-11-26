use crate::errors::mssql_error;
use crate::rows::{int, optional_text, text, MssqlClient};
use crate::session::MssqlSession;
use cottondata_driver::{DbError, ExplainResult, ProcessEntry, ServerHealthMetrics};
use futures_util::TryStreamExt;
use std::time::Instant;
use tiberius::QueryItem;

const SHOWPLAN_COLUMN_MARKER: &str = "showplan";

pub(crate) fn explain(session: &mut MssqlSession, sql: &str, analyze: bool) -> Result<ExplainResult, DbError> {
    let (setting, label) = if analyze {
        ("STATISTICS XML", "STATISTICS XML")
    } else {
        ("SHOWPLAN_XML", "SHOWPLAN XML")
    };
    let start = Instant::now();
    session.run_command(&format!("SET {setting} ON"))?;
    let plan = {
        let MssqlSession { executor, client, .. } = &mut *session;
        executor.block_on(collect_plan_xml(client, sql, analyze))
    };
    let reset = session.run_command(&format!("SET {setting} OFF"));
    let raw_output = plan?;
    reset?;
    Ok(ExplainResult {
        is_analyze: analyze,
        label: label.into(),
        raw_output,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

async fn collect_plan_xml(client: &mut MssqlClient, sql: &str, analyze: bool) -> Result<String, DbError> {
    let mut stream = client.simple_query(sql).await.map_err(mssql_error)?;
    let mut is_plan_result = !analyze;
    let mut plans = Vec::new();
    while let Some(item) = stream.try_next().await.map_err(mssql_error)? {
        match item {
            QueryItem::Metadata(metadata) => {
                is_plan_result = !analyze
                    || metadata
                        .columns()
                        .first()
                        .is_some_and(|column| column.name().to_ascii_lowercase().contains(SHOWPLAN_COLUMN_MARKER));
            }
            QueryItem::Row(row) if is_plan_result => {
                if let Some(plan) = optional_text(&row, 0) {
                    plans.push(plan);
                }
            }
            QueryItem::Row(_) => {}
        }
    }
    Ok(plans.join("\n"))
}

pub(crate) async fn fetch_processlist(client: &mut MssqlClient) -> Result<Vec<ProcessEntry>, DbError> {
    let rows = client
        .simple_query(
            "SELECT s.session_id, s.login_name, ISNULL(s.host_name, ''), DB_NAME(s.database_id), \
                    ISNULL(r.command, s.status), \
                    DATEDIFF(SECOND, ISNULL(r.start_time, s.last_request_start_time), SYSDATETIME()), \
                    r.status, t.text \
             FROM sys.dm_exec_sessions s \
             LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id \
             OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t \
             WHERE s.is_user_process = 1 ORDER BY s.session_id",
        )
        .await
        .map_err(mssql_error)?
        .into_first_result()
        .await
        .map_err(mssql_error)?;
    Ok(rows
        .iter()
        .map(|row| ProcessEntry {
            id: u64::try_from(int(row, 0)).unwrap_or(0),
            user: text(row, 1),
            host: text(row, 2),
            db: optional_text(row, 3),
            command: text(row, 4),
            time_secs: int(row, 5),
            state: optional_text(row, 6),
            info: optional_text(row, 7),
        })
        .collect())
}

pub(crate) async fn fetch_health_metrics(client: &mut MssqlClient) -> Result<ServerHealthMetrics, DbError> {
    let row = client
        .simple_query(
            "SELECT DATEDIFF(SECOND, i.sqlserver_start_time, SYSDATETIME()), \
                    (SELECT COUNT(*) FROM sys.dm_exec_sessions WHERE is_user_process = 1), \
                    (SELECT COUNT(*) FROM sys.dm_exec_requests r \
                       JOIN sys.dm_exec_sessions s ON s.session_id = r.session_id WHERE s.is_user_process = 1), \
                    CAST(SERVERPROPERTY('Edition') AS nvarchar(128)) \
             FROM sys.dm_os_sys_info i",
        )
        .await
        .map_err(mssql_error)?
        .into_row()
        .await
        .map_err(mssql_error)?
        .ok_or_else(|| DbError::message("SQL Server did not return health metrics"))?;
    let mut metrics = ServerHealthMetrics::default();
    metrics.push("Uptime", format!("{}s", int(&row, 0)));
    metrics.push("User sessions", int(&row, 1).to_string());
    metrics.push("Running requests", int(&row, 2).to_string());
    metrics.push("Edition", text(&row, 3));
    Ok(metrics)
}
