use std::collections::HashMap;

use cottondata_core::SqlDialect;
use cottondata_driver::{ForeignKeyInfo, IndexInfo, TableDetail, TableKind};

use crate::SchemaCatalog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSnapshot {
    pub name: String,
    pub column_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub is_auto_increment: bool,
    pub default_expr: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSnapshot {
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<ColumnSnapshot>,
    pub detail: Option<TableDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDiff {
    pub table_name: String,
    pub added_columns: Vec<ColumnSnapshot>,
    pub removed_columns: Vec<ColumnSnapshot>,
    pub altered_columns: Vec<(ColumnSnapshot, ColumnSnapshot)>,
    pub added_indexes: Vec<IndexInfo>,
    pub removed_indexes: Vec<IndexInfo>,
    pub added_fks: Vec<ForeignKeyInfo>,
    pub removed_fks: Vec<ForeignKeyInfo>,
}

impl TableDiff {
    pub fn is_empty(&self) -> bool {
        self.change_count() == 0
    }

    pub fn change_count(&self) -> usize {
        self.added_columns.len()
            + self.removed_columns.len()
            + self.altered_columns.len()
            + self.added_indexes.len()
            + self.removed_indexes.len()
            + self.added_fks.len()
            + self.removed_fks.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDiff {
    pub source_label: String,
    pub target_label: String,
    pub only_in_source: Vec<TableSnapshot>,
    pub only_in_target: Vec<TableSnapshot>,
    pub modified_tables: Vec<TableDiff>,
    pub identical_count: usize,
}

impl SchemaDiff {
    pub fn has_differences(&self) -> bool {
        !self.only_in_source.is_empty()
            || !self.only_in_target.is_empty()
            || !self.modified_tables.is_empty()
    }
}

pub fn snapshot_from_catalog(catalog: &SchemaCatalog, schema_name: &str) -> Vec<TableSnapshot> {
    let Some(schema) = catalog
        .schemas()
        .iter()
        .find(|s| catalog.resolve(s.name).eq_ignore_ascii_case(schema_name))
    else {
        return Vec::new();
    };
    schema
        .tables
        .iter()
        .map(|t| TableSnapshot {
            name: catalog.resolve(t.name).to_string(),
            kind: t.kind,
            columns: t
                .columns
                .iter()
                .map(|c| ColumnSnapshot {
                    name: catalog.resolve(c.name).to_string(),
                    column_type: catalog.resolve(c.column_type).to_string(),
                    is_nullable: c.is_nullable,
                    is_primary_key: c.is_primary_key,
                    is_auto_increment: c.is_auto_increment,
                    default_expr: c.default_expr.clone(),
                    comment: c.comment.clone(),
                })
                .collect(),
            detail: t.detail.clone(),
        })
        .collect()
}

pub fn compare_schema_snapshots(
    source_label: String,
    target_label: String,
    source_tables: &[TableSnapshot],
    target_tables: &[TableSnapshot],
) -> SchemaDiff {
    let source_map: HashMap<String, &TableSnapshot> = source_tables
        .iter()
        .map(|t| (t.name.to_ascii_lowercase(), t))
        .collect();
    let target_map: HashMap<String, &TableSnapshot> = target_tables
        .iter()
        .map(|t| (t.name.to_ascii_lowercase(), t))
        .collect();

    let mut only_in_source = Vec::new();
    let mut only_in_target = Vec::new();
    let mut modified_tables = Vec::new();
    let mut identical_count = 0usize;

    for src_table in source_tables {
        let key = src_table.name.to_ascii_lowercase();
        match target_map.get(&key) {
            None => only_in_source.push(src_table.clone()),
            Some(dst_table) => {
                let table_diff = compare_table_snapshots(src_table, dst_table);
                if table_diff.is_empty() {
                    identical_count += 1;
                } else {
                    modified_tables.push(table_diff);
                }
            }
        }
    }

    for dst_table in target_tables {
        let key = dst_table.name.to_ascii_lowercase();
        if !source_map.contains_key(&key) {
            only_in_target.push(dst_table.clone());
        }
    }

    only_in_source.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    only_in_target.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    modified_tables.sort_by(|a, b| {
        a.table_name
            .to_ascii_lowercase()
            .cmp(&b.table_name.to_ascii_lowercase())
            .then_with(|| a.table_name.cmp(&b.table_name))
    });

    SchemaDiff {
        source_label,
        target_label,
        only_in_source,
        only_in_target,
        modified_tables,
        identical_count,
    }
}

fn compare_table_snapshots(src: &TableSnapshot, dst: &TableSnapshot) -> TableDiff {
    let src_cols: HashMap<String, &ColumnSnapshot> = src
        .columns
        .iter()
        .map(|c| (c.name.to_ascii_lowercase(), c))
        .collect();
    let dst_cols: HashMap<String, &ColumnSnapshot> = dst
        .columns
        .iter()
        .map(|c| (c.name.to_ascii_lowercase(), c))
        .collect();

    let mut added_columns = Vec::new();
    let mut removed_columns = Vec::new();
    let mut altered_columns = Vec::new();

    for src_col in &src.columns {
        let key = src_col.name.to_ascii_lowercase();
        match dst_cols.get(&key) {
            None => added_columns.push(src_col.clone()),
            Some(dst_col) => {
                if !columns_equivalent(src_col, dst_col) {
                    altered_columns.push((src_col.clone(), (*dst_col).clone()));
                }
            }
        }
    }

    for dst_col in &dst.columns {
        let key = dst_col.name.to_ascii_lowercase();
        if !src_cols.contains_key(&key) {
            removed_columns.push(dst_col.clone());
        }
    }

    let mut added_indexes = Vec::new();
    let mut removed_indexes = Vec::new();
    let mut added_fks = Vec::new();
    let mut removed_fks = Vec::new();

    if let (Some(src_detail), Some(dst_detail)) = (&src.detail, &dst.detail) {
        compare_indexes(
            &src_detail.indexes,
            &dst_detail.indexes,
            &mut added_indexes,
            &mut removed_indexes,
        );
        compare_foreign_keys(
            &src_detail.foreign_keys,
            &dst_detail.foreign_keys,
            &mut added_fks,
            &mut removed_fks,
        );
    }

    TableDiff {
        table_name: src.name.clone(),
        added_columns,
        removed_columns,
        altered_columns,
        added_indexes,
        removed_indexes,
        added_fks,
        removed_fks,
    }
}

fn compare_indexes(
    src_indexes: &[IndexInfo],
    dst_indexes: &[IndexInfo],
    added: &mut Vec<IndexInfo>,
    removed: &mut Vec<IndexInfo>,
) {
    let src_map: HashMap<String, &IndexInfo> = src_indexes
        .iter()
        .map(|idx| (idx.name.to_ascii_lowercase(), idx))
        .collect();
    let dst_map: HashMap<String, &IndexInfo> = dst_indexes
        .iter()
        .map(|idx| (idx.name.to_ascii_lowercase(), idx))
        .collect();

    for src_idx in src_indexes {
        let key = src_idx.name.to_ascii_lowercase();
        match dst_map.get(&key) {
            None => added.push(src_idx.clone()),
            Some(dst_idx) => {
                if !indexes_equivalent(src_idx, dst_idx) {
                    removed.push((*dst_idx).clone());
                    added.push(src_idx.clone());
                }
            }
        }
    }

    for dst_idx in dst_indexes {
        let key = dst_idx.name.to_ascii_lowercase();
        if !src_map.contains_key(&key) {
            removed.push(dst_idx.clone());
        }
    }
}

fn compare_foreign_keys(
    src_fks: &[ForeignKeyInfo],
    dst_fks: &[ForeignKeyInfo],
    added: &mut Vec<ForeignKeyInfo>,
    removed: &mut Vec<ForeignKeyInfo>,
) {
    let src_map: HashMap<(String, String), &ForeignKeyInfo> = src_fks
        .iter()
        .map(|fk| {
            (
                (
                    fk.constraint_name.to_ascii_lowercase(),
                    fk.column_name.to_ascii_lowercase(),
                ),
                fk,
            )
        })
        .collect();
    let dst_map: HashMap<(String, String), &ForeignKeyInfo> = dst_fks
        .iter()
        .map(|fk| {
            (
                (
                    fk.constraint_name.to_ascii_lowercase(),
                    fk.column_name.to_ascii_lowercase(),
                ),
                fk,
            )
        })
        .collect();

    for src_fk in src_fks {
        let key = (
            src_fk.constraint_name.to_ascii_lowercase(),
            src_fk.column_name.to_ascii_lowercase(),
        );
        match dst_map.get(&key) {
            None => added.push(src_fk.clone()),
            Some(dst_fk) => {
                if !fks_equivalent(src_fk, dst_fk) {
                    removed.push((*dst_fk).clone());
                    added.push(src_fk.clone());
                }
            }
        }
    }

    for dst_fk in dst_fks {
        let key = (
            dst_fk.constraint_name.to_ascii_lowercase(),
            dst_fk.column_name.to_ascii_lowercase(),
        );
        if !src_map.contains_key(&key) {
            removed.push(dst_fk.clone());
        }
    }
}

fn normalize_type(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn normalize_default(default_expr: Option<&str>) -> Option<String> {
    default_expr
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
}

fn columns_equivalent(a: &ColumnSnapshot, b: &ColumnSnapshot) -> bool {
    normalize_type(&a.column_type) == normalize_type(&b.column_type)
        && a.is_nullable == b.is_nullable
        && a.is_auto_increment == b.is_auto_increment
        && normalize_default(a.default_expr.as_deref())
            == normalize_default(b.default_expr.as_deref())
}

fn indexes_equivalent(a: &IndexInfo, b: &IndexInfo) -> bool {
    a.non_unique == b.non_unique
        && a.index_type.trim().eq_ignore_ascii_case(b.index_type.trim())
        && a.columns.len() == b.columns.len()
        && a.columns
            .iter()
            .zip(&b.columns)
            .all(|(ca, cb)| ca.eq_ignore_ascii_case(cb))
}

fn fks_equivalent(a: &ForeignKeyInfo, b: &ForeignKeyInfo) -> bool {
    a.constraint_name.eq_ignore_ascii_case(&b.constraint_name)
        && a.column_name.eq_ignore_ascii_case(&b.column_name)
        && a.ref_schema.eq_ignore_ascii_case(&b.ref_schema)
        && a.ref_table.eq_ignore_ascii_case(&b.ref_table)
        && a.ref_column.eq_ignore_ascii_case(&b.ref_column)
        && a.update_rule
            .trim()
            .eq_ignore_ascii_case(b.update_rule.trim())
        && a.delete_rule
            .trim()
            .eq_ignore_ascii_case(b.delete_rule.trim())
}

pub fn generate_migration_sql(
    diff: &SchemaDiff,
    target_schema: &str,
    dialect: SqlDialect,
    include_drops: bool,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "-- Migration: {} -> {}",
        diff.source_label, diff.target_label
    ));

    let mut statements = Vec::new();

    for table in &diff.only_in_source {
        statements.push(format_create_table(table, target_schema, dialect));
    }

    for table_diff in &diff.modified_tables {
        let fq = qualify_table(dialect, target_schema, &table_diff.table_name);

        if include_drops {
            for fk in &table_diff.removed_fks {
                statements.push(format_drop_fk(&fq, fk, dialect));
            }
            for idx in &table_diff.removed_indexes {
                statements.push(format_drop_index(&fq, target_schema, idx, dialect));
            }
        }

        for col in &table_diff.added_columns {
            statements.push(format_add_column(&fq, col, dialect));
        }

        for (src_col, _) in &table_diff.altered_columns {
            statements.push(format_alter_column(&fq, src_col, dialect));
        }

        for idx in &table_diff.added_indexes {
            statements.push(format_add_index(&fq, target_schema, &table_diff.table_name, idx, dialect));
        }

        for fk in &table_diff.added_fks {
            statements.push(format_add_fk(&fq, target_schema, fk, dialect));
        }

        if include_drops {
            for col in &table_diff.removed_columns {
                statements.push(format!(
                    "ALTER TABLE {fq} DROP COLUMN {};",
                    dialect.quote_ident(&col.name)
                ));
            }
        }
    }

    if include_drops {
        for table in &diff.only_in_target {
            let fq = qualify_table(dialect, target_schema, &table.name);
            let keyword = match table.kind {
                TableKind::BaseTable => "TABLE",
                TableKind::View => "VIEW",
            };
            statements.push(format!("DROP {keyword} {fq};"));
        }
    }

    if statements.is_empty() {
        lines.push("-- No changes required.".to_string());
    } else {
        lines.push(String::new());
        lines.extend(statements);
    }

    lines.join("\n") + "\n"
}

fn qualify_table(dialect: SqlDialect, schema: &str, table: &str) -> String {
    if schema.is_empty() {
        dialect.quote_ident(table)
    } else {
        dialect.qualified_table(schema, table)
    }
}

fn format_create_table(table: &TableSnapshot, target_schema: &str, dialect: SqlDialect) -> String {
    if let Some(ddl) = table
        .detail
        .as_ref()
        .map(|d| d.ddl.trim())
        .filter(|s| !s.is_empty())
    {
        return if ddl.ends_with(';') {
            ddl.to_string()
        } else {
            format!("{ddl};")
        };
    }

    let fq = qualify_table(dialect, target_schema, &table.name);
    let mut defs: Vec<String> = table
        .columns
        .iter()
        .map(|c| format!("    {}", format_column_def(c, dialect, false)))
        .collect();

    let pk_cols: Vec<String> = table
        .columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| dialect.quote_ident(&c.name))
        .collect();
    if !pk_cols.is_empty() {
        defs.push(format!("    PRIMARY KEY ({})", pk_cols.join(", ")));
    }

    format!("CREATE TABLE {fq} (\n{}\n);", defs.join(",\n"))
}

fn format_column_def(col: &ColumnSnapshot, dialect: SqlDialect, for_alter: bool) -> String {
    let mut def = format!("{} {}", dialect.quote_ident(&col.name), col.column_type);
    if col.is_nullable {
        def.push_str(" NULL");
    } else {
        def.push_str(" NOT NULL");
    }

    if col.is_auto_increment && !for_alter {
        match dialect {
            SqlDialect::MySql => def.push_str(" AUTO_INCREMENT"),
            SqlDialect::SqlServer => def.push_str(" IDENTITY(1,1)"),
            SqlDialect::PostgreSql | SqlDialect::CockroachDb => def.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
            SqlDialect::Sqlite => {}
        }
    } else if col.is_auto_increment && dialect == SqlDialect::MySql {
        def.push_str(" AUTO_INCREMENT");
    }

    if let Some(default_val) = col
        .default_expr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let emit_default = match dialect {
            SqlDialect::MySql => true,
            SqlDialect::SqlServer | SqlDialect::PostgreSql | SqlDialect::CockroachDb | SqlDialect::Sqlite => {
                !for_alter
            }
        } && !(col.is_auto_increment && dialect.is_postgres_family());
        if emit_default {
            def.push_str(&format!(" DEFAULT {default_val}"));
        }
    }

    if matches!(dialect, SqlDialect::MySql) {
        if let Some(comment) = col.comment.as_deref().filter(|s| !s.is_empty()) {
            def.push_str(&format!(" COMMENT {}", dialect.string_literal(comment)));
        }
    }

    def
}

fn format_add_column(fq: &str, col: &ColumnSnapshot, dialect: SqlDialect) -> String {
    let def = format_column_def(col, dialect, false);
    match dialect {
        SqlDialect::SqlServer => format!("ALTER TABLE {fq} ADD {def};"),
        SqlDialect::MySql | SqlDialect::PostgreSql | SqlDialect::CockroachDb | SqlDialect::Sqlite => {
            format!("ALTER TABLE {fq} ADD COLUMN {def};")
        }
    }
}

fn format_alter_column(fq: &str, col: &ColumnSnapshot, dialect: SqlDialect) -> String {
    let q_col = dialect.quote_ident(&col.name);
    match dialect {
        SqlDialect::MySql => format!("ALTER TABLE {fq} MODIFY COLUMN {};", format_column_def(col, dialect, true)),
        SqlDialect::SqlServer => format!("ALTER TABLE {fq} ALTER COLUMN {};", format_column_def(col, dialect, true)),
        SqlDialect::PostgreSql | SqlDialect::CockroachDb => {
            let nullability = if col.is_nullable { "DROP NOT NULL" } else { "SET NOT NULL" };
            let default_action = match col.default_expr.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(default_expr) => format!("SET DEFAULT {default_expr}"),
                None => "DROP DEFAULT".to_string(),
            };
            format!(
                "ALTER TABLE {fq} ALTER COLUMN {q_col} TYPE {column_type}, ALTER COLUMN {q_col} {nullability}, ALTER COLUMN {q_col} {default_action};",
                column_type = col.column_type
            )
        }
        SqlDialect::Sqlite => format!(
            "-- SQLite cannot alter column {q_col} on {fq} in place; rebuild the table to apply: {}",
            format_column_def(col, dialect, false)
        ),
    }
}

