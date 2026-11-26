use crate::errors::mssql_error;
use crate::rows::MssqlClient;
use crate::values::{cell_value, column_type_label, is_numeric_type};
use cottondata_driver::DbError;
use cottondata_grid::{CellValue, ColumnMeta, ColumnarPage};
use futures_util::TryStreamExt;
use tiberius::{Column, QueryItem};

fn column_meta(column: &Column) -> ColumnMeta {
    let column_type = column.column_type();
    ColumnMeta {
        name: column.name().to_string(),
        table: None,
        schema: None,
        type_name: column_type_label(column_type).to_string(),
        is_primary_key: false,
        is_nullable: true,
        is_auto_increment: false,
        is_numeric: is_numeric_type(column_type),
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
