use cottondata_core::SqlDialect;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDraft {
    pub original_name: Option<String>,
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default_expr: String,
    pub auto_increment: bool,
    pub comment: String,
    pub is_dropped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDraft {
    pub original_name: Option<String>,
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub is_dropped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyDraft {
    pub original_name: Option<String>,
    pub name: String,
    pub column: String,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_column: String,
    pub on_update: String,
    pub on_delete: String,
    pub is_dropped: bool,
}

#[derive(Debug, Clone)]
pub struct TableStructureEdit {
    pub schema: String,
    pub table: String,
    pub original_columns: Vec<ColumnDraft>,
    pub columns: Vec<ColumnDraft>,
    pub original_indexes: Vec<IndexDraft>,
    pub indexes: Vec<IndexDraft>,
    pub original_fks: Vec<ForeignKeyDraft>,
    pub fks: Vec<ForeignKeyDraft>,
}

pub fn build_alter_table_statements(
    dialect: SqlDialect,
    edit: &TableStructureEdit,
) -> Result<Vec<String>, String> {
    validate_edit(edit)?;

    let table_ref = format_table_ref(dialect, &edit.schema, &edit.table);
    let mut statements = Vec::new();

    let (dropped_fks, added_fks) = diff_foreign_keys(&edit.original_fks, &edit.fks);
    for fk_name in &dropped_fks {
        let q_name = dialect.quote_ident(fk_name);
        let stmt = match dialect {
            SqlDialect::MySql => format!("ALTER TABLE {table_ref} DROP FOREIGN KEY {q_name}"),
            SqlDialect::SqlServer => format!("ALTER TABLE {table_ref} DROP CONSTRAINT {q_name}"),
        };
        statements.push(stmt);
    }

    let (dropped_indexes, added_indexes) = diff_indexes(&edit.original_indexes, &edit.indexes);
    for idx_name in &dropped_indexes {
        let stmt = match dialect {
            SqlDialect::MySql => {
                if idx_name.eq_ignore_ascii_case("PRIMARY") {
                    format!("ALTER TABLE {table_ref} DROP PRIMARY KEY")
                } else {
                    format!(
                        "ALTER TABLE {table_ref} DROP INDEX {}",
                        dialect.quote_ident(idx_name)
                    )
                }
            }
            SqlDialect::SqlServer => {
                format!(
                    "DROP INDEX {} ON {table_ref}",
                    dialect.quote_ident(idx_name)
                )
            }
        };
        statements.push(stmt);
    }

    let dropped_columns = collect_dropped_columns(&edit.original_columns, &edit.columns);
    for col_name in &dropped_columns {
        statements.push(format!(
            "ALTER TABLE {table_ref} DROP COLUMN {}",
            dialect.quote_ident(col_name)
        ));
    }

    for col in &edit.columns {
        if col.is_dropped {
            continue;
        }
        let orig_name = col
            .original_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if orig_name.is_none() {
            statements.push(build_add_column_sql(dialect, &table_ref, col));
        }
    }

    let orig_col_map: HashMap<&str, &ColumnDraft> = edit
        .original_columns
        .iter()
        .map(|c| {
            let key = c
                .original_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| c.name.trim());
            (key, c)
        })
        .collect();

    for col in &edit.columns {
        if col.is_dropped {
            continue;
        }
        let Some(orig_name) = col
            .original_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let orig_col = orig_col_map.get(orig_name).copied();
        statements.extend(build_modify_column_sql(
            dialect,
            &table_ref,
            &edit.table,
            orig_name,
            orig_col,
            col,
        ));
    }

    for idx in added_indexes {
        let cols_sql = idx
            .columns
            .iter()
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .map(|c| dialect.quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        let q_name = dialect.quote_ident(idx.name.trim());
        let unique_kw = if idx.unique { "UNIQUE " } else { "" };
        let stmt = match dialect {
            SqlDialect::MySql => {
                format!("ALTER TABLE {table_ref} ADD {unique_kw}INDEX {q_name} ({cols_sql})")
            }
            SqlDialect::SqlServer => {
                format!("CREATE {unique_kw}INDEX {q_name} ON {table_ref} ({cols_sql})")
            }
        };
        statements.push(stmt);
    }

    for fk in added_fks {
        let q_name = dialect.quote_ident(fk.name.trim());
        let q_col = dialect.quote_ident(fk.column.trim());
        let ref_schema = if fk.ref_schema.trim().is_empty() {
            edit.schema.trim()
        } else {
            fk.ref_schema.trim()
        };
        let ref_table_ref = format_table_ref(dialect, ref_schema, fk.ref_table.trim());
        let q_ref_col = dialect.quote_ident(fk.ref_column.trim());

        let mut stmt = format!(
            "ALTER TABLE {table_ref} ADD CONSTRAINT {q_name} FOREIGN KEY ({q_col}) REFERENCES {ref_table_ref} ({q_ref_col})"
        );
        if !fk.on_delete.trim().is_empty() {
            stmt.push_str(&format!(" ON DELETE {}", fk.on_delete.trim()));
        }
        if !fk.on_update.trim().is_empty() {
            stmt.push_str(&format!(" ON UPDATE {}", fk.on_update.trim()));
        }
        statements.push(stmt);
    }

    Ok(statements)
}

fn validate_edit(edit: &TableStructureEdit) -> Result<(), String> {
    if edit.table.trim().is_empty() {
        return Err("Table name cannot be empty".to_string());
    }

    for col in &edit.columns {
        if col.is_dropped {
            continue;
        }
        if col.name.trim().is_empty() {
            return Err("Column name cannot be empty".to_string());
        }
        if col.data_type.trim().is_empty() {
            return Err(format!(
                "Column '{}' data type cannot be empty",
                col.name.trim()
            ));
        }
    }

    for idx in &edit.indexes {
        if idx.is_dropped {
            continue;
        }
        if idx.name.trim().is_empty() {
            return Err("Index name cannot be empty".to_string());
        }
        let has_col = idx.columns.iter().any(|c| !c.trim().is_empty());
        if !has_col {
            return Err(format!(
                "Index '{}' must specify at least one column",
                idx.name.trim()
            ));
        }
    }

    for fk in &edit.fks {
        if fk.is_dropped {
            continue;
        }
        if fk.name.trim().is_empty() {
            return Err("Foreign key name cannot be empty".to_string());
        }
        if fk.column.trim().is_empty() {
            return Err(format!(
                "Foreign key '{}' column cannot be empty",
                fk.name.trim()
            ));
        }
        if fk.ref_table.trim().is_empty() {
            return Err(format!(
                "Foreign key '{}' referenced table cannot be empty",
                fk.name.trim()
            ));
        }
        if fk.ref_column.trim().is_empty() {
            return Err(format!(
                "Foreign key '{}' referenced column cannot be empty",
                fk.name.trim()
            ));
        }
    }

    Ok(())
}

fn format_table_ref(dialect: SqlDialect, schema: &str, table: &str) -> String {
    let schema = schema.trim();
    let table = table.trim();
    if !schema.is_empty() {
        return dialect.qualified_table(schema, table);
    }
    match dialect {
        SqlDialect::MySql => dialect.quote_ident(table),
        SqlDialect::SqlServer => {
            if let Some((owner, object)) = table.split_once('.') {
                format!(
                    "{}.{}",
                    dialect.quote_ident(owner),
                    dialect.quote_ident(object)
                )
            } else {
                dialect.quote_ident(table)
            }
        }
    }
}

fn format_mysql_column_spec(dialect: SqlDialect, col: &ColumnDraft) -> String {
    let nullability = if col.nullable { "NULL" } else { "NOT NULL" };
    let mut spec = format!("{} {nullability}", col.data_type.trim());
    if !col.default_expr.trim().is_empty() {
        spec.push_str(&format!(" DEFAULT {}", col.default_expr.trim()));
    }
    if col.auto_increment {
        spec.push_str(" AUTO_INCREMENT");
    }
    if !col.comment.trim().is_empty() {
        spec.push_str(&format!(
            " COMMENT {}",
            dialect.string_literal(col.comment.trim())
        ));
    }
    spec
}

fn build_add_column_sql(dialect: SqlDialect, table_ref: &str, col: &ColumnDraft) -> String {
    let q_name = dialect.quote_ident(col.name.trim());
    match dialect {
        SqlDialect::MySql => {
            let spec = format_mysql_column_spec(dialect, col);
            format!("ALTER TABLE {table_ref} ADD COLUMN {q_name} {spec}")
        }
        SqlDialect::SqlServer => {
            let nullability = if col.nullable { "NULL" } else { "NOT NULL" };
            let mut spec = col.data_type.trim().to_string();
            if col.auto_increment {
                spec.push_str(" IDENTITY(1,1)");
            }
            spec.push_str(&format!(" {nullability}"));
            if !col.default_expr.trim().is_empty() {
                spec.push_str(&format!(" DEFAULT {}", col.default_expr.trim()));
            }
            format!("ALTER TABLE {table_ref} ADD {q_name} {spec}")
        }
    }
}

fn build_modify_column_sql(
    dialect: SqlDialect,
    table_ref: &str,
    raw_table: &str,
    orig_name: &str,
    orig_col: Option<&ColumnDraft>,
    col: &ColumnDraft,
) -> Vec<String> {
    let new_name = col.name.trim();
    let renamed = orig_name != new_name;

    let (type_changed, nullable_changed, default_changed, auto_inc_changed, comment_changed) =
        match orig_col {
            Some(orig) => (
                orig.data_type.trim() != col.data_type.trim(),
                orig.nullable != col.nullable,
                orig.default_expr.trim() != col.default_expr.trim(),
                orig.auto_increment != col.auto_increment,
                orig.comment.trim() != col.comment.trim(),
            ),
            None => (true, false, false, false, false),
        };

    match dialect {
        SqlDialect::MySql => {
            let spec_changed = type_changed
                || nullable_changed
                || default_changed
                || auto_inc_changed
                || comment_changed;
            if !renamed && !spec_changed {
                return Vec::new();
            }
            let spec = format_mysql_column_spec(dialect, col);
            let q_new = dialect.quote_ident(new_name);
            if renamed {
                let q_orig = dialect.quote_ident(orig_name);
                vec![format!(
                    "ALTER TABLE {table_ref} CHANGE COLUMN {q_orig} {q_new} {spec}"
                )]
            } else {
                vec![format!(
                    "ALTER TABLE {table_ref} MODIFY COLUMN {q_new} {spec}"
                )]
            }
        }
        SqlDialect::SqlServer => {
            let mut stmts = Vec::new();
            if renamed {
                let (owner, object) = raw_table.trim().split_once('.').unwrap_or(("dbo", raw_table.trim()));
                let target = format!(
                    "{}.{}.{}",
                    dialect.quote_ident(owner),
                    dialect.quote_ident(object),
                    dialect.quote_ident(orig_name)
                );
                stmts.push(format!(
                    "EXEC sp_rename {}, {}, N'COLUMN'",
                    dialect.string_literal(&target),
                    dialect.string_literal(new_name)
                ));
            }
            if type_changed || nullable_changed {
                let nullability = if col.nullable { "NULL" } else { "NOT NULL" };
                stmts.push(format!(
                    "ALTER TABLE {table_ref} ALTER COLUMN {} {} {nullability}",
                    dialect.quote_ident(new_name),
                    col.data_type.trim()
                ));
            }
            if default_changed && !col.default_expr.trim().is_empty() {
                stmts.push(format!(
                    "ALTER TABLE {table_ref} ADD DEFAULT {} FOR {}",
                    col.default_expr.trim(),
                    dialect.quote_ident(new_name)
                ));
            }
            stmts
        }
    }
}

fn collect_dropped_columns(
    original_columns: &[ColumnDraft],
    columns: &[ColumnDraft],
) -> Vec<String> {
    let mut dropped = Vec::new();
    let mut seen = HashSet::new();
    let mut referenced = HashSet::new();

    for col in columns {
        if let Some(orig) = col
            .original_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            referenced.insert(orig.to_string());
            if col.is_dropped && seen.insert(orig.to_string()) {
                dropped.push(orig.to_string());
            }
        }
    }

    for orig in original_columns {
        let key = orig
            .original_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| orig.name.trim());
        if !key.is_empty() && !referenced.contains(key) && seen.insert(key.to_string()) {
            dropped.push(key.to_string());
        }
    }

    dropped
}

