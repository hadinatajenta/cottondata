use crate::values::{is_binary_type, is_numeric_type, type_label};
use cottondata_grid::ColumnMeta;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use tokio_postgres::{Client, Column};

const TABLE_ATTRIBUTES_SQL: &str = "SELECT n.nspname, c.relname, a.attnum, a.attname, NOT a.attnotnull, \
        COALESCE(bool_or(i.indisprimary), false), \
        COALESCE(bool_or(i.indisunique AND i.indnatts = 1), false), \
        (COALESCE(to_jsonb(a) ->> 'attidentity', '') <> '' OR COALESCE(pg_get_expr(d.adbin, d.adrelid), '') LIKE 'nextval(%') \
     FROM pg_class c \
     JOIN pg_namespace n ON n.oid = c.relnamespace \
     JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
     LEFT JOIN pg_index i ON i.indrelid = c.oid AND a.attnum = ANY(i.indkey) \
     LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
     WHERE c.oid = $1 \
     GROUP BY n.nspname, c.relname, a.attnum, a.attname, a.attnotnull, to_jsonb(a) ->> 'attidentity', d.adbin, d.adrelid";

const TABLE_NAMES_SQL: &str = "SELECT n.nspname, c.relname, a.attnum, a.attname \
     FROM pg_class c \
     JOIN pg_namespace n ON n.oid = c.relnamespace \
     JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 \
     WHERE c.oid = $1";

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttributeInfo {
    name: String,
    is_nullable: bool,
    is_primary_key: bool,
    is_unique_key: bool,
    is_auto_increment: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TableInfo {
    schema: String,
    table: String,
    attributes: HashMap<i16, AttributeInfo>,
}

#[derive(Default)]
pub(crate) struct MetadataCache {
    tables: HashMap<u32, Option<TableInfo>>,
}

impl MetadataCache {
    pub(crate) fn clear(&mut self) {
        self.tables.clear();
    }

    pub(crate) async fn describe_columns(&mut self, client: &Client, columns: &[Column]) -> Vec<ColumnMeta> {
        let mut described = Vec::with_capacity(columns.len());
        for column in columns {
            let source = match (column.table_oid(), column.column_id()) {
                (Some(table_oid), Some(column_id)) if table_oid != 0 && column_id > 0 => self
                    .table_info(client, table_oid)
                    .await
                    .and_then(|table| table.attributes.get(&column_id).map(|attribute| (table, attribute))),
                _ => None,
            };
            described.push(build_column_meta(column, source));
        }
        described
    }

    async fn table_info(&mut self, client: &Client, table_oid: u32) -> Option<&TableInfo> {
        if let Entry::Vacant(slot) = self.tables.entry(table_oid) {
            slot.insert(load_table_info(client, table_oid).await);
        }
        self.tables.get(&table_oid).and_then(Option::as_ref)
    }
}

fn build_column_meta(column: &Column, source: Option<(&TableInfo, &AttributeInfo)>) -> ColumnMeta {
    let column_type = column.type_();
    let mut meta = ColumnMeta {
        name: column.name().to_string(),
        org_name: None,
        table: None,
        schema: None,
        type_name: type_label(column_type),
        is_primary_key: false,
        is_unique_key: false,
        is_nullable: true,
        is_auto_increment: false,
        is_numeric: is_numeric_type(column_type),
        is_binary: is_binary_type(column_type),
    };
    if let Some((table, attribute)) = source {
        meta.org_name = Some(attribute.name.clone());
        meta.table = Some(table.table.clone());
        meta.schema = Some(table.schema.clone());
        meta.is_primary_key = attribute.is_primary_key;
        meta.is_unique_key = attribute.is_unique_key;
        meta.is_nullable = attribute.is_nullable;
        meta.is_auto_increment = attribute.is_auto_increment;
    }
    meta
}

async fn load_table_info(client: &Client, table_oid: u32) -> Option<TableInfo> {
    if let Ok(rows) = client.query(TABLE_ATTRIBUTES_SQL, &[&table_oid]).await {
        return table_info_from_rows(&rows, true);
    }
    let rows = client.query(TABLE_NAMES_SQL, &[&table_oid]).await.ok()?;
    table_info_from_rows(&rows, false)
}

fn table_info_from_rows(rows: &[tokio_postgres::Row], has_key_flags: bool) -> Option<TableInfo> {
    let first = rows.first()?;
    let mut info = TableInfo {
        schema: first.try_get::<_, String>(0).ok()?,
        table: first.try_get::<_, String>(1).ok()?,
        attributes: HashMap::with_capacity(rows.len()),
    };
    for row in rows {
        let Ok(attnum) = row.try_get::<_, i16>(2) else {
            continue;
        };
        let Ok(name) = row.try_get::<_, String>(3) else {
            continue;
        };
        let flag = |index: usize| has_key_flags && row.try_get::<_, bool>(index).unwrap_or(false);
        info.attributes.insert(
            attnum,
            AttributeInfo {
                name,
                is_nullable: !has_key_flags || flag(4),
                is_primary_key: flag(5),
                is_unique_key: flag(6),
                is_auto_increment: flag(7),
            },
        );
    }
    Some(info)
}