fn format_add_index(fq: &str, target_schema: &str, table_name: &str, idx: &IndexInfo, dialect: SqlDialect) -> String {
    let cols: Vec<String> = idx.columns.iter().map(|c| dialect.quote_ident(c)).collect();
    let cols_sql = cols.join(", ");
    match dialect {
        SqlDialect::MySql => {
            if idx.name.eq_ignore_ascii_case("PRIMARY") {
                format!("ALTER TABLE {fq} ADD PRIMARY KEY ({cols_sql});")
            } else if idx.non_unique {
                format!(
                    "ALTER TABLE {fq} ADD INDEX {} ({cols_sql});",
                    dialect.quote_ident(&idx.name)
                )
            } else {
                format!(
                    "ALTER TABLE {fq} ADD UNIQUE INDEX {} ({cols_sql});",
                    dialect.quote_ident(&idx.name)
                )
            }
        }
        SqlDialect::SqlServer | SqlDialect::PostgreSql | SqlDialect::CockroachDb => {
            let unique = if idx.non_unique { "" } else { "UNIQUE " };
            format!(
                "CREATE {unique}INDEX {} ON {fq} ({cols_sql});",
                dialect.quote_ident(&idx.name)
            )
        }
        SqlDialect::Sqlite => {
            let unique = if idx.non_unique { "" } else { "UNIQUE " };
            format!(
                "CREATE {unique}INDEX {} ON {} ({cols_sql});",
                qualify_table(dialect, target_schema, &idx.name),
                dialect.quote_ident(table_name)
            )
        }
    }
}