fn normalize_columns(columns: &[String]) -> Vec<&str> {
    columns
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect()
}

fn index_changed(orig: &IndexDraft, orig_name: &str, curr: &IndexDraft) -> bool {
    orig_name != curr.name.trim()
        || orig.unique != curr.unique
        || normalize_columns(&orig.columns) != normalize_columns(&curr.columns)
}

fn diff_indexes<'a>(
    original_indexes: &'a [IndexDraft],
    indexes: &'a [IndexDraft],
) -> (Vec<String>, Vec<&'a IndexDraft>) {
    let orig_map: HashMap<&str, &IndexDraft> = original_indexes
        .iter()
        .map(|idx| {
            let key = idx
                .original_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| idx.name.trim());
            (key, idx)
        })
        .collect();

    let mut dropped = Vec::new();
    let mut dropped_seen = HashSet::new();
    let mut added = Vec::new();
    let mut referenced = HashSet::new();

    for idx in indexes {
        let orig_name = idx
            .original_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        match orig_name {
            Some(orig) => {
                referenced.insert(orig.to_string());
                if idx.is_dropped {
                    if dropped_seen.insert(orig.to_string()) {
                        dropped.push(orig.to_string());
                    }
                } else {
                    let changed = match orig_map.get(orig) {
                        Some(orig_idx) => index_changed(orig_idx, orig, idx),
                        None => orig != idx.name.trim(),
                    };
                    if changed {
                        if dropped_seen.insert(orig.to_string()) {
                            dropped.push(orig.to_string());
                        }
                        added.push(idx);
                    }
                }
            }
            None => {
                if !idx.is_dropped {
                    added.push(idx);
                }
            }
        }
    }

    for orig in original_indexes {
        let key = orig
            .original_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| orig.name.trim());
        if !key.is_empty() && !referenced.contains(key) && dropped_seen.insert(key.to_string()) {
            dropped.push(key.to_string());
        }
    }

    (dropped, added)
}

