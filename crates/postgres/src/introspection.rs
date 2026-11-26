use crate::rows::{flag, optional_text, rows, text};
use cottondata_core::SqlDialect;
use cottondata_driver::{ColumnSummary, DbError, RoutineSummary, SchemaObjects, SchemaSummary, TableKind, TableSummary};
use std::collections::{BTreeMap, HashSet};
use tokio_postgres::Client;

const SYSTEM_SCHEMAS: [&str; 6] = [
    "pg_catalog",
    "information_schema",
    "pg_toast",
    "crdb_internal",
    "pg_extension",
    "pg_statistic",
];

pub fn is_system_schema(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SYSTEM_SCHEMAS.contains(&lower.as_str()) || lower.starts_with("pg_temp_") || lower.starts_with("pg_toast_temp_")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    Postgres,
    Cockroach,
}

impl Flavor {
    pub(crate) fn dialect(self) -> SqlDialect {
        match self {
            Self::Postgres => SqlDialect::PostgreSql,
            Self::Cockroach => SqlDialect::CockroachDb,
        }
    }
}

pub(crate) async fn list_schemas(client: &Client) -> Result<Vec<SchemaSummary>, DbError> {
    let schema_rows = rows(
        client,
        "SELECT schema_name::text FROM information_schema.schemata ORDER BY schema_name",
        &[],
    )
    .await?;
    let mut seen = HashSet::new();
    Ok(schema_rows
        .iter()
        .map(|row| text(row, 0))
        .filter(|name| seen.insert(name.clone()))
        .map(|name| SchemaSummary {
            is_system: is_system_schema(&name),
            default_collation: None,
            name,
        })
        .collect())
}

