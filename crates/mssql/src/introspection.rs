use crate::ddl::render_create_table;
use crate::errors::mssql_error;
use crate::rows::{flag, int, optional_text, text, MssqlClient};
use crate::values::format_declared_type;
use cottondata_core::SqlDialect;
use cottondata_driver::{
    ColumnSummary, DbError, ForeignKeyInfo, InboundForeignKeyInfo, IndexInfo, RoutineSummary, SchemaObjects,
    SchemaSummary, TableDetail, TableKind, TableSummary,
};
use std::collections::BTreeMap;
use tiberius::{Row, ToSql};

const SYSTEM_DATABASES: [&str; 4] = ["master", "model", "msdb", "tempdb"];

pub fn is_system_database(name: &str) -> bool {
    SYSTEM_DATABASES.iter().any(|system| system.eq_ignore_ascii_case(name))
}

async fn fetch_rows(client: &mut MssqlClient, sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Row>, DbError> {
    client
        .query(sql, params)
        .await
        .map_err(mssql_error)?
        .into_first_result()
        .await
        .map_err(mssql_error)
}

fn database_ref(database: &str) -> String {
    SqlDialect::SqlServer.quote_ident(database)
}

pub(crate) async fn list_schemas(client: &mut MssqlClient) -> Result<Vec<SchemaSummary>, DbError> {
    let rows = fetch_rows(
        client,
        "SELECT name, collation_name FROM sys.databases WHERE state = 0 AND HAS_DBACCESS(name) = 1 ORDER BY name",
        &[],
    )
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let name = text(row, 0);
            SchemaSummary {
                is_system: is_system_database(&name),
                default_collation: optional_text(row, 1),
                name,
            }
        })
        .collect())
}

pub(crate) async fn load_schema_objects(client: &mut MssqlClient, database: &str) -> Result<SchemaObjects, DbError> {
    let db = database_ref(database);
    let object_rows = fetch_rows(
        client,
        &format!(
            "SELECT s.name, o.name, o.type FROM {db}.sys.objects o \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name"
        ),
        &[],
    )
    .await?;
    let column_rows = fetch_rows(
        client,
        &format!(
            "SELECT s.name, o.name, c.name, ty.name, c.max_length, c.precision, c.scale, \
                    c.is_nullable, c.is_identity, \
                    CAST(CASE WHEN pk.column_id IS NULL THEN 0 ELSE 1 END AS bit), dc.definition \
             FROM {db}.sys.columns c \
             JOIN {db}.sys.objects o ON o.object_id = c.object_id AND o.type IN ('U', 'V') AND o.is_ms_shipped = 0 \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             JOIN {db}.sys.types ty ON ty.user_type_id = c.user_type_id \
             LEFT JOIN {db}.sys.default_constraints dc ON dc.object_id = c.default_object_id \
             LEFT JOIN (SELECT ic.object_id, ic.column_id FROM {db}.sys.indexes i \
                        JOIN {db}.sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
                        WHERE i.is_primary_key = 1) pk \
               ON pk.object_id = c.object_id AND pk.column_id = c.column_id \
             ORDER BY s.name, o.name, c.column_id"
        ),
        &[],
    )
    .await?;
    let routine_rows = fetch_rows(
        client,
        &format!(
            "SELECT s.name + '.' + o.name, o.type_desc FROM {db}.sys.objects o \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.type IN ('P', 'FN', 'IF', 'TF') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name"
        ),
        &[],
    )
    .await
    .unwrap_or_default();

    let mut columns_by_table: BTreeMap<String, Vec<ColumnSummary>> = BTreeMap::new();
    for row in &column_rows {
        let data_type = text(row, 3);
        let column_type = format_declared_type(&data_type, int(row, 4) as i16, int(row, 5) as u8, int(row, 6) as u8);
        columns_by_table
            .entry(format!("{}.{}", text(row, 0), text(row, 1)))
            .or_default()
            .push(ColumnSummary {
                name: text(row, 2),
                data_type,
                column_type,
                is_nullable: flag(row, 7),
                is_auto_increment: flag(row, 8),
                is_primary_key: flag(row, 9),
                default_expr: optional_text(row, 10),
                comment: None,
            });
    }

    let tables = object_rows
        .iter()
        .map(|row| {
            let name = format!("{}.{}", text(row, 0), text(row, 1));
            TableSummary {
                columns: columns_by_table.remove(&name).unwrap_or_default(),
                kind: if text(row, 2).trim() == "V" {
                    TableKind::View
                } else {
                    TableKind::BaseTable
                },
                name,
            }
        })
        .collect();
    let routines = routine_rows
        .iter()
        .map(|row| RoutineSummary {
            name: text(row, 0),
            kind: text(row, 1),
        })
        .collect();
    Ok(SchemaObjects { tables, routines })
}

