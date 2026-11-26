use crate::errors::postgres_error;
use cottondata_driver::DbError;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Row};

pub(crate) fn text(row: &Row, index: usize) -> String {
    optional_text(row, index).unwrap_or_default()
}

pub(crate) fn optional_text(row: &Row, index: usize) -> Option<String> {
    row.try_get::<_, Option<String>>(index).ok().flatten()
}

pub(crate) fn flag(row: &Row, index: usize) -> bool {
    row.try_get::<_, Option<bool>>(index).ok().flatten().unwrap_or(false)
}

pub(crate) async fn rows(client: &Client, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<Vec<Row>, DbError> {
    client.query(sql, params).await.map_err(postgres_error)
}