fn fk_changed(orig: &ForeignKeyDraft, orig_name: &str, curr: &ForeignKeyDraft) -> bool {
    orig_name != curr.name.trim()
        || orig.column.trim() != curr.column.trim()
        || orig.ref_schema.trim() != curr.ref_schema.trim()
        || orig.ref_table.trim() != curr.ref_table.trim()
        || orig.ref_column.trim() != curr.ref_column.trim()
        || orig.on_update.trim() != curr.on_update.trim()
        || orig.on_delete.trim() != curr.on_delete.trim()
}

fn diff_foreign_keys<'a>(
    original_fks: &'a [ForeignKeyDraft],
    fks: &'a [ForeignKeyDraft],
) -> (Vec<String>, Vec<&'a ForeignKeyDraft>) {
    let orig_map: HashMap<&str, &ForeignKeyDraft> = original_fks
        .iter()
        .map(|fk| {
            let key = fk
                .original_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| fk.name.trim());
            (key, fk)
        })
        .collect();

    let mut dropped = Vec::new();
    let mut dropped_seen = HashSet::new();
    let mut added = Vec::new();
    let mut referenced = HashSet::new();

    for fk in fks {
        let orig_name = fk
            .original_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        match orig_name {
            Some(orig) => {
                referenced.insert(orig.to_string());
                if fk.is_dropped {
                    if dropped_seen.insert(orig.to_string()) {
                        dropped.push(orig.to_string());
                    }
                } else {
                    let changed = match orig_map.get(orig) {
                        Some(orig_fk) => fk_changed(orig_fk, orig, fk),
                        None => orig != fk.name.trim(),
                    };
                    if changed {
                        if dropped_seen.insert(orig.to_string()) {
                            dropped.push(orig.to_string());
                        }
                        added.push(fk);
                    }
                }
            }
            None => {
                if !fk.is_dropped {
                    added.push(fk);
                }
            }
        }
    }

    for orig in original_fks {
        let key = orig
            .original_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| orig.name.trim());
        if !key.is_empty() && !referenced.contains(key) && dropped_seen.insert(key.to_string()) {
            dropped.push(key.to_string());
        }
    }

    (dropped, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(original_name: Option<&str>, name: &str, data_type: &str) -> ColumnDraft {
        ColumnDraft {
            original_name: original_name.map(str::to_string),
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable: false,
            default_expr: String::new(),
            auto_increment: false,
            comment: String::new(),
            is_dropped: false,
        }
    }

    fn idx(
        original_name: Option<&str>,
        name: &str,
        columns: &[&str],
        unique: bool,
        is_dropped: bool,
    ) -> IndexDraft {
        IndexDraft {
            original_name: original_name.map(str::to_string),
            name: name.to_string(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            unique,
            is_dropped,
        }
    }

    fn fk(
        original_name: Option<&str>,
        name: &str,
        column: &str,
        ref_schema: &str,
        ref_table: &str,
        ref_column: &str,
    ) -> ForeignKeyDraft {
        ForeignKeyDraft {
            original_name: original_name.map(str::to_string),
            name: name.to_string(),
            column: column.to_string(),
            ref_schema: ref_schema.to_string(),
            ref_table: ref_table.to_string(),
            ref_column: ref_column.to_string(),
            on_update: String::new(),
            on_delete: String::new(),
            is_dropped: false,
        }
    }

    fn base_edit() -> TableStructureEdit {
        let orig_cols = vec![
            ColumnDraft {
                auto_increment: true,
                ..col(Some("id"), "id", "BIGINT")
            },
            ColumnDraft {
                comment: "user email".to_string(),
                ..col(Some("email"), "email", "VARCHAR(120)")
            },
        ];
        let orig_idxs = vec![idx(Some("idx_email"), "idx_email", &["email"], true, false)];
        let orig_fks = vec![ForeignKeyDraft {
            on_update: "CASCADE".to_string(),
            on_delete: "RESTRICT".to_string(),
            ..fk(Some("fk_org"), "fk_org", "org_id", "app", "orgs", "id")
        }];
        TableStructureEdit {
            schema: "app".to_string(),
            table: "users".to_string(),
            original_columns: orig_cols.clone(),
            columns: orig_cols,
            original_indexes: orig_idxs.clone(),
            indexes: orig_idxs,
            original_fks: orig_fks.clone(),
            fks: orig_fks,
        }
    }

    #[test]
    fn no_op_diff_returns_empty_for_both_dialects() {
        let edit = base_edit();
        assert_eq!(
            build_alter_table_statements(SqlDialect::MySql, &edit).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            build_alter_table_statements(SqlDialect::SqlServer, &edit).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn mysql_add_modify_rename_drop_columns_indexes_fks_in_safe_order() {
        let mut edit = base_edit();
        edit.original_columns
            .push(col(Some("org_id"), "org_id", "BIGINT"));
        edit.columns.push(ColumnDraft {
            is_dropped: true,
            ..col(Some("org_id"), "org_id", "BIGINT")
        });
        edit.columns[0].data_type = "BIGINT UNSIGNED".to_string();
        edit.columns[1].name = "primary_email".to_string();
        edit.columns[1].data_type = "VARCHAR(255)".to_string();
        edit.columns[1].comment = "O'Brien's email".to_string();
        edit.columns.push(ColumnDraft {
            default_expr: "CURRENT_TIMESTAMP".to_string(),
            comment: "creation time".to_string(),
            ..col(None, "created_at", "TIMESTAMP")
        });
        edit.indexes[0].is_dropped = true;
        edit.indexes.push(idx(
            None,
            "idx_primary_email",
            &["primary_email"],
            true,
            false,
        ));
        edit.fks[0].is_dropped = true;
        edit.fks.push(ForeignKeyDraft {
            on_update: "CASCADE".to_string(),
            on_delete: "SET NULL".to_string(),
            ..fk(None, "fk_team", "team_id", "app", "teams", "id")
        });

        let stmts = build_alter_table_statements(SqlDialect::MySql, &edit).unwrap();
        assert_eq!(
            stmts,
            vec![
                "ALTER TABLE `app`.`users` DROP FOREIGN KEY `fk_org`",
                "ALTER TABLE `app`.`users` DROP INDEX `idx_email`",
                "ALTER TABLE `app`.`users` DROP COLUMN `org_id`",
                "ALTER TABLE `app`.`users` ADD COLUMN `created_at` TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT 'creation time'",
                "ALTER TABLE `app`.`users` MODIFY COLUMN `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT",
                "ALTER TABLE `app`.`users` CHANGE COLUMN `email` `primary_email` VARCHAR(255) NOT NULL COMMENT 'O''Brien''s email'",
                "ALTER TABLE `app`.`users` ADD UNIQUE INDEX `idx_primary_email` (`primary_email`)",
                "ALTER TABLE `app`.`users` ADD CONSTRAINT `fk_team` FOREIGN KEY (`team_id`) REFERENCES `app`.`teams` (`id`) ON DELETE SET NULL ON UPDATE CASCADE",
            ]
        );
    }

    #[test]
    fn sql_server_add_modify_rename_drop_columns_indexes_fks() {
        let mut edit = TableStructureEdit {
            schema: "shop".to_string(),
            table: "sales.orders".to_string(),
            original_columns: vec![
                ColumnDraft {
                    auto_increment: true,
                    ..col(Some("id"), "id", "INT")
                },
                ColumnDraft {
                    nullable: true,
                    ..col(Some("note"), "note", "NVARCHAR(50)")
                },
                ColumnDraft {
                    nullable: true,
                    ..col(Some("legacy"), "legacy", "INT")
                },
            ],
            columns: vec![
                ColumnDraft {
                    auto_increment: true,
                    ..col(Some("id"), "id", "INT")
                },
                ColumnDraft {
                    default_expr: "N''".to_string(),
                    ..col(Some("note"), "order_note", "NVARCHAR(200)")
                },
                ColumnDraft {
                    nullable: true,
                    is_dropped: true,
                    ..col(Some("legacy"), "legacy", "INT")
                },
                ColumnDraft {
                    auto_increment: true,
                    ..col(None, "seq", "BIGINT")
                },
                ColumnDraft {
                    default_expr: "N'pending'".to_string(),
                    ..col(None, "status", "NVARCHAR(20)")
                },
            ],
            original_indexes: vec![idx(Some("ix_legacy"), "ix_legacy", &["legacy"], false, false)],
            indexes: vec![
                idx(Some("ix_legacy"), "ix_legacy", &["legacy"], false, true),
                idx(
                    None,
                    "ix_status_note",
                    &["status", "order_note"],
                    false,
                    false,
                ),
            ],
            original_fks: vec![ForeignKeyDraft {
                on_delete: "CASCADE".to_string(),
                ..fk(
                    Some("fk_legacy"),
                    "fk_legacy",
                    "legacy",
                    "shop",
                    "sales.customers",
                    "id",
                )
            }],
            fks: vec![
                ForeignKeyDraft {
                    on_delete: "CASCADE".to_string(),
                    is_dropped: true,
                    ..fk(
                        Some("fk_legacy"),
                        "fk_legacy",
                        "legacy",
                        "shop",
                        "sales.customers",
                        "id",
                    )
                },
                ForeignKeyDraft {
                    on_update: "NO ACTION".to_string(),
                    on_delete: "CASCADE".to_string(),
                    ..fk(
                        None,
                        "fk_customer",
                        "customer_id",
                        "",
                        "sales.customers",
                        "id",
                    )
                },
            ],
        };

        let stmts = build_alter_table_statements(SqlDialect::SqlServer, &edit).unwrap();
        assert_eq!(
            stmts,
            vec![
                "ALTER TABLE [shop].[sales].[orders] DROP CONSTRAINT [fk_legacy]",
                "DROP INDEX [ix_legacy] ON [shop].[sales].[orders]",
                "ALTER TABLE [shop].[sales].[orders] DROP COLUMN [legacy]",
                "ALTER TABLE [shop].[sales].[orders] ADD [seq] BIGINT IDENTITY(1,1) NOT NULL",
                "ALTER TABLE [shop].[sales].[orders] ADD [status] NVARCHAR(20) NOT NULL DEFAULT N'pending'",
                "EXEC sp_rename N'[sales].[orders].[note]', N'order_note', N'COLUMN'",
                "ALTER TABLE [shop].[sales].[orders] ALTER COLUMN [order_note] NVARCHAR(200) NOT NULL",
                "ALTER TABLE [shop].[sales].[orders] ADD DEFAULT N'' FOR [order_note]",
                "CREATE INDEX [ix_status_note] ON [shop].[sales].[orders] ([status], [order_note])",
                "ALTER TABLE [shop].[sales].[orders] ADD CONSTRAINT [fk_customer] FOREIGN KEY ([customer_id]) REFERENCES [shop].[sales].[customers] ([id]) ON DELETE CASCADE ON UPDATE NO ACTION",
            ]
        );

        edit.columns.push(ColumnDraft {
            nullable: true,
            is_dropped: true,
            ..col(None, "ignored", "INT")
        });
        assert_eq!(
            build_alter_table_statements(SqlDialect::SqlServer, &edit).unwrap(),
            stmts
        );
    }

    #[test]
    fn sql_server_rename_without_type_change_emits_only_sp_rename() {
        let mut edit = TableStructureEdit {
            schema: "app".to_string(),
            table: "users".to_string(),
            original_columns: vec![ColumnDraft {
                nullable: true,
                ..col(Some("fname"), "fname", "NVARCHAR(80)")
            }],
            columns: vec![ColumnDraft {
                nullable: true,
                ..col(Some("fname"), "first_name", "NVARCHAR(80)")
            }],
            original_indexes: vec![],
            indexes: vec![],
            original_fks: vec![],
            fks: vec![],
        };

        let stmts = build_alter_table_statements(SqlDialect::SqlServer, &edit).unwrap();
        assert_eq!(
            stmts,
            vec!["EXEC sp_rename N'[dbo].[users].[fname]', N'first_name', N'COLUMN'"]
        );

        edit.columns[0].name = "fname".to_string();
        edit.columns[0].nullable = false;
        let stmts = build_alter_table_statements(SqlDialect::SqlServer, &edit).unwrap();
        assert_eq!(
            stmts,
            vec!["ALTER TABLE [app].[dbo].[users] ALTER COLUMN [fname] NVARCHAR(80) NOT NULL"]
        );
    }

    #[test]
    fn modified_index_and_fk_emit_drop_then_add() {
        let mut edit = base_edit();
        edit.indexes[0].unique = false;
        edit.indexes[0].columns.push("id".to_string());
        edit.fks[0].on_delete = "CASCADE".to_string();

        let mysql_stmts = build_alter_table_statements(SqlDialect::MySql, &edit).unwrap();
        assert_eq!(
            mysql_stmts,
            vec![
                "ALTER TABLE `app`.`users` DROP FOREIGN KEY `fk_org`",
                "ALTER TABLE `app`.`users` DROP INDEX `idx_email`",
                "ALTER TABLE `app`.`users` ADD INDEX `idx_email` (`email`, `id`)",
                "ALTER TABLE `app`.`users` ADD CONSTRAINT `fk_org` FOREIGN KEY (`org_id`) REFERENCES `app`.`orgs` (`id`) ON DELETE CASCADE ON UPDATE CASCADE",
            ]
        );

        let mssql_stmts = build_alter_table_statements(SqlDialect::SqlServer, &edit).unwrap();
        assert_eq!(
            mssql_stmts,
            vec![
                "ALTER TABLE [app].[dbo].[users] DROP CONSTRAINT [fk_org]",
                "DROP INDEX [idx_email] ON [app].[dbo].[users]",
                "CREATE INDEX [idx_email] ON [app].[dbo].[users] ([email], [id])",
                "ALTER TABLE [app].[dbo].[users] ADD CONSTRAINT [fk_org] FOREIGN KEY ([org_id]) REFERENCES [app].[dbo].[orgs] ([id]) ON DELETE CASCADE ON UPDATE CASCADE",
            ]
        );
    }

    #[test]
    fn omits_removed_entries_not_present_in_draft_lists() {
        let mut edit = base_edit();
        edit.columns.pop();
        edit.indexes.clear();
        edit.fks.clear();

        let stmts = build_alter_table_statements(SqlDialect::MySql, &edit).unwrap();
        assert_eq!(
            stmts,
            vec![
                "ALTER TABLE `app`.`users` DROP FOREIGN KEY `fk_org`",
                "ALTER TABLE `app`.`users` DROP INDEX `idx_email`",
                "ALTER TABLE `app`.`users` DROP COLUMN `email`",
            ]
        );
    }

    #[test]
    fn validation_rejects_empty_names_and_types_for_active_entries() {
        let mut empty_table = base_edit();
        empty_table.table = "   ".to_string();
        assert!(build_alter_table_statements(SqlDialect::MySql, &empty_table).is_err());

        let mut empty_col_name = base_edit();
        empty_col_name.columns[0].name = " ".to_string();
        assert!(build_alter_table_statements(SqlDialect::MySql, &empty_col_name).is_err());

        let mut empty_col_type = base_edit();
        empty_col_type.columns[0].data_type = "".to_string();
        assert!(build_alter_table_statements(SqlDialect::MySql, &empty_col_type).is_err());

        let mut dropped_empty_col = base_edit();
        dropped_empty_col.columns.push(ColumnDraft {
            nullable: true,
            is_dropped: true,
            ..col(None, "", "")
        });
        assert!(build_alter_table_statements(SqlDialect::MySql, &dropped_empty_col).is_ok());

        let mut empty_idx_name = base_edit();
        empty_idx_name.indexes[0].name = "".to_string();
        assert!(build_alter_table_statements(SqlDialect::MySql, &empty_idx_name).is_err());

        let mut empty_idx_cols = base_edit();
        empty_idx_cols.indexes[0].columns = vec![" ".to_string()];
        assert!(build_alter_table_statements(SqlDialect::MySql, &empty_idx_cols).is_err());

        let mut empty_fk_name = base_edit();
        empty_fk_name.fks[0].name = "".to_string();
        assert!(build_alter_table_statements(SqlDialect::SqlServer, &empty_fk_name).is_err());

        let mut empty_fk_col = base_edit();
        empty_fk_col.fks[0].column = "".to_string();
        assert!(build_alter_table_statements(SqlDialect::SqlServer, &empty_fk_col).is_err());

        let mut empty_fk_ref_table = base_edit();
        empty_fk_ref_table.fks[0].ref_table = "".to_string();
        assert!(build_alter_table_statements(SqlDialect::SqlServer, &empty_fk_ref_table).is_err());

        let mut empty_fk_ref_col = base_edit();
        empty_fk_ref_col.fks[0].ref_column = "".to_string();
        assert!(build_alter_table_statements(SqlDialect::SqlServer, &empty_fk_ref_col).is_err());
    }
}
