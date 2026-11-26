use crate::errors::sqlite_error;
use cottondata_core::SqlDialect;
use cottondata_driver::{
    ColumnSummary, DbError, ForeignKeyInfo, InboundForeignKeyInfo, IndexInfo, RoutineSummary, SchemaObjects,
    SchemaSummary, TableDetail, TableKind, TableSummary,
};
use rusqlite::{Connection, Row};

const DIALECT: SqlDialect = SqlDialect::Sqlite;

fn quoted_schema(schema: &str) -> String {
    DIALECT.quote_ident(if schema.trim().is_empty() { "main" } else { schema })
}

fn string_at(row: &Row<'_>, index: usize) -> String {
    row.get::<_, Option<String>>(index).ok().flatten().unwrap_or_default()
}

pub(crate) fn list_schemas(connection: &Connection) -> Result<Vec<SchemaSummary>, DbError> {
    let mut statement = connection.prepare("PRAGMA database_list").map_err(sqlite_error)?;
    let schemas = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite_error)?
        .filter_map(Result::ok)
        .map(|name| SchemaSummary {
            is_system: name.eq_ignore_ascii_case("temp"),
            default_collation: None,
            name,
        })
        .collect();
    Ok(schemas)
}

pub(crate) fn load_schema_objects(connection: &Connection, schema: &str) -> Result<SchemaObjects, DbError> {
    let schema_ref = quoted_schema(schema);
    let mut statement = connection
        .prepare(&format!(
            "SELECT name, type FROM {schema_ref}.sqlite_master \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name"
        ))
        .map_err(sqlite_error)?;
    let objects: Vec<(String, String)> = statement
        .query_map([], |row| Ok((string_at(row, 0), string_at(row, 1))))
        .map_err(sqlite_error)?
        .filter_map(Result::ok)
        .collect();
    let mut tables = Vec::with_capacity(objects.len());
    for (name, kind) in objects {
        tables.push(TableSummary {
            columns: load_columns(connection, schema, &name)?,
            kind: if kind == "view" {
                TableKind::View
            } else {
                TableKind::BaseTable
            },
            name,
        });
    }
    let mut trigger_statement = connection
        .prepare(&format!(
            "SELECT name FROM {schema_ref}.sqlite_master WHERE type = 'trigger' ORDER BY name"
        ))
        .map_err(sqlite_error)?;
    let routines = trigger_statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sqlite_error)?
        .filter_map(Result::ok)
        .map(|name| RoutineSummary {
            name,
            kind: "TRIGGER".into(),
        })
        .collect();
    Ok(SchemaObjects { tables, routines })
}

fn load_columns(connection: &Connection, schema: &str, table: &str) -> Result<Vec<ColumnSummary>, DbError> {
    let mut statement = connection
        .prepare("SELECT name, type, \"notnull\", dflt_value, pk, hidden FROM pragma_table_xinfo(?1, ?2) ORDER BY cid")
        .map_err(sqlite_error)?;
    let raw: Vec<(String, String, bool, Option<String>, i64, i64)> = statement
        .query_map([table, if schema.is_empty() { "main" } else { schema }], |row| {
            Ok((
                string_at(row, 0),
                string_at(row, 1),
                row.get::<_, i64>(2).unwrap_or(0) != 0,
                row.get::<_, Option<String>>(3).ok().flatten(),
                row.get::<_, i64>(4).unwrap_or(0),
                row.get::<_, i64>(5).unwrap_or(0),
            ))
        })
        .map_err(sqlite_error)?
        .filter_map(Result::ok)
        .collect();
    let primary_key_count = raw.iter().filter(|column| column.4 > 0).count();
    Ok(raw
        .into_iter()
        .filter(|column| column.5 != 1)
        .map(|(name, declared_type, is_not_null, default_expr, pk_position, _)| {
            let is_rowid_alias = pk_position > 0 && primary_key_count == 1 && declared_type.eq_ignore_ascii_case("INTEGER");
            ColumnSummary {
                name,
                data_type: declared_type.to_ascii_lowercase(),
                column_type: declared_type.to_ascii_lowercase(),
                is_primary_key: pk_position > 0,
                is_nullable: !is_not_null && pk_position == 0,
                is_auto_increment: is_rowid_alias,
                default_expr,
                comment: None,
            }
        })
        .collect())
}