fn format_drop_index(fq: &str, target_schema: &str, idx: &IndexInfo, dialect: SqlDialect) -> String {
    match dialect {
        SqlDialect::MySql => {
            if idx.name.eq_ignore_ascii_case("PRIMARY") {
                format!("ALTER TABLE {fq} DROP PRIMARY KEY;")
            } else {
                format!(
                    "ALTER TABLE {fq} DROP INDEX {};",
                    dialect.quote_ident(&idx.name)
                )
            }
        }
        SqlDialect::SqlServer => format!(
            "DROP INDEX {} ON {fq};",
            dialect.quote_ident(&idx.name)
        ),
        SqlDialect::CockroachDb => format!("DROP INDEX {fq}@{};", dialect.quote_ident(&idx.name)),
        SqlDialect::PostgreSql | SqlDialect::Sqlite => {
            format!("DROP INDEX {};", qualify_table(dialect, target_schema, &idx.name))
        }
    }
}

fn format_add_fk(fq: &str, target_schema: &str, fk: &ForeignKeyInfo, dialect: SqlDialect) -> String {
    if dialect == SqlDialect::Sqlite {
        return format!(
            "-- SQLite cannot add foreign key {} on {fq} in place; rebuild the table to add it.",
            dialect.quote_ident(&fk.constraint_name)
        );
    }
    let ref_schema = if fk.ref_schema.is_empty() {
        target_schema
    } else {
        &fk.ref_schema
    };
    let ref_fq = qualify_table(dialect, ref_schema, &fk.ref_table);
    let mut stmt = format!(
        "ALTER TABLE {fq} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {ref_fq} ({})",
        dialect.quote_ident(&fk.constraint_name),
        dialect.quote_ident(&fk.column_name),
        dialect.quote_ident(&fk.ref_column),
    );
    let on_update = fk.update_rule.trim();
    if !on_update.is_empty() && !on_update.eq_ignore_ascii_case("NO ACTION") {
        stmt.push_str(&format!(" ON UPDATE {on_update}"));
    }
    let on_delete = fk.delete_rule.trim();
    if !on_delete.is_empty() && !on_delete.eq_ignore_ascii_case("NO ACTION") {
        stmt.push_str(&format!(" ON DELETE {on_delete}"));
    }
    stmt.push(';');
    stmt
}

