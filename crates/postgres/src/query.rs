use crate::errors::postgres_error_for;
use crate::metadata::MetadataCache;
use crate::values::display_text;
use cottondata_core::{SqlDialect, UNLIMITED_ROWS};
use cottondata_driver::DbError;
use cottondata_grid::{CellValue, ColumnMeta, ColumnarPage};
use cottondata_sql::{build_paginated_query, has_explicit_row_limit, tokenize, TokenKind};
use futures_util::{pin_mut, TryStreamExt};
use tokio_postgres::error::SqlState;
use tokio_postgres::types::Type;
use tokio_postgres::{Client, SimpleQueryMessage, SimpleQueryRow};

pub(crate) const CURSOR_NAME: &str = "cottondata_cursor";


pub(crate) type OverflowCanceller = dyn Fn() -> std::thread::JoinHandle<()> + Send + Sync;

pub(crate) struct ExecutionOptions<'a> {
    pub(crate) max_rows: u32,
    pub(crate) is_in_transaction: bool,
    pub(crate) is_cursor_supported: bool,
    pub(crate) overflow_canceller: Option<&'a OverflowCanceller>,
}

pub(crate) struct ExecutionResult {
    pub(crate) page: Option<ColumnarPage>,
    pub(crate) affected_rows: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransactionEffect {
    None,
    Begins,
    Ends,
}

pub(crate) fn transaction_effect(sql: &str) -> TransactionEffect {
    let words = leading_words(sql, 2);
    match words.first().map(String::as_str) {
        Some("BEGIN" | "START") => TransactionEffect::Begins,
        Some("COMMIT" | "END" | "ABORT") => TransactionEffect::Ends,
        Some("ROLLBACK") if words.get(1).map(String::as_str) != Some("TO") => TransactionEffect::Ends,
        Some("PREPARE") if words.get(1).map(String::as_str) == Some("TRANSACTION") => TransactionEffect::Ends,
        _ => TransactionEffect::None,
    }
}

pub(crate) fn is_cursor_compatible(sql: &str) -> bool {
    let tokens: Vec<_> = tokenize(sql)
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    let Some(first) = tokens.first() else {
        return false;
    };
    let leading = first.text.to_ascii_uppercase();
    if !matches!(leading.as_str(), "SELECT" | "WITH" | "VALUES" | "TABLE" | "(") {
        return false;
    }
    let has_side_effect_keyword = tokens.iter().any(|token| {
        matches!(token.kind, TokenKind::Keyword | TokenKind::Identifier)
            && matches!(
                token.text.to_ascii_uppercase().as_str(),
                "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "INTO"
            )
    });
    let statement_count = tokens.iter().filter(|token| token.text == ";").count();
    let ends_with_single_terminator = statement_count == 0
        || (statement_count == 1 && tokens.last().is_some_and(|token| token.text == ";"));
    !has_side_effect_keyword && ends_with_single_terminator
}

pub(crate) fn cursor_body(sql: &str) -> &str {
    sql.trim().trim_end_matches(|c: char| c == ';' || c.is_whitespace())
}

fn leading_words(sql: &str, limit: usize) -> Vec<String> {
    tokenize(sql)
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
        .take(limit)
        .map(|token| token.text.to_ascii_uppercase())
        .collect()
}

pub(crate) async fn execute_statement(
    client: &Client,
    cache: &mut MetadataCache,
    sql: &str,
    options: &ExecutionOptions<'_>,
) -> Result<ExecutionResult, DbError> {
    let max_rows = options.max_rows;
    let unlimited = max_rows == UNLIMITED_ROWS;
    let prepared = client.prepare(sql).await.ok();
    let Some(statement) = prepared.filter(|statement| !statement.columns().is_empty()) else {
        return run_simple(client, sql, sql, max_rows, None, None, None).await;
    };
    let metas = cache.describe_columns(client, statement.columns()).await;
    let types: Vec<Type> = statement.columns().iter().map(|column| column.type_().clone()).collect();
    let is_plain_read = is_cursor_compatible(sql);
    if !unlimited && options.is_cursor_supported && is_plain_read {
        let page = fetch_through_cursor(client, sql, max_rows, options.is_in_transaction, metas, &types).await?;
        return Ok(ExecutionResult {
            page: Some(page),
            affected_rows: 0,
        });
    }
    if !unlimited && !options.is_cursor_supported && is_plain_read && !has_explicit_row_limit(sql) {
        let fetch_limit = max_rows.max(1).saturating_add(1);
        if let Some(limited_sql) = build_paginated_query(SqlDialect::CockroachDb, sql, fetch_limit, 0) {
            return run_simple(client, &limited_sql, sql, max_rows, Some(metas), Some(&types), None).await;
        }
    }
    let overflow_canceller = (!unlimited)
        .then_some(options.overflow_canceller)
        .flatten()
        .filter(|_| is_plain_read);
    run_simple(client, sql, sql, max_rows, Some(metas), Some(&types), overflow_canceller).await
}

async fn fetch_through_cursor(
    client: &Client,
    sql: &str,
    max_rows: u32,
    is_in_transaction: bool,
    metas: Vec<ColumnMeta>,
    types: &[Type],
) -> Result<ColumnarPage, DbError> {
    let row_cap = max_rows.max(1);
    let (begin, commit) = if is_in_transaction { ("", "") } else { ("BEGIN;\n", "\nCOMMIT;") };
    let script = format!(
        "{begin}DECLARE {CURSOR_NAME} NO SCROLL CURSOR FOR {}\n;\nFETCH FORWARD {} FROM {CURSOR_NAME};\nCLOSE {CURSOR_NAME};{commit}",
        cursor_body(sql),
        row_cap.saturating_add(1)
    );
    let messages = match client.simple_query(&script).await {
        Ok(messages) => messages,
        Err(error) => {
            if !is_in_transaction {
                let _ = client.batch_execute("ROLLBACK").await;
            }
            return Err(postgres_error_for(error, Some(sql)));
        }
    };
    let mut page = ColumnarPage::new(metas, sql.to_string());
    let mut row_buffer = Vec::with_capacity(types.len());
    for message in messages {
        if let SimpleQueryMessage::Row(row) = message {
            if page.row_count >= row_cap as usize {
                page.has_more_rows = true;
                continue;
            }
            fill_cells(&row, types, &mut row_buffer);
            page.push_row(&row_buffer);
        }
    }
    Ok(page)
}

async fn run_simple(
    client: &Client,
    exec_sql: &str,
    display_sql: &str,
    max_rows: u32,
    metas: Option<Vec<ColumnMeta>>,
    types: Option<&[Type]>,
    overflow_canceller: Option<&OverflowCanceller>,
) -> Result<ExecutionResult, DbError> {
    let row_cap = if max_rows == UNLIMITED_ROWS {
        usize::MAX
    } else {
        max_rows.max(1) as usize
    };
    let stream = client
        .simple_query_raw(exec_sql)
        .await
        .map_err(|error| postgres_error_for(error, Some(display_sql)))?;
    pin_mut!(stream);
    let mut overflow_cancel_thread: Option<std::thread::JoinHandle<()>> = None;
    let mut prepared_metas = metas;
    let mut page: Option<ColumnarPage> = None;
    let mut is_first_set_complete = false;
    let mut affected_rows = 0u64;
    let mut row_buffer = Vec::new();
    let empty_types: &[Type] = &[];
    loop {
        let message = match stream.try_next().await {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(error) if overflow_cancel_thread.is_some() && error.code() == Some(&SqlState::QUERY_CANCELED) => break,
            Err(error) => {
                if let Some(thread) = overflow_cancel_thread.take() {
                    let _ = thread.join();
                }
                return Err(postgres_error_for(error, Some(display_sql)));
            }
        };
        match message {
            SimpleQueryMessage::RowDescription(columns) if page.is_none() => {
                let metas = prepared_metas
                    .take()
                    .filter(|metas| metas.len() == columns.len())
                    .unwrap_or_else(|| columns.iter().map(|column| text_column_meta(column.name())).collect());
                page = Some(ColumnarPage::new(metas, display_sql.to_string()));
            }
            SimpleQueryMessage::RowDescription(_) => {}
            SimpleQueryMessage::Row(row) => {
                let Some(page) = page.as_mut().filter(|_| !is_first_set_complete) else {
                    continue;
                };
                if page.row_count >= row_cap {
                    page.has_more_rows = true;
                    if overflow_cancel_thread.is_none() {
                        overflow_cancel_thread = overflow_canceller.map(|cancel| cancel());
                    }
                    continue;
                }
                fill_cells(&row, types.unwrap_or(empty_types), &mut row_buffer);
                page.push_row(&row_buffer);
            }
            SimpleQueryMessage::CommandComplete(count) => {
                if page.is_some() {
                    is_first_set_complete = true;
                } else {
                    affected_rows = affected_rows.saturating_add(count);
                }
            }
            _ => {}
        }
    }
    if let Some(thread) = overflow_cancel_thread {
        let _ = thread.join();
    }
    Ok(ExecutionResult { page, affected_rows })
}

fn fill_cells(row: &SimpleQueryRow, types: &[Type], buffer: &mut Vec<CellValue>) {
    buffer.clear();
    for index in 0..row.len() {
        buffer.push(match row.get(index) {
            Some(raw) => CellValue::from_text(display_text(types.get(index), raw)),
            None => CellValue::Null,
        });
    }
}

fn text_column_meta(name: &str) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        org_name: None,
        table: None,
        schema: None,
        type_name: "text".into(),
        is_primary_key: false,
        is_unique_key: false,
        is_nullable: true,
        is_auto_increment: false,
        is_numeric: false,
        is_binary: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_reads_use_a_cursor_and_writes_or_scripts_do_not() {
        assert!(is_cursor_compatible("SELECT * FROM orders"));
        assert!(is_cursor_compatible("  with x as (select 1) select * from x;  "));
        assert!(is_cursor_compatible("VALUES (1), (2)"));
        assert!(is_cursor_compatible("(SELECT 1) UNION (SELECT 2)"));
        assert!(!is_cursor_compatible("WITH gone AS (DELETE FROM t RETURNING *) SELECT * FROM gone"));
        assert!(!is_cursor_compatible("SELECT * INTO backup FROM t"));
        assert!(!is_cursor_compatible("INSERT INTO t VALUES (1) RETURNING id"));
        assert!(!is_cursor_compatible("SHOW search_path"));
        assert!(!is_cursor_compatible("EXPLAIN SELECT 1"));
        assert!(!is_cursor_compatible("SELECT 1; SELECT 2"));
    }

    #[test]
    fn cursor_body_strips_trailing_terminators_but_keeps_comments_safe() {
        assert_eq!(cursor_body("SELECT 1 ;  \n"), "SELECT 1");
        assert_eq!(cursor_body("SELECT 1 -- note"), "SELECT 1 -- note");
    }

    #[test]
    fn transaction_control_statements_update_tracking() {
        assert_eq!(transaction_effect("begin"), TransactionEffect::Begins);
        assert_eq!(transaction_effect("START TRANSACTION READ ONLY"), TransactionEffect::Begins);
        assert_eq!(transaction_effect("commit;"), TransactionEffect::Ends);
        assert_eq!(transaction_effect("ROLLBACK"), TransactionEffect::Ends);
        assert_eq!(transaction_effect("ROLLBACK TO SAVEPOINT sp"), TransactionEffect::None);
        assert_eq!(transaction_effect("PREPARE TRANSACTION 'x'"), TransactionEffect::Ends);
        assert_eq!(transaction_effect("/* hi */ SELECT 1"), TransactionEffect::None);
    }
}
