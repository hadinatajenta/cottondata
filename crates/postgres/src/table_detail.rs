use crate::cockroach_detail;
use crate::errors::postgres_error;
use crate::introspection::Flavor;
use crate::rows::{flag, optional_text, rows, text};
use crate::values::referential_action;
use cottondata_core::SqlDialect;
use cottondata_driver::{DbError, ForeignKeyInfo, InboundForeignKeyInfo, IndexInfo, TableDetail};
use std::collections::{BTreeMap, HashSet};
use tokio_postgres::Client;

pub(crate) async fn load_table_detail(
    client: &Client,
    flavor: Flavor,
    schema: &str,
    table: &str,
) -> Result<TableDetail, DbError> {
    match flavor {
        Flavor::Postgres => load_postgres_table_detail(client, schema, table).await,
        Flavor::Cockroach => cockroach_detail::load_cockroach_table_detail(client, schema, table).await,
    }
}

async fn load_postgres_table_detail(client: &Client, schema: &str, table: &str) -> Result<TableDetail, DbError> {
    let object_rows = rows(
        client,
        "SELECT c.oid, c.relkind::text, \
                CASE WHEN c.relkind IN ('v', 'm') THEN pg_get_viewdef(c.oid, true) END, \
                CASE WHEN c.reltuples >= 0 THEN c.reltuples::bigint END, \
                pg_size_pretty(pg_total_relation_size(c.oid)), am.amname::text, \
                obj_description(c.oid, 'pg_class') \
         FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_am am ON am.oid = c.relam \
         WHERE n.nspname = $1 AND c.relname = $2",
        &[&schema, &table],
    )
    .await?;
    let Some(object_row) = object_rows.first() else {
        return Err(DbError::message(format!("Table {schema}.{table} was not found")));
    };
    let table_oid: u32 = object_row.try_get(0).map_err(postgres_error)?;
    let relkind = text(object_row, 1);
    let view_definition = optional_text(object_row, 2);
    let row_estimate = object_row
        .try_get::<_, Option<i64>>(3)
        .ok()
        .flatten()
        .and_then(|estimate| u64::try_from(estimate).ok());
    let size = optional_text(object_row, 4);
    let access_method = optional_text(object_row, 5);
    let table_comment = optional_text(object_row, 6);

    let index_rows = rows(
        client,
        "SELECT ic.relname::text, ix.indisunique, ix.indisprimary, am.amname::text, \
                pg_get_indexdef(ix.indexrelid, k.ordinal::int, true), pg_get_indexdef(ix.indexrelid) \
         FROM pg_index ix \
         JOIN pg_class ic ON ic.oid = ix.indexrelid \
         JOIN pg_am am ON am.oid = ic.relam \
         CROSS JOIN LATERAL generate_series(1, ix.indnatts) AS k(ordinal) \
         WHERE ix.indrelid = $1 \
         ORDER BY ix.indisprimary DESC, ic.relname, k.ordinal",
        &[&table_oid],
    )
    .await
    .unwrap_or_default();
    let mut indexes: BTreeMap<String, IndexInfo> = BTreeMap::new();
    let mut index_definitions: Vec<(String, bool, String)> = Vec::new();
    for row in &index_rows {
        let name = text(row, 0);
        let is_primary = flag(row, 2);
        let entry = indexes.entry(name.clone()).or_insert_with(|| {
            index_definitions.push((name.clone(), is_primary, text(row, 5)));
            IndexInfo {
                name: name.clone(),
                non_unique: !flag(row, 1),
                index_type: if is_primary {
                    "PRIMARY KEY".to_string()
                } else {
                    text(row, 3).to_ascii_uppercase()
                },
                columns: Vec::new(),
            }
        });
        entry.columns.push(text(row, 4));
    }

    let foreign_key_rows = rows(
        client,
        "SELECT con.conname::text, a.attname::text, fn.nspname::text, fc.relname::text, fa.attname::text, \
                con.confupdtype::text, con.confdeltype::text \
         FROM pg_constraint con \
         CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(local_attnum, foreign_attnum, ordinal) \
         JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.local_attnum \
         JOIN pg_class fc ON fc.oid = con.confrelid \
         JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
         JOIN pg_attribute fa ON fa.attrelid = con.confrelid AND fa.attnum = k.foreign_attnum \
         WHERE con.contype = 'f' AND con.conrelid = $1 \
         ORDER BY con.conname, k.ordinal",
        &[&table_oid],
    )
    .await
    .unwrap_or_default();
    let foreign_keys = foreign_key_rows
        .iter()
        .map(|row| ForeignKeyInfo {
            constraint_name: text(row, 0),
            column_name: text(row, 1),
            ref_schema: text(row, 2),
            ref_table: text(row, 3),
            ref_column: text(row, 4),
            update_rule: referential_action(&text(row, 5)).to_string(),
            delete_rule: referential_action(&text(row, 6)).to_string(),
        })
        .collect();

    let inbound_rows = rows(
        client,
        "SELECT con.conname::text, sn.nspname::text, sc.relname::text, a.attname::text, fa.attname::text, \
                con.confupdtype::text, con.confdeltype::text \
         FROM pg_constraint con \
         CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(local_attnum, foreign_attnum, ordinal) \
         JOIN pg_class sc ON sc.oid = con.conrelid \
         JOIN pg_namespace sn ON sn.oid = sc.relnamespace \
         JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.local_attnum \
         JOIN pg_attribute fa ON fa.attrelid = con.confrelid AND fa.attnum = k.foreign_attnum \
         WHERE con.contype = 'f' AND con.confrelid = $1 \
         ORDER BY sn.nspname, sc.relname, con.conname, k.ordinal",
        &[&table_oid],
    )
    .await
    .unwrap_or_default();
    let inbound_foreign_keys = inbound_rows
        .iter()
        .map(|row| InboundForeignKeyInfo {
            constraint_name: text(row, 0),
            source_schema: text(row, 1),
            source_table: text(row, 2),
            source_column: text(row, 3),
            target_column: text(row, 4),
            update_rule: referential_action(&text(row, 5)).to_string(),
            delete_rule: referential_action(&text(row, 6)).to_string(),
        })
        .collect();

    let dialect = SqlDialect::PostgreSql;
    let qualified = dialect.qualified_table(schema, table);
    let ddl = match view_definition {
        Some(definition) => {
            let keyword = if relkind == "m" { "MATERIALIZED VIEW" } else { "VIEW" };
            format!("CREATE {keyword} {qualified} AS\n{}", definition.trim_end())
        }
        None => {
            render_postgres_create_table(client, table_oid, &qualified, table_comment.as_deref(), &index_definitions)
                .await?
        }
    };
    let engine = match (access_method, size) {
        (Some(method), Some(size)) => Some(format!("{method} · {size}")),
        (method, size) => method.or(size),
    };

    Ok(TableDetail {
        ddl,
        engine,
        collation: None,
        row_estimate,
        indexes: indexes.into_values().collect(),
        foreign_keys,
        inbound_foreign_keys,
    })
}

