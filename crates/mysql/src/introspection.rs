use crate::client::mysql_error;
use cottondata_driver::{
    ColumnSummary, DbError, ForeignKeyInfo, InboundForeignKeyInfo, IndexInfo, RoutineSummary, SchemaObjects,
    SchemaSummary, TableDetail, TableKind, TableSummary,
};
use mysql::prelude::Queryable;
use mysql::Conn;
use std::collections::BTreeMap;

pub const SYSTEM_SCHEMAS: &[&str] = &["information_schema", "mysql", "performance_schema", "sys"];

pub fn is_system_schema(name: &str) -> bool {
    SYSTEM_SCHEMAS.iter().any(|system| system.eq_ignore_ascii_case(name))
}

pub(crate) fn list_schemas(conn: &mut Conn) -> Result<Vec<SchemaSummary>, DbError> {
    let rows: Vec<(String, Option<String>)> = conn
        .query(
            "SELECT schema_name, default_collation_name \
             FROM information_schema.schemata \
             ORDER BY schema_name",
        )
        .map_err(mysql_error)?;
    Ok(rows
        .into_iter()
        .map(|(name, default_collation)| SchemaSummary {
            is_system: is_system_schema(&name),
            name,
            default_collation,
        })
        .collect())
}

type ColumnRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

pub(crate) fn load_schema_objects(conn: &mut Conn, schema: &str) -> Result<SchemaObjects, DbError> {
    let table_rows: Vec<(String, String)> = conn
        .exec(
            "SELECT table_name, table_type \
             FROM information_schema.tables \
             WHERE table_schema = ? \
             ORDER BY table_name",
            (schema,),
        )
        .map_err(mysql_error)?;

    let column_rows: Vec<ColumnRow> = conn
        .exec(
            "SELECT table_name, column_name, data_type, column_type, \
                    column_key, is_nullable, extra, column_default, column_comment \
             FROM information_schema.columns \
             WHERE table_schema = ? \
             ORDER BY table_name, ordinal_position",
            (schema,),
        )
        .map_err(mysql_error)?;

    let routine_rows: Vec<(String, String)> = conn
        .exec(
            "SELECT routine_name, routine_type \
             FROM information_schema.routines \
             WHERE routine_schema = ? \
             ORDER BY routine_name",
            (schema,),
        )
        .unwrap_or_default();

    let mut columns_by_table: BTreeMap<String, Vec<ColumnSummary>> = BTreeMap::new();
    for (table, name, data_type, column_type, column_key, nullable, extra, default_expr, comment) in column_rows {
        columns_by_table.entry(table).or_default().push(ColumnSummary {
            name,
            data_type,
            column_type,
            is_primary_key: column_key.eq_ignore_ascii_case("PRI"),
            is_nullable: nullable.eq_ignore_ascii_case("YES"),
            is_auto_increment: extra.to_ascii_lowercase().contains("auto_increment"),
            default_expr,
            comment: comment.filter(|text| !text.is_empty()),
        });
    }

    let tables = table_rows
        .into_iter()
        .map(|(name, table_type)| TableSummary {
            columns: columns_by_table.remove(&name).unwrap_or_default(),
            kind: if table_type.eq_ignore_ascii_case("VIEW") {
                TableKind::View
            } else {
                TableKind::BaseTable
            },
            name,
        })
        .collect();
    let routines = routine_rows
        .into_iter()
        .map(|(name, kind)| RoutineSummary { name, kind })
        .collect();
    Ok(SchemaObjects { tables, routines })
}

