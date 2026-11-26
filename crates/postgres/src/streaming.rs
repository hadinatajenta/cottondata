use crate::errors::postgres_error_for;
use crate::metadata::MetadataCache;
use crate::query::{cursor_body, is_cursor_compatible, CURSOR_NAME};
use crate::values::display_text;
use cottondata_driver::DbError;
use cottondata_grid::ColumnMeta;
use futures_util::{pin_mut, TryStreamExt};
use tokio_postgres::types::Type;
use tokio_postgres::{Client, SimpleQueryMessage, SimpleQueryRow};

const EXPORT_BATCH_ROWS: u32 = 2000;

type RowSink<'a> = &'a mut dyn FnMut(&[Option<String>]) -> Result<bool, String>;

#[allow(clippy::type_complexity)]
pub(crate) async fn stream_statement(
    client: &Client,
    cache: &mut MetadataCache,
    sql: &str,
    is_in_transaction: bool,
    is_cursor_supported: bool,
    on_columns: &mut dyn FnMut(&[ColumnMeta]) -> Result<(), String>,
    on_row: &mut dyn FnMut(&[Option<String>]) -> Result<bool, String>,
) -> Result<u64, DbError> {
    let statement = client
        .prepare(sql)
        .await
        .map_err(|error| postgres_error_for(error, Some(sql)))?;
    let metas = cache.describe_columns(client, statement.columns()).await;
    let types: Vec<Type> = statement.columns().iter().map(|column| column.type_().clone()).collect();
    on_columns(&metas).map_err(DbError::message)?;
    if metas.is_empty() {
        return Ok(0);
    }
    if !(is_cursor_supported && is_cursor_compatible(sql)) {
        return stream_simple(client, sql, &types, on_row).await;
    }

    let declare = format!(
        "{}DECLARE {CURSOR_NAME} NO SCROLL CURSOR FOR {}\n",
        if is_in_transaction { "" } else { "BEGIN;\n" },
        cursor_body(sql)
    );
    client
        .batch_execute(&declare)
        .await
        .map_err(|error| postgres_error_for(error, Some(sql)))?;
    let outcome = stream_cursor_batches(client, &types, on_row).await;
    let close = if is_in_transaction {
        format!("CLOSE {CURSOR_NAME}")
    } else if outcome.is_ok() {
        format!("CLOSE {CURSOR_NAME}; COMMIT")
    } else {
        "ROLLBACK".to_string()
    };
    let closed = client.batch_execute(&close).await;
    let rows = outcome?;
    closed.map_err(|error| postgres_error_for(error, Some(sql)))?;
    Ok(rows)
}

async fn stream_cursor_batches(
    client: &Client,
    types: &[Type],
    on_row: RowSink<'_>,
) -> Result<u64, DbError> {
    let fetch = format!("FETCH FORWARD {EXPORT_BATCH_ROWS} FROM {CURSOR_NAME}");
    let mut rows_streamed = 0u64;
    let mut row_buffer: Vec<Option<String>> = Vec::with_capacity(types.len());
    loop {
        let messages = client
            .simple_query(&fetch)
            .await
            .map_err(|error| postgres_error_for(error, None))?;
        let mut batch_rows = 0u32;
        for message in messages {
            if let SimpleQueryMessage::Row(row) = message {
                batch_rows += 1;
                fill_optional_text(&row, types, &mut row_buffer);
                rows_streamed += 1;
                if !on_row(&row_buffer).map_err(DbError::message)? {
                    return Ok(rows_streamed);
                }
            }
        }
        if batch_rows < EXPORT_BATCH_ROWS {
            return Ok(rows_streamed);
        }
    }
}

async fn stream_simple(
    client: &Client,
    sql: &str,
    types: &[Type],
    on_row: RowSink<'_>,
) -> Result<u64, DbError> {
    let stream = client
        .simple_query_raw(sql)
        .await
        .map_err(|error| postgres_error_for(error, Some(sql)))?;
    pin_mut!(stream);
    let mut rows_streamed = 0u64;
    let mut keep_going = true;
    let mut row_buffer: Vec<Option<String>> = Vec::with_capacity(types.len());
    while let Some(message) = stream
        .try_next()
        .await
        .map_err(|error| postgres_error_for(error, Some(sql)))?
    {
        match message {
            SimpleQueryMessage::Row(row) if keep_going => {
                fill_optional_text(&row, types, &mut row_buffer);
                rows_streamed += 1;
                keep_going = on_row(&row_buffer).map_err(DbError::message)?;
            }
            SimpleQueryMessage::CommandComplete(_) if rows_streamed > 0 || !keep_going => keep_going = false,
            _ => {}
        }
    }
    Ok(rows_streamed)
}

fn fill_optional_text(row: &SimpleQueryRow, types: &[Type], buffer: &mut Vec<Option<String>>) {
    buffer.clear();
    for index in 0..row.len() {
        buffer.push(row.get(index).map(|raw| display_text(types.get(index), raw)));
    }
}