fn format_drop_fk(fq: &str, fk: &ForeignKeyInfo, dialect: SqlDialect) -> String {
    let constraint = dialect.quote_ident(&fk.constraint_name);
    match dialect {
        SqlDialect::MySql => format!("ALTER TABLE {fq} DROP FOREIGN KEY {constraint};"),
        SqlDialect::SqlServer | SqlDialect::PostgreSql | SqlDialect::CockroachDb => {
            format!("ALTER TABLE {fq} DROP CONSTRAINT {constraint};")
        }
        SqlDialect::Sqlite => format!(
            "-- SQLite cannot drop foreign key {constraint} on {fq} in place; rebuild the table to remove it."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cottondata_driver::{ColumnSummary, SchemaObjects, SchemaSummary, TableSummary};

    fn col(
        name: &str,
        column_type: &str,
        is_nullable: bool,
        is_primary_key: bool,
        is_auto_increment: bool,
        default_expr: Option<&str>,
    ) -> ColumnSnapshot {
        ColumnSnapshot {
            name: name.to_string(),
            column_type: column_type.to_string(),
            is_nullable,
            is_primary_key,
            is_auto_increment,
            default_expr: default_expr.map(str::to_string),
            comment: None,
        }
    }

    fn table(
        name: &str,
        columns: Vec<ColumnSnapshot>,
        detail: Option<TableDetail>,
    ) -> TableSnapshot {
        TableSnapshot {
            name: name.to_string(),
            kind: TableKind::BaseTable,
            columns,
            detail,
        }
    }

    fn detail(
        ddl: &str,
        indexes: Vec<IndexInfo>,
        foreign_keys: Vec<ForeignKeyInfo>,
    ) -> TableDetail {
        TableDetail {
            ddl: ddl.to_string(),
            engine: Some("InnoDB".to_string()),
            collation: None,
            row_estimate: None,
            indexes,
            foreign_keys,
            inbound_foreign_keys: Vec::new(),
        }
    }

    #[test]
    fn snapshot_from_catalog_extracts_matching_schema_case_insensitively() {
        let mut catalog = SchemaCatalog::new();
        catalog.apply_schemas(vec![SchemaSummary {
            name: "Shop".to_string(),
            default_collation: None,
            is_system: false,
        }]);
        catalog.apply_schema_objects(
            "shop",
            SchemaObjects {
                tables: vec![TableSummary {
                    name: "orders".to_string(),
                    kind: TableKind::BaseTable,
                    columns: vec![ColumnSummary {
                        name: "id".to_string(),
                        data_type: "bigint".to_string(),
                        column_type: "bigint unsigned".to_string(),
                        is_primary_key: true,
                        is_nullable: false,
                        is_auto_increment: true,
                        default_expr: None,
                        comment: Some("primary key".to_string()),
                    }],
                }],
                routines: Vec::new(),
            },
        );

        let snap = snapshot_from_catalog(&catalog, "SHOP");
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].name, "orders");
        assert_eq!(snap[0].columns.len(), 1);
        assert_eq!(snap[0].columns[0].column_type, "bigint unsigned");
        assert_eq!(snap[0].columns[0].comment.as_deref(), Some("primary key"));
        assert!(snapshot_from_catalog(&catalog, "missing").is_empty());
    }

    #[test]
    fn identical_schemas_with_case_and_whitespace_differences_produce_no_diff() {
        let src = vec![table(
            "Users",
            vec![
                col("id", "BIGINT  UNSIGNED", false, true, true, None),
                col("status", "VARCHAR(32)", false, false, false, Some(" 'active' ")),
            ],
            Some(detail(
                "",
                vec![IndexInfo {
                    name: "idx_status".to_string(),
                    non_unique: true,
                    index_type: "BTREE".to_string(),
                    columns: vec!["status".to_string()],
                }],
                Vec::new(),
            )),
        )];
        let dst = vec![table(
            "users",
            vec![
                col("ID", "bigint unsigned", false, true, true, None),
                col("STATUS", "varchar(32)", false, false, false, Some("'ACTIVE'")),
            ],
            Some(detail(
                "",
                vec![IndexInfo {
                    name: "IDX_STATUS".to_string(),
                    non_unique: true,
                    index_type: "btree".to_string(),
                    columns: vec!["STATUS".to_string()],
                }],
                Vec::new(),
            )),
        )];

        let diff = compare_schema_snapshots("dev".into(), "staging".into(), &src, &dst);
        assert!(!diff.has_differences());
        assert_eq!(diff.identical_count, 1);
        assert!(diff.only_in_source.is_empty());
        assert!(diff.only_in_target.is_empty());
        assert!(diff.modified_tables.is_empty());

        let sql = generate_migration_sql(&diff, "staging", SqlDialect::MySql, true);
        assert!(sql.contains("-- Migration: dev -> staging"));
        assert!(sql.contains("-- No changes required."));
    }

    #[test]
    fn added_and_removed_tables_are_sorted_alphabetically() {
        let src = vec![
            table("zeta", vec![col("id", "int", false, true, false, None)], None),
            table("alpha", vec![col("id", "int", false, true, false, None)], None),
        ];
        let dst = vec![
            table("omega", vec![col("id", "int", false, true, false, None)], None),
            table("beta", vec![col("id", "int", false, true, false, None)], None),
        ];

        let diff = compare_schema_snapshots("dev".into(), "prod".into(), &src, &dst);
        assert!(diff.has_differences());
        assert_eq!(diff.identical_count, 0);
        let added_names: Vec<&str> = diff.only_in_source.iter().map(|t| t.name.as_str()).collect();
        let removed_names: Vec<&str> = diff.only_in_target.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(added_names, vec!["alpha", "zeta"]);
        assert_eq!(removed_names, vec!["beta", "omega"]);
    }

    #[test]
    fn detects_added_removed_and_altered_columns() {
        let src = vec![table(
            "accounts",
            vec![
                col("id", "bigint", false, true, true, None),
                col("email", "varchar(320)", false, false, false, None),
                col("tier", "varchar(20)", false, false, false, Some("'free'")),
                col("verified_at", "datetime", true, false, false, None),
            ],
            None,
        )];
        let dst = vec![table(
            "accounts",
            vec![
                col("id", "bigint", false, true, true, None),
                col("email", "varchar(120)", true, false, false, None),
                col("tier", "varchar(20)", false, false, false, Some("'basic'")),
                col("legacy_flag", "tinyint(1)", false, false, false, Some("0")),
            ],
            None,
        )];

        let diff = compare_schema_snapshots("dev".into(), "staging".into(), &src, &dst);
        assert!(diff.has_differences());
        assert_eq!(diff.modified_tables.len(), 1);
        let td = &diff.modified_tables[0];
        assert!(!td.is_empty());
        assert_eq!(td.added_columns.len(), 1);
        assert_eq!(td.added_columns[0].name, "verified_at");
        assert_eq!(td.removed_columns.len(), 1);
        assert_eq!(td.removed_columns[0].name, "legacy_flag");
        assert_eq!(td.altered_columns.len(), 2);
        assert_eq!(td.altered_columns[0].0.name, "email");
        assert_eq!(td.altered_columns[0].0.column_type, "varchar(320)");
        assert_eq!(td.altered_columns[0].1.column_type, "varchar(120)");
        assert_eq!(td.altered_columns[1].0.name, "tier");
        assert_eq!(td.change_count(), 4);
    }

    #[test]
    fn detects_index_and_foreign_key_diffs_including_redefinitions() {
        let src_detail = detail(
            "",
            vec![
                IndexInfo {
                    name: "idx_new".to_string(),
                    non_unique: false,
                    index_type: "BTREE".to_string(),
                    columns: vec!["email".to_string()],
                },
                IndexInfo {
                    name: "idx_changed".to_string(),
                    non_unique: true,
                    index_type: "BTREE".to_string(),
                    columns: vec!["org_id".to_string(), "created_at".to_string()],
                },
            ],
            vec![
                ForeignKeyInfo {
                    constraint_name: "fk_org".to_string(),
                    column_name: "org_id".to_string(),
                    ref_schema: "app".to_string(),
                    ref_table: "orgs".to_string(),
                    ref_column: "id".to_string(),
                    update_rule: "NO ACTION".to_string(),
                    delete_rule: "CASCADE".to_string(),
                },
            ],
        );
        let dst_detail = detail(
            "",
            vec![
                IndexInfo {
                    name: "idx_old".to_string(),
                    non_unique: true,
                    index_type: "BTREE".to_string(),
                    columns: vec!["legacy".to_string()],
                },
                IndexInfo {
                    name: "idx_changed".to_string(),
                    non_unique: true,
                    index_type: "BTREE".to_string(),
                    columns: vec!["org_id".to_string()],
                },
            ],
            vec![
                ForeignKeyInfo {
                    constraint_name: "fk_legacy".to_string(),
                    column_name: "legacy_id".to_string(),
                    ref_schema: "app".to_string(),
                    ref_table: "legacy".to_string(),
                    ref_column: "id".to_string(),
                    update_rule: "NO ACTION".to_string(),
                    delete_rule: "NO ACTION".to_string(),
                },
            ],
        );

        let cols = vec![col("id", "bigint", false, true, true, None)];
        let src = vec![table("members", cols.clone(), Some(src_detail))];
        let dst = vec![table("members", cols, Some(dst_detail))];

        let diff = compare_schema_snapshots("dev".into(), "staging".into(), &src, &dst);
        assert_eq!(diff.modified_tables.len(), 1);
        let td = &diff.modified_tables[0];
        assert_eq!(td.added_indexes.len(), 2);
        assert_eq!(td.removed_indexes.len(), 2);
        assert_eq!(td.added_fks.len(), 1);
        assert_eq!(td.added_fks[0].constraint_name, "fk_org");
        assert_eq!(td.removed_fks.len(), 1);
        assert_eq!(td.removed_fks[0].constraint_name, "fk_legacy");
        assert_eq!(td.change_count(), 6);
    }

    #[test]
    fn generates_mysql_migration_with_and_without_drops() {
        let src = vec![
            table(
                "audit_log",
                vec![
                    col("id", "bigint", false, true, true, None),
                    col("payload", "text", true, false, false, None),
                ],
                None,
            ),
            table(
                "custom_ddl",
                vec![col("id", "int", false, true, false, None)],
                Some(detail("CREATE TABLE `custom_ddl` (`id` int NOT NULL)", Vec::new(), Vec::new())),
            ),
            table(
                "users",
                vec![
                    col("id", "bigint", false, true, true, None),
                    col("email", "varchar(255)", false, false, false, None),
                    col("org_id", "bigint", false, false, false, None),
                ],
                Some(detail(
                    "",
                    vec![IndexInfo {
                        name: "uq_users_email".to_string(),
                        non_unique: false,
                        index_type: "BTREE".to_string(),
                        columns: vec!["email".to_string()],
                    }],
                    vec![ForeignKeyInfo {
                        constraint_name: "fk_users_org".to_string(),
                        column_name: "org_id".to_string(),
                        ref_schema: "staging".to_string(),
                        ref_table: "orgs".to_string(),
                        ref_column: "id".to_string(),
                        update_rule: "NO ACTION".to_string(),
                        delete_rule: "CASCADE".to_string(),
                    }],
                )),
            ),
        ];
        let dst = vec![
            table(
                "users",
                vec![
                    col("id", "bigint", false, true, true, None),
                    col("email", "varchar(100)", true, false, false, None),
                    col("old_col", "int", true, false, false, None),
                ],
                Some(detail(
                    "",
                    vec![IndexInfo {
                        name: "idx_old".to_string(),
                        non_unique: true,
                        index_type: "BTREE".to_string(),
                        columns: vec!["old_col".to_string()],
                    }],
                    vec![ForeignKeyInfo {
                        constraint_name: "fk_old".to_string(),
                        column_name: "old_col".to_string(),
                        ref_schema: "staging".to_string(),
                        ref_table: "legacy".to_string(),
                        ref_column: "id".to_string(),
                        update_rule: "NO ACTION".to_string(),
                        delete_rule: "NO ACTION".to_string(),
                    }],
                )),
            ),
            table("obsolete", vec![col("id", "int", false, true, false, None)], None),
        ];

        let diff = compare_schema_snapshots("dev".into(), "staging".into(), &src, &dst);

        let safe_sql = generate_migration_sql(&diff, "staging", SqlDialect::MySql, false);
        assert!(safe_sql.contains("-- Migration: dev -> staging"));
        assert!(safe_sql.contains("CREATE TABLE `staging`.`audit_log`"));
        assert!(safe_sql.contains("AUTO_INCREMENT"));
        assert!(safe_sql.contains("PRIMARY KEY (`id`)"));
        assert!(safe_sql.contains("CREATE TABLE `custom_ddl` (`id` int NOT NULL);"));
        assert!(safe_sql.contains("ALTER TABLE `staging`.`users` ADD COLUMN `org_id` bigint NOT NULL;"));
        assert!(safe_sql.contains("ALTER TABLE `staging`.`users` MODIFY COLUMN `email` varchar(255) NOT NULL;"));
        assert!(safe_sql.contains("ALTER TABLE `staging`.`users` ADD UNIQUE INDEX `uq_users_email` (`email`);"));
        assert!(safe_sql.contains("ALTER TABLE `staging`.`users` ADD CONSTRAINT `fk_users_org` FOREIGN KEY (`org_id`) REFERENCES `staging`.`orgs` (`id`) ON DELETE CASCADE;"));
        assert!(!safe_sql.contains("DROP "));

        let full_sql = generate_migration_sql(&diff, "staging", SqlDialect::MySql, true);
        assert!(full_sql.contains("ALTER TABLE `staging`.`users` DROP FOREIGN KEY `fk_old`;"));
        assert!(full_sql.contains("ALTER TABLE `staging`.`users` DROP INDEX `idx_old`;"));
        assert!(full_sql.contains("ALTER TABLE `staging`.`users` DROP COLUMN `old_col`;"));
        assert!(full_sql.contains("DROP TABLE `staging`.`obsolete`;"));
    }

    #[test]
    fn generates_sql_server_migration_with_and_without_drops() {
        let src = vec![
            table(
                "dbo.events",
                vec![
                    col("id", "bigint", false, true, true, None),
                    col("kind", "nvarchar(64)", false, false, false, Some("N'info'")),
                ],
                None,
            ),
            table(
                "users",
                vec![
                    col("id", "int", false, true, true, None),
                    col("email", "nvarchar(255)", false, false, false, None),
                    col("org_id", "int", false, false, false, Some("0")),
                ],
                Some(detail(
                    "",
                    vec![IndexInfo {
                        name: "ix_users_email".to_string(),
                        non_unique: false,
                        index_type: "NONCLUSTERED".to_string(),
                        columns: vec!["email".to_string()],
                    }],
                    vec![ForeignKeyInfo {
                        constraint_name: "fk_users_org".to_string(),
                        column_name: "org_id".to_string(),
                        ref_schema: "prod".to_string(),
                        ref_table: "orgs".to_string(),
                        ref_column: "id".to_string(),
                        update_rule: "NO ACTION".to_string(),
                        delete_rule: "SET NULL".to_string(),
                    }],
                )),
            ),
        ];
        let dst = vec![
            table(
                "users",
                vec![
                    col("id", "int", false, true, true, None),
                    col("email", "nvarchar(100)", true, false, false, None),
                    col("deprecated", "bit", false, false, false, Some("0")),
                ],
                Some(detail(
                    "",
                    vec![IndexInfo {
                        name: "ix_old".to_string(),
                        non_unique: true,
                        index_type: "NONCLUSTERED".to_string(),
                        columns: vec!["deprecated".to_string()],
                    }],
                    vec![ForeignKeyInfo {
                        constraint_name: "fk_old".to_string(),
                        column_name: "deprecated".to_string(),
                        ref_schema: "prod".to_string(),
                        ref_table: "legacy".to_string(),
                        ref_column: "id".to_string(),
                        update_rule: "NO ACTION".to_string(),
                        delete_rule: "NO ACTION".to_string(),
                    }],
                )),
            ),
            TableSnapshot {
                name: "v_legacy".to_string(),
                kind: TableKind::View,
                columns: Vec::new(),
                detail: None,
            },
        ];

        let diff = compare_schema_snapshots("dev".into(), "prod".into(), &src, &dst);

        let safe_sql = generate_migration_sql(&diff, "prod", SqlDialect::SqlServer, false);
        assert!(safe_sql.contains("CREATE TABLE [prod].[dbo].[events]"));
        assert!(safe_sql.contains("IDENTITY(1,1)"));
        assert!(safe_sql.contains("DEFAULT N'info'"));
        assert!(safe_sql.contains("ALTER TABLE [prod].[dbo].[users] ADD [org_id] int NOT NULL DEFAULT 0;"));
        assert!(safe_sql.contains("ALTER TABLE [prod].[dbo].[users] ALTER COLUMN [email] nvarchar(255) NOT NULL;"));
        assert!(safe_sql.contains("CREATE UNIQUE INDEX [ix_users_email] ON [prod].[dbo].[users] ([email]);"));
        assert!(safe_sql.contains("ALTER TABLE [prod].[dbo].[users] ADD CONSTRAINT [fk_users_org] FOREIGN KEY ([org_id]) REFERENCES [prod].[dbo].[orgs] ([id]) ON DELETE SET NULL;"));
        assert!(!safe_sql.contains("DROP "));

        let full_sql = generate_migration_sql(&diff, "prod", SqlDialect::SqlServer, true);
        assert!(full_sql.contains("ALTER TABLE [prod].[dbo].[users] DROP CONSTRAINT [fk_old];"));
        assert!(full_sql.contains("DROP INDEX [ix_old] ON [prod].[dbo].[users];"));
        assert!(full_sql.contains("ALTER TABLE [prod].[dbo].[users] DROP COLUMN [deprecated];"));
        assert!(full_sql.contains("DROP VIEW [prod].[dbo].[v_legacy];"));
    }

    #[test]
    fn generates_postgres_and_sqlite_migrations_with_engine_specific_syntax() {
        let index = IndexInfo {
            name: "ix_users_email".to_string(),
            non_unique: false,
            index_type: "btree".to_string(),
            columns: vec!["email".to_string()],
        };
        let src = vec![table(
            "users",
            vec![
                col("id", "bigint", false, true, true, None),
                col("email", "varchar(255)", false, false, false, None),
                col("status", "text", false, false, false, Some("'active'")),
            ],
            Some(detail("", vec![index], Vec::new())),
        )];
        let dst = vec![table(
            "users",
            vec![
                col("id", "bigint", false, true, true, None),
                col("email", "varchar(100)", true, false, false, None),
            ],
            Some(detail("", Vec::new(), Vec::new())),
        )];
        let diff = compare_schema_snapshots("dev".into(), "public".into(), &src, &dst);

        let postgres_sql = generate_migration_sql(&diff, "public", SqlDialect::PostgreSql, false);
        assert!(postgres_sql.contains(
            "ALTER TABLE \"public\".\"users\" ADD COLUMN \"status\" text NOT NULL DEFAULT 'active';"
        ));
        assert!(postgres_sql.contains(
            "ALTER TABLE \"public\".\"users\" ALTER COLUMN \"email\" TYPE varchar(255), ALTER COLUMN \"email\" SET NOT NULL, ALTER COLUMN \"email\" DROP DEFAULT;"
        ));
        assert!(postgres_sql.contains(
            "CREATE UNIQUE INDEX \"ix_users_email\" ON \"public\".\"users\" (\"email\");"
        ));

        let sqlite_sql = generate_migration_sql(&diff, "main", SqlDialect::Sqlite, false);
        assert!(sqlite_sql.contains("CREATE UNIQUE INDEX \"main\".\"ix_users_email\" ON \"users\" (\"email\");"));
        assert!(sqlite_sql.contains("-- SQLite cannot alter column \"email\""));
    }
}