pub(crate) fn primary_key_columns(connection: &Connection, schema: &str, table: &str) -> Vec<String> {
    let Ok(mut statement) =
        connection.prepare("SELECT name, pk FROM pragma_table_info(?1, ?2) WHERE pk > 0 ORDER BY pk")
    else {
        return Vec::new();
    };
    statement
        .query_map([table, if schema.is_empty() { "main" } else { schema }], |row| row.get::<_, String>(0))
        .map(|names| names.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub(crate) fn load_table_detail(connection: &Connection, schema: &str, table: &str) -> Result<TableDetail, DbError> {
    let schema_name = if schema.is_empty() { "main" } else { schema };
    let schema_ref = quoted_schema(schema);
    let mut definition_statement = connection
        .prepare(&format!(
            "SELECT type, sql FROM {schema_ref}.sqlite_master WHERE tbl_name = ?1 AND sql IS NOT NULL \
             ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'view' THEN 0 WHEN 'index' THEN 1 ELSE 2 END, name"
        ))
        .map_err(sqlite_error)?;
    let definitions: Vec<(String, String)> = definition_statement
        .query_map([table], |row| Ok((string_at(row, 0), string_at(row, 1))))
        .map_err(sqlite_error)?
        .filter_map(Result::ok)
        .collect();
    if !definitions.iter().any(|(kind, _)| kind == "table" || kind == "view") {
        return Err(DbError::message(format!("Table {schema_name}.{table} was not found")));
    }
    let ddl = definitions
        .iter()
        .map(|(_, sql)| if sql.trim_end().ends_with(';') { sql.clone() } else { format!("{sql};") })
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut indexes = Vec::new();
    let primary_keys = primary_key_columns(connection, schema_name, table);
    if !primary_keys.is_empty() {
        indexes.push(IndexInfo {
            name: "PRIMARY".into(),
            non_unique: false,
            index_type: "PRIMARY KEY".into(),
            columns: primary_keys,
        });
    }
    let mut index_statement = connection
        .prepare("SELECT name, \"unique\", origin FROM pragma_index_list(?1, ?2)")
        .map_err(sqlite_error)?;
    let index_rows: Vec<(String, bool, String)> = index_statement
        .query_map([table, schema_name], |row| {
            Ok((string_at(row, 0), row.get::<_, i64>(1).unwrap_or(0) != 0, string_at(row, 2)))
        })
        .map_err(sqlite_error)?
        .filter_map(Result::ok)
        .collect();
    for (name, is_unique, origin) in index_rows {
        if origin == "pk" {
            continue;
        }
        let mut column_statement = connection
            .prepare("SELECT name FROM pragma_index_info(?1, ?2) ORDER BY seqno")
            .map_err(sqlite_error)?;
        let columns = column_statement
            .query_map([name.as_str(), schema_name], |row| row.get::<_, Option<String>>(0))
            .map_err(sqlite_error)?
            .filter_map(Result::ok)
            .map(|column| column.unwrap_or_else(|| "<expression>".into()))
            .collect();
        indexes.push(IndexInfo {
            name,
            non_unique: !is_unique,
            index_type: if origin == "u" { "UNIQUE CONSTRAINT".into() } else { "BTREE".into() },
            columns,
        });
    }

    let foreign_keys = load_foreign_keys(connection, schema_name, table)?
        .into_iter()
        .map(|(id, referenced_table, from, to, on_update, on_delete)| ForeignKeyInfo {
            constraint_name: foreign_key_name(table, id),
            column_name: from,
            ref_schema: schema_name.to_string(),
            ref_table: referenced_table,
            ref_column: to,
            update_rule: on_update,
            delete_rule: on_delete,
        })
        .collect();

    let mut inbound_foreign_keys = Vec::new();
    for other_table in table_names(connection, schema_name)? {
        for (id, referenced_table, from, to, on_update, on_delete) in load_foreign_keys(connection, schema_name, &other_table)? {
            if !referenced_table.eq_ignore_ascii_case(table) {
                continue;
            }
            inbound_foreign_keys.push(InboundForeignKeyInfo {
                constraint_name: foreign_key_name(&other_table, id),
                source_schema: schema_name.to_string(),
                source_table: other_table.clone(),
                source_column: from,
                target_column: to,
                update_rule: on_update,
                delete_rule: on_delete,
            });
        }
    }

    Ok(TableDetail {
        ddl,
        engine: Some("SQLite".into()),
        collation: None,
        row_estimate: None,
        indexes,
        foreign_keys,
        inbound_foreign_keys,
    })
}

pub(crate) fn foreign_key_name(table: &str, id: i64) -> String {
    format!("fk_{table}_{id}")
}

fn table_names(connection: &Connection, schema: &str) -> Result<Vec<String>, DbError> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT name FROM {}.sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            quoted_schema(schema)
        ))
        .map_err(sqlite_error)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sqlite_error)?
        .filter_map(Result::ok)
        .collect();
    Ok(names)
}