pub(crate) fn load_table_detail(conn: &mut Conn, schema: &str, table: &str) -> Result<TableDetail, DbError> {
    let qualified = format!("`{}`.`{}`", schema.replace('`', "``"), table.replace('`', "``"));
    let ddl_row: Option<(String, String)> = conn
        .query_first(format!("SHOW CREATE TABLE {qualified}"))
        .map_err(mysql_error)?;
    let ddl = ddl_row.map(|(_, ddl)| ddl).unwrap_or_default();

    let meta_row: Option<(Option<String>, Option<String>, Option<u64>)> = conn
        .exec_first(
            "SELECT engine, table_collation, table_rows \
             FROM information_schema.tables \
             WHERE table_schema = ? AND table_name = ?",
            (schema, table),
        )
        .unwrap_or(None);
    let (engine, collation, row_estimate) = meta_row.unwrap_or((None, None, None));

    let index_rows: Vec<(String, u8, String, Option<String>)> = conn
        .exec(
            "SELECT index_name, non_unique, index_type, column_name \
             FROM information_schema.statistics \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY index_name, seq_in_index",
            (schema, table),
        )
        .unwrap_or_default();
    let mut indexes: BTreeMap<String, IndexInfo> = BTreeMap::new();
    for (name, non_unique, index_type, column) in index_rows {
        let entry = indexes.entry(name.clone()).or_insert_with(|| IndexInfo {
            name,
            non_unique: non_unique != 0,
            index_type,
            columns: Vec::new(),
        });
        if let Some(column) = column {
            entry.columns.push(column);
        }
    }

    let foreign_key_rows: Vec<(String, String, String, String, String, String, String)> = conn
        .exec(
            "SELECT k.constraint_name, k.column_name, k.referenced_table_schema, \
                    k.referenced_table_name, k.referenced_column_name, \
                    r.update_rule, r.delete_rule \
             FROM information_schema.key_column_usage k \
             JOIN information_schema.referential_constraints r \
               ON k.constraint_schema = r.constraint_schema \
              AND k.constraint_name = r.constraint_name \
             WHERE k.table_schema = ? AND k.table_name = ? \
               AND k.referenced_column_name IS NOT NULL \
             ORDER BY k.constraint_name, k.ordinal_position",
            (schema, table),
        )
        .unwrap_or_default();
    let foreign_keys = foreign_key_rows
        .into_iter()
        .map(
            |(constraint_name, column_name, ref_schema, ref_table, ref_column, update_rule, delete_rule)| {
                ForeignKeyInfo {
                    constraint_name,
                    column_name,
                    ref_schema,
                    ref_table,
                    ref_column,
                    update_rule,
                    delete_rule,
                }
            },
        )
        .collect();

    let inbound_foreign_key_rows: Vec<(String, String, String, String, String, String, String)> = conn
        .exec(
            "SELECT k.constraint_name, k.table_schema, k.table_name, k.column_name, \
                    k.referenced_column_name, r.update_rule, r.delete_rule \
             FROM information_schema.key_column_usage k \
             JOIN information_schema.referential_constraints r \
               ON k.constraint_schema = r.constraint_schema \
              AND k.constraint_name = r.constraint_name \
             WHERE k.referenced_table_schema = ? AND k.referenced_table_name = ? \
               AND k.referenced_column_name IS NOT NULL \
             ORDER BY k.table_schema, k.table_name, k.constraint_name, k.ordinal_position",
            (schema, table),
        )
        .unwrap_or_default();
    let inbound_foreign_keys = inbound_foreign_key_rows
        .into_iter()
        .map(
            |(constraint_name, source_schema, source_table, source_column, target_column, update_rule, delete_rule)| {
                InboundForeignKeyInfo {
                    constraint_name,
                    source_schema,
                    source_table,
                    source_column,
                    target_column,
                    update_rule,
                    delete_rule,
                }
            },
        )
        .collect();

    Ok(TableDetail {
        ddl,
        engine,
        collation,
        row_estimate,
        indexes: indexes.into_values().collect(),
        foreign_keys,
        inbound_foreign_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_standard_mysql_system_schemas_case_insensitively() {
        assert!(is_system_schema("information_schema"));
        assert!(is_system_schema("PERFORMANCE_SCHEMA"));
        assert!(is_system_schema("mysql"));
        assert!(!is_system_schema("app"));
    }
}
