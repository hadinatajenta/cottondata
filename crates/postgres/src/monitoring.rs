use crate::errors::{postgres_error, postgres_error_for};
use crate::introspection::Flavor;
use cottondata_driver::{DbError, ExplainResult, ProcessEntry, ServerHealthMetrics};
use std::time::Instant;
use tokio_postgres::{Client, SimpleQueryMessage, SimpleQueryRow};

const EXPLAIN_SAVEPOINT: &str = "cottondata_explain";

pub(crate) fn explain_statement(flavor: Flavor, sql: &str, analyze: bool) -> String {
    let body = sql.trim().trim_end_matches(|c: char| c == ';' || c.is_whitespace());
    match (flavor, analyze) {
        (Flavor::Postgres, true) => format!("EXPLAIN (ANALYZE, BUFFERS) {body}"),
        (Flavor::Cockroach, true) => format!("EXPLAIN ANALYZE {body}"),
        (_, false) => format!("EXPLAIN {body}"),
    }
}

pub(crate) async fn explain(
    client: &Client,
    flavor: Flavor,
    sql: &str,
    analyze: bool,
    is_in_transaction: bool,
) -> Result<ExplainResult, DbError> {
    let start = Instant::now();
    let statement = explain_statement(flavor, sql, analyze);
    let messages = if analyze {
        explain_and_roll_back(client, &statement, sql, is_in_transaction).await?
    } else {
        client
            .simple_query(&statement)
            .await
            .map_err(|error| postgres_error_for(error, Some(sql)))?
    };
    let raw_output = messages
        .iter()
        .filter_map(|message| match message {
            SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ExplainResult {
        is_analyze: analyze,
        label: if analyze {
            "EXPLAIN ANALYZE (changes rolled back)".into()
        } else {
            "EXPLAIN".into()
        },
        raw_output,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

async fn explain_and_roll_back(
    client: &Client,
    statement: &str,
    sql: &str,
    is_in_transaction: bool,
) -> Result<Vec<SimpleQueryMessage>, DbError> {
    let (open, undo) = if is_in_transaction {
        (
            format!("SAVEPOINT {EXPLAIN_SAVEPOINT}"),
            format!("ROLLBACK TO SAVEPOINT {EXPLAIN_SAVEPOINT}; RELEASE SAVEPOINT {EXPLAIN_SAVEPOINT}"),
        )
    } else {
        ("BEGIN".to_string(), "ROLLBACK".to_string())
    };
    client.batch_execute(&open).await.map_err(postgres_error)?;
    let explained = client.simple_query(statement).await;
    let undone = client.batch_execute(&undo).await;
    let messages = explained.map_err(|error| postgres_error_for(error, Some(sql)))?;
    undone.map_err(postgres_error)?;
    Ok(messages)
}

fn row_text(row: &SimpleQueryRow, name: &str) -> Option<String> {
    row.try_get(name).ok().flatten().map(str::to_string)
}

fn data_rows(messages: Vec<SimpleQueryMessage>) -> Vec<SimpleQueryRow> {
    messages
        .into_iter()
        .filter_map(|message| match message {
            SimpleQueryMessage::Row(row) => Some(row),
            _ => None,
        })
        .collect()
}

pub(crate) async fn fetch_processlist(
    client: &Client,
    flavor: Flavor,
    cockroach_query_ids: &mut Vec<String>,
) -> Result<Vec<ProcessEntry>, DbError> {
    match flavor {
        Flavor::Postgres => {
            let messages = client
                .simple_query(
                    "SELECT s.pid::text AS id, s.usename::text AS user_name, \
                            COALESCE(s.client_addr::text, s.client_hostname, 'local') AS host, \
                            s.datname::text AS db, COALESCE(s.state, '') AS command, \
                            COALESCE(EXTRACT(EPOCH FROM (now() - COALESCE(s.query_start, s.backend_start)))::bigint, 0)::text AS elapsed, \
                            NULLIF(concat_ws(': ', to_jsonb(s) ->> 'wait_event_type', to_jsonb(s) ->> 'wait_event'), '') AS wait, \
                            s.query AS info \
                     FROM pg_stat_activity s WHERE s.datname IS NOT NULL ORDER BY s.pid",
                )
                .await
                .map_err(postgres_error)?;
            Ok(data_rows(messages)
                .iter()
                .map(|row| ProcessEntry {
                    id: row_text(row, "id").and_then(|id| id.parse().ok()).unwrap_or(0),
                    user: row_text(row, "user_name").unwrap_or_default(),
                    host: row_text(row, "host").unwrap_or_default(),
                    db: row_text(row, "db"),
                    command: row_text(row, "command").unwrap_or_default(),
                    time_secs: row_text(row, "elapsed").and_then(|secs| secs.parse().ok()).unwrap_or(0),
                    state: row_text(row, "wait"),
                    info: row_text(row, "info"),
                })
                .collect())
        }
        Flavor::Cockroach => {
            let messages = client
                .simple_query(
                    "SELECT query_id, user_name, client_address, application_name, \
                            (EXTRACT(epoch FROM now()) - EXTRACT(epoch FROM start))::INT8 AS elapsed, query \
                     FROM [SHOW CLUSTER QUERIES] ORDER BY start",
                )
                .await
                .map_err(postgres_error)?;
            cockroach_query_ids.clear();
            Ok(data_rows(messages)
                .iter()
                .map(|row| {
                    cockroach_query_ids.push(row_text(row, "query_id").unwrap_or_default());
                    ProcessEntry {
                        id: cockroach_query_ids.len() as u64,
                        user: row_text(row, "user_name").unwrap_or_default(),
                        host: row_text(row, "client_address").unwrap_or_default(),
                        db: None,
                        command: row_text(row, "application_name").unwrap_or_default(),
                        time_secs: row_text(row, "elapsed").and_then(|secs| secs.parse().ok()).unwrap_or(0),
                        state: Some("executing".into()),
                        info: row_text(row, "query"),
                    }
                })
                .collect())
        }
    }
}

pub(crate) async fn fetch_health_metrics(client: &Client, flavor: Flavor) -> Result<ServerHealthMetrics, DbError> {
    let mut metrics = ServerHealthMetrics::default();
    let probes: &[(&str, &str)] = match flavor {
        Flavor::Postgres => &[
            (
                "Uptime",
                "SELECT EXTRACT(EPOCH FROM now() - pg_postmaster_start_time())::bigint::text || 's'",
            ),
            ("Sessions", "SELECT count(*)::text FROM pg_stat_activity WHERE datname IS NOT NULL"),
            ("Active queries", "SELECT count(*)::text FROM pg_stat_activity WHERE state = 'active'"),
            ("Max connections", "SELECT current_setting('max_connections')"),
            ("Database size", "SELECT pg_size_pretty(pg_database_size(current_database()))"),
            (
                "Cache hit ratio",
                "SELECT COALESCE(round(100.0 * blks_hit / NULLIF(blks_hit + blks_read, 0), 2)::text || '%', 'n/a') \
                 FROM pg_stat_database WHERE datname = current_database()",
            ),
            ("Server version", "SELECT current_setting('server_version')"),
        ],
        Flavor::Cockroach => &[
            ("Sessions", "SELECT count(*)::STRING FROM [SHOW CLUSTER SESSIONS]"),
            ("Active queries", "SELECT count(*)::STRING FROM [SHOW CLUSTER QUERIES]"),
            ("Live nodes", "SELECT count(*)::STRING FROM crdb_internal.gossip_nodes WHERE is_live"),
            ("Cluster version", "SHOW CLUSTER SETTING version"),
            ("Build", "SELECT version()"),
        ],
    };
    for (label, sql) in probes {
        let Ok(messages) = client.simple_query(sql).await else {
            continue;
        };
        if let Some(value) = data_rows(messages).first().and_then(|row| row.get(0).map(str::to_string)) {
            metrics.push(*label, value);
        }
    }
    if metrics.metrics.is_empty() {
        return Err(DbError::message("The server did not return any health metrics (missing privileges?)"));
    }
    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explain_statements_follow_each_flavor_and_strip_terminators() {
        assert_eq!(
            explain_statement(Flavor::Postgres, "SELECT 1;", true),
            "EXPLAIN (ANALYZE, BUFFERS) SELECT 1"
        );
        assert_eq!(explain_statement(Flavor::Cockroach, "SELECT 1", true), "EXPLAIN ANALYZE SELECT 1");
        assert_eq!(explain_statement(Flavor::Postgres, " SELECT 1 ; ", false), "EXPLAIN SELECT 1");
    }
}