pub(crate) async fn load_table_detail(
    client: &mut MssqlClient,
    database: &str,
    table: &str,
) -> Result<TableDetail, DbError> {
    let db = database_ref(database);
    let (owner, object) = table.split_once('.').unwrap_or(("dbo", table));
    let object_rows = fetch_rows(
        client,
        &format!(
            "SELECT o.object_id, o.type, m.definition, \
                    (SELECT SUM(p.rows) FROM {db}.sys.partitions p WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)) \
             FROM {db}.sys.objects o JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             LEFT JOIN {db}.sys.sql_modules m ON m.object_id = o.object_id \
             WHERE s.name = @P1 AND o.name = @P2"
        ),
        &[&owner, &object],
    )
    .await?;
    let Some(object_row) = object_rows.first() else {
        return Err(DbError::message(format!("Table {database}.{table} was not found")));
    };
    let object_id = int(object_row, 0) as i32;
    let view_definition = optional_text(object_row, 2);
    let row_estimate = u64::try_from(int(object_row, 3)).ok();

    let index_rows = fetch_rows(
        client,
        &format!(
            "SELECT i.name, i.is_unique, i.type_desc, c.name, i.is_primary_key FROM {db}.sys.indexes i \
             JOIN {db}.sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
             JOIN {db}.sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             WHERE i.object_id = @P1 AND i.name IS NOT NULL AND ic.is_included_column = 0 \
             ORDER BY i.name, ic.key_ordinal"
        ),
        &[&object_id],
    )
    .await
    .unwrap_or_default();
    let mut indexes: BTreeMap<String, IndexInfo> = BTreeMap::new();
    let mut primary_key_columns = Vec::new();
    for row in &index_rows {
        let column = text(row, 3);
        if flag(row, 4) {
            primary_key_columns.push(column.clone());
        }
        indexes
            .entry(text(row, 0))
            .or_insert_with(|| IndexInfo {
                name: text(row, 0),
                non_unique: !flag(row, 1),
                index_type: text(row, 2),
                columns: Vec::new(),
            })
            .columns
            .push(column);
    }

    let foreign_key_rows = fetch_rows(
        client,
        &format!(
            "SELECT fk.name, pc.name, rs.name, rt.name, rc.name, \
                    fk.update_referential_action_desc, fk.delete_referential_action_desc \
             FROM {db}.sys.foreign_keys fk \
             JOIN {db}.sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
             JOIN {db}.sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
             JOIN {db}.sys.objects rt ON rt.object_id = fkc.referenced_object_id \
             JOIN {db}.sys.schemas rs ON rs.schema_id = rt.schema_id \
             JOIN {db}.sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
             WHERE fk.parent_object_id = @P1 ORDER BY fk.name, fkc.constraint_column_id"
        ),
        &[&object_id],
    )
    .await
    .unwrap_or_default();
    let foreign_keys = foreign_key_rows
        .iter()
        .map(|row| ForeignKeyInfo {
            constraint_name: text(row, 0),
            column_name: text(row, 1),
            ref_schema: database.to_string(),
            ref_table: format!("{}.{}", text(row, 2), text(row, 3)),
            ref_column: text(row, 4),
            update_rule: text(row, 5),
            delete_rule: text(row, 6),
        })
        .collect();

    let inbound_foreign_key_rows = fetch_rows(
        client,
        &format!(
            "SELECT fk.name, ps.name, pt.name, pc.name, rc.name, \
                    fk.update_referential_action_desc, fk.delete_referential_action_desc \
             FROM {db}.sys.foreign_keys fk \
             JOIN {db}.sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
             JOIN {db}.sys.objects pt ON pt.object_id = fkc.parent_object_id \
             JOIN {db}.sys.schemas ps ON ps.schema_id = pt.schema_id \
             JOIN {db}.sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
             JOIN {db}.sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
             WHERE fk.referenced_object_id = @P1 ORDER BY ps.name, pt.name, fk.name, fkc.constraint_column_id"
        ),
        &[&object_id],
    )
    .await
    .unwrap_or_default();
    let inbound_foreign_keys = inbound_foreign_key_rows
        .iter()
        .map(|row| InboundForeignKeyInfo {
            constraint_name: text(row, 0),
            source_schema: database.to_string(),
            source_table: format!("{}.{}", text(row, 1), text(row, 2)),
            source_column: text(row, 3),
            target_column: text(row, 4),
            update_rule: text(row, 5),
            delete_rule: text(row, 6),
        })
        .collect();

    let ddl = match view_definition {
        Some(definition) => definition,
        None => {
            let objects = load_schema_objects(client, database).await?;
            let columns = objects
                .tables
                .into_iter()
                .find(|candidate| candidate.name.eq_ignore_ascii_case(table))
                .map(|found| found.columns)
                .unwrap_or_default();
            render_create_table(owner, object, &columns, &primary_key_columns)
        }
    };

    Ok(TableDetail {
        ddl,
        engine: None,
        collation: None,
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
    fn system_databases_are_detected_case_insensitively() {
        assert!(is_system_database("MASTER"));
        assert!(is_system_database("tempdb"));
        assert!(!is_system_database("shop"));
    }
}