#[allow(clippy::type_complexity)]
fn load_foreign_keys(
    connection: &Connection,
    schema: &str,
    table: &str,
) -> Result<Vec<(i64, String, String, String, String, String)>, DbError> {
    let mut statement = connection
        .prepare(
            "SELECT id, \"table\", \"from\", \"to\", on_update, on_delete FROM pragma_foreign_key_list(?1, ?2) \
             ORDER BY id, seq",
        )
        .map_err(sqlite_error)?;
    let foreign_keys = statement
        .query_map([table, schema], |row| {
            Ok((
                row.get::<_, i64>(0).unwrap_or(0),
                string_at(row, 1),
                string_at(row, 2),
                string_at(row, 3),
                string_at(row, 4),
                string_at(row, 5),
            ))
        })
        .map_err(sqlite_error)?
        .filter_map(Result::ok)
        .collect();
    Ok(foreign_keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_database() -> Connection {
        let connection = Connection::open_in_memory().expect("memory db");
        connection
            .execute_batch(
                "CREATE TABLE customers (id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, note TEXT DEFAULT 'x');
                 CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id) ON DELETE CASCADE, total REAL);
                 CREATE INDEX ix_orders_customer ON orders(customer_id);
                 CREATE VIEW big_orders AS SELECT * FROM orders WHERE total > 100;
                 CREATE TRIGGER trg_orders AFTER INSERT ON orders BEGIN SELECT 1; END;",
            )
            .expect("schema");
        connection
    }

    #[test]
    fn lists_main_schema_and_objects_with_columns_views_and_triggers() {
        let connection = sample_database();
        let schemas = list_schemas(&connection).expect("schemas");
        assert!(schemas.iter().any(|schema| schema.name == "main" && !schema.is_system));
        let objects = load_schema_objects(&connection, "main").expect("objects");
        let names: Vec<&str> = objects.tables.iter().map(|table| table.name.as_str()).collect();
        assert_eq!(names, vec!["big_orders", "customers", "orders"]);
        let customers = &objects.tables[1];
        let id = &customers.columns[0];
        assert!(id.is_primary_key && id.is_auto_increment && !id.is_nullable);
        assert_eq!(customers.columns[2].default_expr.as_deref(), Some("'x'"));
        assert_eq!(objects.tables[0].kind, TableKind::View);
        assert_eq!(objects.routines[0].name, "trg_orders");
    }

    #[test]
    fn table_detail_includes_ddl_indexes_and_foreign_keys_both_ways() {
        let connection = sample_database();
        let orders = load_table_detail(&connection, "main", "orders").expect("orders");
        assert!(orders.ddl.starts_with("CREATE TABLE orders"));
        assert!(orders.ddl.contains("CREATE INDEX ix_orders_customer"));
        assert!(orders.indexes.iter().any(|index| index.name == "PRIMARY"));
        assert!(orders.indexes.iter().any(|index| index.columns == vec!["customer_id".to_string()]));
        assert_eq!(orders.foreign_keys[0].ref_table, "customers");
        assert_eq!(orders.foreign_keys[0].delete_rule, "CASCADE");
        let customers = load_table_detail(&connection, "main", "customers").expect("customers");
        assert_eq!(customers.inbound_foreign_keys[0].source_table, "orders");
        assert!(customers.indexes.iter().any(|index| !index.non_unique && index.columns == vec!["email".to_string()]));
        assert!(load_table_detail(&connection, "main", "missing").is_err());
    }
}
