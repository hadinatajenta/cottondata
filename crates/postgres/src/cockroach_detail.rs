use crate::errors::postgres_error;
use crate::rows::{rows, text};
use crate::values::is_true_text;
use cottondata_core::SqlDialect;
use cottondata_driver::{DbError, ForeignKeyInfo, InboundForeignKeyInfo, IndexInfo, TableDetail};
use std::collections::BTreeMap;
use tokio_postgres::Client;

pub(crate) async fn load_cockroach_table_detail(client: &Client, schema: &str, table: &str) -> Result<TableDetail, DbError> {
    let dialect = SqlDialect::CockroachDb;
    let qualified = dialect.qualified_table(schema, table);
    let ddl = client
        .simple_query(&format!("SHOW CREATE TABLE {qualified}"))
        .await
        .map_err(postgres_error)?
        .into_iter()
        .find_map(|message| match message {
            tokio_postgres::SimpleQueryMessage::Row(row) => row
                .try_get("create_statement")
                .ok()
                .flatten()
                .map(str::to_string),
            _ => None,
        })
        .map(|statement| if statement.ends_with(';') { statement } else { format!("{statement};") })
        .ok_or_else(|| DbError::message(format!("Table {schema}.{table} was not found")))?;

    let mut indexes: BTreeMap<String, IndexInfo> = BTreeMap::new();
    if let Ok(messages) = client.simple_query(&format!("SHOW INDEXES FROM {qualified}")).await {
        for message in messages {
            let tokio_postgres::SimpleQueryMessage::Row(row) = message else {
                continue;
            };
            let column_text = |name: &str| row.try_get(name).ok().flatten().map(str::to_string);
            let is_storing = is_true_text(column_text("storing").as_deref());
            let is_implicit = is_true_text(column_text("implicit").as_deref());
            let (Some(name), Some(column)) = (column_text("index_name"), column_text("column_name")) else {
                continue;
            };
            if is_storing || is_implicit {
                continue;
            }
            let is_primary = name == "primary" || name.ends_with("_pkey");
            indexes
                .entry(name.clone())
                .or_insert_with(|| IndexInfo {
                    name,
                    non_unique: is_true_text(column_text("non_unique").as_deref()),
                    index_type: if is_primary { "PRIMARY KEY".into() } else { "BTREE".into() },
                    columns: Vec::new(),
                })
                .columns
                .push(column);
        }
    }

    let foreign_keys = rows(
        client,
        "SELECT rc.constraint_name::text, kcu.column_name::text, pk.table_schema::text, pk.table_name::text, \
                pk.column_name::text, rc.update_rule::text, rc.delete_rule::text \
         FROM information_schema.referential_constraints rc \
         JOIN information_schema.key_column_usage kcu \
           ON kcu.constraint_schema = rc.constraint_schema AND kcu.constraint_name = rc.constraint_name \
         JOIN information_schema.key_column_usage pk \
           ON pk.constraint_schema = rc.unique_constraint_schema \
          AND pk.constraint_name = rc.unique_constraint_name \
          AND pk.ordinal_position = kcu.position_in_unique_constraint \
         WHERE kcu.table_schema = $1 AND kcu.table_name = $2 \
         ORDER BY rc.constraint_name, kcu.ordinal_position",
        &[&schema, &table],
    )
    .await
    .unwrap_or_default()
    .iter()
    .map(|row| ForeignKeyInfo {
        constraint_name: text(row, 0),
        column_name: text(row, 1),
        ref_schema: text(row, 2),
        ref_table: text(row, 3),
        ref_column: text(row, 4),
        update_rule: text(row, 5),
        delete_rule: text(row, 6),
    })
    .collect();

    let inbound_foreign_keys = rows(
        client,
        "SELECT rc.constraint_name::text, kcu.table_schema::text, kcu.table_name::text, kcu.column_name::text, \
                pk.column_name::text, rc.update_rule::text, rc.delete_rule::text \
         FROM information_schema.referential_constraints rc \
         JOIN information_schema.key_column_usage kcu \
           ON kcu.constraint_schema = rc.constraint_schema AND kcu.constraint_name = rc.constraint_name \
         JOIN information_schema.key_column_usage pk \
           ON pk.constraint_schema = rc.unique_constraint_schema \
          AND pk.constraint_name = rc.unique_constraint_name \
          AND pk.ordinal_position = kcu.position_in_unique_constraint \
         WHERE pk.table_schema = $1 AND pk.table_name = $2 \
         ORDER BY kcu.table_schema, kcu.table_name, rc.constraint_name, kcu.ordinal_position",
        &[&schema, &table],
    )
    .await
    .unwrap_or_default()
    .iter()
    .map(|row| InboundForeignKeyInfo {
        constraint_name: text(row, 0),
        source_schema: text(row, 1),
        source_table: text(row, 2),
        source_column: text(row, 3),
        target_column: text(row, 4),
        update_rule: text(row, 5),
        delete_rule: text(row, 6),
    })
    .collect();

    Ok(TableDetail {
        ddl,
        engine: None,
        collation: None,
        row_estimate: None,
        indexes: indexes.into_values().collect(),
        foreign_keys,
        inbound_foreign_keys,
    })
}