pub(crate) async fn load_schema_objects(client: &Client, flavor: Flavor, schema: &str) -> Result<SchemaObjects, DbError> {
    let table_rows = rows(
        client,
        "SELECT table_name::text, table_type::text FROM information_schema.tables \
         WHERE table_schema = $1 ORDER BY table_name",
        &[&schema],
    )
    .await?;
    let materialized_rows = if flavor == Flavor::Postgres {
        rows(
            client,
            "SELECT matviewname::text FROM pg_matviews WHERE schemaname = $1 ORDER BY matviewname",
            &[&schema],
        )
        .await
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut columns_by_table = load_columns(client, flavor, schema).await?;
    let primary_keys = load_primary_key_columns(client, schema).await.unwrap_or_default();
    for (table_name, columns) in columns_by_table.iter_mut() {
        for column in columns.iter_mut() {
            column.is_primary_key = primary_keys.contains(&(table_name.clone(), column.name.clone()));
        }
    }

    let mut tables: Vec<TableSummary> = table_rows
        .iter()
        .map(|row| {
            let name = text(row, 0);
            TableSummary {
                kind: if text(row, 1).contains("VIEW") {
                    TableKind::View
                } else {
                    TableKind::BaseTable
                },
                columns: columns_by_table.remove(&name).unwrap_or_default(),
                name,
            }
        })
        .collect();
    for row in &materialized_rows {
        let name = text(row, 0);
        if tables.iter().any(|table| table.name == name) {
            continue;
        }
        tables.push(TableSummary {
            kind: TableKind::View,
            columns: columns_by_table.remove(&name).unwrap_or_default(),
            name,
        });
    }
    tables.sort_by(|left, right| left.name.cmp(&right.name));

    let routine_rows = rows(
        client,
        "SELECT DISTINCT routine_name::text, routine_type::text FROM information_schema.routines \
         WHERE routine_schema = $1 ORDER BY routine_name",
        &[&schema],
    )
    .await
    .unwrap_or_default();
    let routines = routine_rows
        .iter()
        .map(|row| RoutineSummary {
            name: text(row, 0),
            kind: optional_text(row, 1).unwrap_or_else(|| "FUNCTION".into()),
        })
        .collect();
    Ok(SchemaObjects { tables, routines })
}

async fn load_columns(
    client: &Client,
    flavor: Flavor,
    schema: &str,
) -> Result<BTreeMap<String, Vec<ColumnSummary>>, DbError> {
    let mut by_table: BTreeMap<String, Vec<ColumnSummary>> = BTreeMap::new();
    match flavor {
        Flavor::Postgres => {
            let column_rows = rows(
                client,
                "SELECT c.relname::text, a.attname::text, format_type(a.atttypid, a.atttypmod), t.typname::text, \
                        NOT a.attnotnull, pg_get_expr(d.adbin, d.adrelid), \
                        (COALESCE(to_jsonb(a) ->> 'attidentity', '') <> '' OR COALESCE(pg_get_expr(d.adbin, d.adrelid), '') LIKE 'nextval(%'), \
                        col_description(c.oid, a.attnum) \
                 FROM pg_attribute a \
                 JOIN pg_class c ON c.oid = a.attrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 JOIN pg_type t ON t.oid = a.atttypid \
                 LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
                 WHERE n.nspname = $1 AND c.relkind IN ('r', 'p', 'v', 'm', 'f') \
                   AND a.attnum > 0 AND NOT a.attisdropped \
                 ORDER BY c.relname, a.attnum",
                &[&schema],
            )
            .await?;
            for row in &column_rows {
                by_table.entry(text(row, 0)).or_default().push(ColumnSummary {
                    name: text(row, 1),
                    column_type: text(row, 2),
                    data_type: text(row, 3),
                    is_primary_key: false,
                    is_nullable: flag(row, 4),
                    is_auto_increment: flag(row, 6),
                    default_expr: optional_text(row, 5),
                    comment: optional_text(row, 7),
                });
            }
        }
        Flavor::Cockroach => {
            let column_rows = rows(
                client,
                "SELECT table_name::text, column_name::text, crdb_sql_type::text, data_type::text, \
                        is_nullable::text, column_default::text, is_identity::text, column_comment::text \
                 FROM information_schema.columns \
                 WHERE table_schema = $1 AND is_hidden = 'NO' \
                 ORDER BY table_name, ordinal_position",
                &[&schema],
            )
            .await?;
            for row in &column_rows {
                let default_expr = optional_text(row, 5);
                let is_sequence_default = default_expr
                    .as_deref()
                    .is_some_and(|expr| expr.starts_with("nextval(") || expr.contains("unique_rowid()"));
                by_table.entry(text(row, 0)).or_default().push(ColumnSummary {
                    name: text(row, 1),
                    column_type: text(row, 2).to_ascii_lowercase(),
                    data_type: text(row, 3),
                    is_primary_key: false,
                    is_nullable: text(row, 4).eq_ignore_ascii_case("YES"),
                    is_auto_increment: is_sequence_default || text(row, 6).eq_ignore_ascii_case("YES"),
                    default_expr,
                    comment: optional_text(row, 7),
                });
            }
        }
    }
    Ok(by_table)
}

async fn load_primary_key_columns(client: &Client, schema: &str) -> Result<HashSet<(String, String)>, DbError> {
    let key_rows = rows(
        client,
        "SELECT kcu.table_name::text, kcu.column_name::text \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON kcu.constraint_schema = tc.constraint_schema \
          AND kcu.constraint_name = tc.constraint_name \
          AND kcu.table_name = tc.table_name \
         WHERE tc.constraint_type = 'PRIMARY KEY' AND tc.table_schema = $1",
        &[&schema],
    )
    .await?;
    Ok(key_rows.iter().map(|row| (text(row, 0), text(row, 1))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_schemas_include_catalogs_toast_and_temp_namespaces() {
        assert!(is_system_schema("pg_catalog"));
        assert!(is_system_schema("INFORMATION_SCHEMA"));
        assert!(is_system_schema("pg_temp_3"));
        assert!(is_system_schema("crdb_internal"));
        assert!(!is_system_schema("public"));
        assert!(!is_system_schema("billing"));
    }

    #[test]
    fn flavors_map_to_their_dialects() {
        assert_eq!(Flavor::Postgres.dialect(), SqlDialect::PostgreSql);
        assert_eq!(Flavor::Cockroach.dialect(), SqlDialect::CockroachDb);
    }
}
