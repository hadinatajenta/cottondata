use crate::errors::mssql_error;
use crate::rows::MssqlClient;
use crate::values::{cell_value, column_text, column_type_label, is_numeric_type};
use cottondata_driver::DbError;
use cottondata_grid::{CellValue, ColumnMeta, ColumnarPage};
use futures_util::TryStreamExt;
use tiberius::{Column, QueryItem};

fn column_meta(column: &Column) -> ColumnMeta {
    let column_type = column.column_type();
    ColumnMeta {
        name: column.name().to_string(),
        org_name: None,
        table: None,
        schema: None,
        type_name: column_type_label(column_type).to_string(),
        is_primary_key: false,
        is_unique_key: false,
        is_nullable: true,
        is_auto_increment: false,
        is_numeric: is_numeric_type(column_type),
        is_binary: false,
    }
}

pub(crate) async fn read_first_result_set(
    client: &mut MssqlClient,
    sql: &str,
    max_rows: u32,
) -> Result<Option<ColumnarPage>, DbError> {
    let mut stream = client.simple_query(sql).await.map_err(mssql_error)?;
    let row_cap = max_rows.max(1) as usize;
    let mut page: Option<ColumnarPage> = None;
    let mut is_first_set_complete = false;
    let mut row_buffer: Vec<CellValue> = Vec::new();
    while let Some(item) = stream.try_next().await.map_err(mssql_error)? {
        match item {
            QueryItem::Metadata(metadata) if page.is_none() && !metadata.columns().is_empty() => {
                let columns = metadata.columns().iter().map(column_meta).collect();
                page = Some(ColumnarPage::new(columns, sql.to_string()));
            }
            QueryItem::Metadata(_) => is_first_set_complete = page.is_some(),
            QueryItem::Row(row) => {
                let Some(page) = page.as_mut().filter(|_| !is_first_set_complete) else {
                    continue;
                };
                if page.row_count >= row_cap {
                    page.has_more_rows = true;
                    continue;
                }
                row_buffer.clear();
                row_buffer.extend(row.cells().map(|(_, data)| cell_value(data)));
                page.push_row(&row_buffer);
            }
        }
    }
    Ok(page)
}

#[allow(clippy::type_complexity)]
pub(crate) async fn stream_first_result_set(
    client: &mut MssqlClient,
    sql: &str,
    on_columns: &mut dyn FnMut(&[ColumnMeta]) -> Result<(), String>,
    on_row: &mut dyn FnMut(&[Option<String>]) -> Result<bool, String>,
) -> Result<(u64, bool), DbError> {
    let mut stream = client.simple_query(sql).await.map_err(mssql_error)?;
    let mut columns_reported = false;
    let mut rows_streamed: u64 = 0;
    let mut stopped_early = false;
    let mut row_buf: Vec<Option<String>> = Vec::new();

    while let Some(item) = stream.try_next().await.map_err(mssql_error)? {
        match item {
            QueryItem::Metadata(metadata) if !columns_reported && !metadata.columns().is_empty() => {
                let columns: Vec<ColumnMeta> = metadata.columns().iter().map(column_meta).collect();
                row_buf.reserve(columns.len());
                on_columns(&columns).map_err(DbError::message)?;
                columns_reported = true;
            }
            QueryItem::Metadata(_) if columns_reported => {
                stopped_early = true;
                break;
            }
            QueryItem::Metadata(_) => {}
            QueryItem::Row(row) => {
                if !columns_reported {
                    continue;
                }
                row_buf.clear();
                row_buf.extend(row.cells().map(|(_, data)| column_text(data)));
                let keep_going = on_row(&row_buf).map_err(DbError::message)?;
                rows_streamed += 1;
                if !keep_going {
                    stopped_early = true;
                    break;
                }
            }
        }
    }

    if !columns_reported {
        on_columns(&[]).map_err(DbError::message)?;
    }

    Ok((rows_streamed, !stopped_early))
}