async fn render_postgres_create_table(
    client: &Client,
    table_oid: u32,
    qualified: &str,
    table_comment: Option<&str>,
    index_definitions: &[(String, bool, String)],
) -> Result<String, DbError> {
    let dialect = SqlDialect::PostgreSql;
    let column_rows = rows(
        client,
        "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), a.attnotnull, \
                pg_get_expr(d.adbin, d.adrelid), to_jsonb(a) ->> 'attidentity', to_jsonb(a) ->> 'attgenerated', \
                col_description(a.attrelid, a.attnum) \
         FROM pg_attribute a \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY a.attnum",
        &[&table_oid],
    )
    .await?;
    let constraint_rows = rows(
        client,
        "SELECT conname::text, pg_get_constraintdef(oid, true), contype::text FROM pg_constraint \
         WHERE conrelid = $1 ORDER BY CASE contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'c' THEN 2 ELSE 3 END, conname",
        &[&table_oid],
    )
    .await
    .unwrap_or_default();

    let mut lines: Vec<String> = column_rows
        .iter()
        .map(|row| {
            let mut line = format!("    {} {}", dialect.quote_ident(&text(row, 0)), text(row, 1));
            let identity = text(row, 4);
            let generated = text(row, 5);
            match (identity.as_str(), generated.as_str()) {
                ("a", _) => line.push_str(" GENERATED ALWAYS AS IDENTITY"),
                ("d", _) => line.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
                (_, "s") => {
                    if let Some(expression) = optional_text(row, 3) {
                        line.push_str(&format!(" GENERATED ALWAYS AS ({expression}) STORED"));
                    }
                }
                _ => {
                    if let Some(default_expr) = optional_text(row, 3) {
                        line.push_str(&format!(" DEFAULT {default_expr}"));
                    }
                }
            }
            if flag(row, 2) {
                line.push_str(" NOT NULL");
            }
            line
        })
        .collect();
    let mut constraint_index_names = HashSet::new();
    for row in &constraint_rows {
        let name = text(row, 0);
        if matches!(text(row, 2).as_str(), "p" | "u" | "x") {
            constraint_index_names.insert(name.clone());
        }
        lines.push(format!("    CONSTRAINT {} {}", dialect.quote_ident(&name), text(row, 1)));
    }

    let mut ddl = format!("CREATE TABLE {qualified} (\n{}\n);", lines.join(",\n"));
    for (name, is_primary, definition) in index_definitions {
        if *is_primary || constraint_index_names.contains(name) {
            continue;
        }
        ddl.push_str(&format!("\n{definition};"));
    }
    if let Some(comment) = table_comment {
        ddl.push_str(&format!("\nCOMMENT ON TABLE {qualified} IS {};", dialect.string_literal(comment)));
    }
    for row in &column_rows {
        if let Some(comment) = optional_text(row, 6) {
            ddl.push_str(&format!(
                "\nCOMMENT ON COLUMN {qualified}.{} IS {};",
                dialect.quote_ident(&text(row, 0)),
                dialect.string_literal(&comment)
            ));
        }
    }
    Ok(ddl)
}
