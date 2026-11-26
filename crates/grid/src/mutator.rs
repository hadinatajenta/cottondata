use crate::page::{CellValue, ColumnarPage};
use cottondata_core::SqlDialect;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationKind {
    Insert,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSqlBatch {
    pub statements: Vec<String>,
    pub delete_count: usize,
    pub update_count: usize,
    pub insert_count: usize,
}

impl PendingSqlBatch {
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    pub fn as_script(&self) -> String {
        if self.statements.is_empty() {
            return String::new();
        }
        let mut out = String::from("START TRANSACTION;\n");
        for stmt in &self.statements {
            out.push_str(stmt);
            out.push_str(";\n");
        }
        out.push_str("COMMIT;");
        out
    }
}

#[derive(Debug, Clone, Default)]
pub struct StagedMutator {
    modified: BTreeMap<(usize, usize), CellValue>,
    deleted_rows: BTreeSet<usize>,
    inserted_rows: Vec<Vec<CellValue>>,
}

impl StagedMutator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_changes(&self) -> bool {
        !self.modified.is_empty() || !self.deleted_rows.is_empty() || !self.inserted_rows.is_empty()
    }

    pub fn pending_summary(&self) -> (usize, usize, usize) {
        let modified_rows: BTreeSet<usize> = self
            .modified
            .keys()
            .map(|(r, _)| *r)
            .filter(|r| !self.deleted_rows.contains(r))
            .collect();
        (self.deleted_rows.len(), modified_rows.len(), self.inserted_rows.len())
    }

    pub fn set_cell(&mut self, page: &ColumnarPage, row: usize, col: usize, new_val: CellValue) {
        let original = page.cell_value(row, col);
        if original == new_val {
            self.modified.remove(&(row, col));
        } else {
            self.deleted_rows.remove(&row);
            self.modified.insert((row, col), new_val);
        }
    }

    pub fn get_modified(&self, row: usize, col: usize) -> Option<&CellValue> {
        self.modified.get(&(row, col))
    }

    pub fn is_deleted(&self, row: usize) -> bool {
        self.deleted_rows.contains(&row)
    }

    pub fn toggle_delete_row(&mut self, row: usize) {
        if !self.deleted_rows.remove(&row) {
            self.deleted_rows.insert(row);
        }
    }

    pub fn insert_blank_row(&mut self, page: &ColumnarPage) -> usize {
        let row = vec![CellValue::Null; page.columns.len()];
        self.inserted_rows.push(row);
        self.inserted_rows.len() - 1
    }

    pub fn clone_row(&mut self, page: &ColumnarPage, source_row: usize) -> usize {
        let row = page
            .columns
            .iter()
            .enumerate()
            .map(|(col_idx, meta)| {
                if meta.is_primary_key || meta.is_auto_increment {
                    CellValue::Null
                } else {
                    self.modified
                        .get(&(source_row, col_idx))
                        .cloned()
                        .unwrap_or_else(|| page.cell_value(source_row, col_idx))
                }
            })
            .collect();
        self.inserted_rows.push(row);
        self.inserted_rows.len() - 1
    }

    pub fn inserted_rows(&self) -> &[Vec<CellValue>] {
        &self.inserted_rows
    }

    pub fn set_inserted_cell(&mut self, inserted_idx: usize, col: usize, val: CellValue) {
        if let Some(row) = self.inserted_rows.get_mut(inserted_idx) {
            if let Some(cell) = row.get_mut(col) {
                *cell = val;
            }
        }
    }

    pub fn remove_inserted_row(&mut self, inserted_idx: usize) {
        if inserted_idx < self.inserted_rows.len() {
            self.inserted_rows.remove(inserted_idx);
        }
    }

    pub fn revert_cell(&mut self, row: usize, col: usize) {
        self.modified.remove(&(row, col));
    }

    pub fn has_row_modifications(&self, row: usize) -> bool {
        self.modified.keys().any(|(r, _)| *r == row)
    }

    pub fn revert_row(&mut self, row: usize) {
        self.deleted_rows.remove(&row);
        self.modified.retain(|(r, _), _| *r != row);
    }

    pub fn revert_all(&mut self) {
        self.modified.clear();
        self.deleted_rows.clear();
        self.inserted_rows.clear();
    }

    pub fn build_pending_sql(
        &self,
        dialect: SqlDialect,
        page: &ColumnarPage,
        schema: &str,
        table: &str,
        primary_key_cols: &[String],
    ) -> Result<PendingSqlBatch, String> {
        let fq_table = dialect.qualified_table(schema, table);
        let pk_indices = resolve_key_indices(page, primary_key_cols);

        if (!self.deleted_rows.is_empty() || !self.modified.is_empty())
            && (pk_indices.is_empty() || pk_indices.len() != primary_key_cols.len())
        {
            return Err("Safe inline UPDATE/DELETE requires all Primary Key / Unique Key columns in the result set.".into());
        }

        let mut statements = Vec::new();
        let mut delete_count = 0;
        let mut update_count = 0;
        let mut insert_count = 0;

        for &row in &self.deleted_rows {
            let where_clause = build_pk_predicate(dialect, page, row, &pk_indices)?;
            statements.push(dialect.delete_single_row(&fq_table, &where_clause));
            delete_count += 1;
        }

        let mut updates_by_row: BTreeMap<usize, Vec<(usize, &CellValue)>> = BTreeMap::new();
        for (&(row, col), val) in &self.modified {
            if !self.deleted_rows.contains(&row) {
                updates_by_row.entry(row).or_default().push((col, val));
            }
        }

        for (row, changes) in updates_by_row {
            let mut assignments = Vec::with_capacity(changes.len());
            for (col, val) in changes {
                let meta = &page.columns[col];
                if val.is_truncated() {
                    return Err(format!(
                        "Column `{}` holds a truncated preview (>4 KiB). Load the full value before saving.",
                        meta.sql_column_name()
                    ));
                }
                if val.is_null() && !meta.is_nullable {
                    return Err(format!(
                        "Column `{}` does not allow NULL values.",
                        meta.sql_column_name()
                    ));
                }
                assignments.push(format!(
                    "{} = {}",
                    dialect.quote_ident(meta.sql_column_name()),
                    format_sql_literal_for_meta(dialect, val, meta)
                ));
            }
            let set_clause = assignments.join(", ");
            let where_clause = build_pk_predicate(dialect, page, row, &pk_indices)?;
            statements.push(dialect.update_single_row(&fq_table, &set_clause, &where_clause));
            update_count += 1;
        }

        for inserted in &self.inserted_rows {
            let mut cols = Vec::new();
            let mut vals = Vec::new();
            for (idx, meta) in page.columns.iter().enumerate() {
                if meta.is_computed() {
                    continue;
                }
                let val = inserted.get(idx).unwrap_or(&CellValue::Null);
                if val.is_truncated() {
                    return Err(format!(
                        "Column `{}` holds a truncated preview (>4 KiB) and cannot be inserted.",
                        meta.sql_column_name()
                    ));
                }
                if (meta.is_auto_increment && val.is_null()) || val.is_default() {
                    continue;
                }
                cols.push(dialect.quote_ident(meta.sql_column_name()));
                vals.push(format_sql_literal_for_meta(dialect, val, meta));
            }
            if cols.is_empty() {
                statements.push(dialect.insert_default_values(&fq_table));
            } else {
                statements.push(format!(
                    "INSERT INTO {fq_table} ({}) VALUES ({})",
                    cols.join(", "),
                    vals.join(", ")
                ));
            }
            insert_count += 1;
        }

        Ok(PendingSqlBatch {
            statements,
            delete_count,
            update_count,
            insert_count,
        })
    }
}

pub fn build_row_key_predicate(
    dialect: SqlDialect,
    page: &ColumnarPage,
    row: usize,
    key_columns: &[String],
) -> Result<String, String> {
    let pk_indices = resolve_key_indices(page, key_columns);
    if pk_indices.is_empty() || pk_indices.len() != key_columns.len() {
        return Err("Missing key columns in result set.".into());
    }
    build_pk_predicate(dialect, page, row, &pk_indices)
}

fn resolve_key_indices(page: &ColumnarPage, key_columns: &[String]) -> Vec<usize> {
    key_columns
        .iter()
        .filter_map(|pk| {
            page.columns
                .iter()
                .position(|c| c.sql_column_name().eq_ignore_ascii_case(pk))
        })
        .collect()
}

fn build_pk_predicate(
    dialect: SqlDialect,
    page: &ColumnarPage,
    row: usize,
    pk_indices: &[usize],
) -> Result<String, String> {
    let mut parts = Vec::with_capacity(pk_indices.len());
    for &idx in pk_indices {
        let meta = &page.columns[idx];
        let val = page.cell_value(row, idx);
        let col_ident = dialect.quote_ident(meta.sql_column_name());
        if val.is_null() {
            parts.push(format!("{col_ident} IS NULL"));
        } else {
            parts.push(format!(
                "{col_ident} = {}",
                format_sql_literal_for_meta(dialect, &val, meta)
            ));
        }
    }
    Ok(parts.join(" AND "))
}

pub fn format_sql_literal(dialect: SqlDialect, val: &CellValue, is_numeric: bool) -> String {
    match val {
        CellValue::Null => "NULL".to_string(),
        CellValue::DefaultExpr => "DEFAULT".to_string(),
        CellValue::Text(s) | CellValue::Truncated { preview: s, .. } => {
            format_sql_text_literal(dialect, s, is_numeric)
        }
    }
}

pub(crate) fn format_sql_text_literal(dialect: SqlDialect, s: &str, is_numeric: bool) -> String {
    if is_numeric && is_plain_number(s) {
        s.trim().to_string()
    } else {
        dialect.string_literal(s)
    }
}

fn format_sql_literal_for_meta(dialect: SqlDialect, val: &CellValue, meta: &crate::page::ColumnMeta) -> String {
    match val {
        CellValue::Null => "NULL".to_string(),
        CellValue::DefaultExpr => "DEFAULT".to_string(),
        CellValue::Text(s) | CellValue::Truncated { preview: s, .. } => {
            let trimmed = s.trim();
            if dialect == SqlDialect::MySql
                && meta.type_name.eq_ignore_ascii_case("bit")
                && is_mysql_bit_literal(trimmed)
            {
                return trimmed.to_string();
            }
            if (meta.is_binary || is_binary_type_name(&meta.type_name)) && is_hex_literal(trimmed) {
                return trimmed.to_string();
            }
            if meta.is_numeric && is_plain_number(trimmed) {
                return trimmed.to_string();
            }
            dialect.string_literal(s)
        }
    }
}

fn is_mysql_bit_literal(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 4
        && (bytes[0] == b'b' || bytes[0] == b'B')
        && bytes[1] == b'\''
        && bytes[bytes.len() - 1] == b'\''
        && bytes[2..bytes.len() - 1].iter().all(|&b| b == b'0' || b == b'1')
}

fn is_hex_literal(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 4
        && bytes[0] == b'0'
        && (bytes[1] == b'x' || bytes[1] == b'X')
        && bytes[2..].len() % 2 == 0
        && bytes[2..].iter().all(|b| b.is_ascii_hexdigit())
}

fn is_binary_type_name(type_name: &str) -> bool {
    let lower = type_name.to_ascii_lowercase();
    lower.contains("blob") || lower.contains("binary") || lower.contains("geometry")
}

fn is_plain_number(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let start = if bytes[0] == b'-' { 1 } else { 0 };
    if start >= bytes.len() {
        return false;
    }
    let mut seen_dot = false;
    for &b in &bytes[start..] {
        if b == b'.' && !seen_dot {
            seen_dot = true;
        } else if !b.is_ascii_digit() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::ColumnMeta;

    #[test]
    fn generates_ordered_delete_update_insert_transaction_sql() {
        let cols = vec![
            ColumnMeta {
                name: "id".into(),
                org_name: Some("id".into()),
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "int".into(),
                is_primary_key: true,
                is_unique_key: false,
                is_nullable: false,
                is_auto_increment: true,
                is_numeric: true,
                is_binary: false,
            },
            ColumnMeta {
                name: "name".into(),
                org_name: Some("name".into()),
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "varchar(100)".into(),
                is_primary_key: false,
                is_unique_key: false,
                is_nullable: false,
                is_auto_increment: false,
                is_numeric: false,
                is_binary: false,
            },
        ];
        let mut page = ColumnarPage::new(cols, "SELECT * FROM users".into());
        page.push_row(&[CellValue::Text("1".into()), CellValue::Text("Alice".into())]);
        page.push_row(&[CellValue::Text("2".into()), CellValue::Text("Bob".into())]);

        let mut mutator = StagedMutator::new();
        mutator.toggle_delete_row(0);
        mutator.set_cell(&page, 1, 1, CellValue::Text("Bob O'Neil".into()));
        let ins = mutator.insert_blank_row(&page);
        mutator.set_inserted_cell(ins, 1, CellValue::Text("Charlie".into()));

        let batch = mutator
            .build_pending_sql(SqlDialect::MySql, &page, "shop", "users", &["id".into()])
            .unwrap();
        assert_eq!(batch.delete_count, 1);
        assert_eq!(batch.update_count, 1);
        assert_eq!(batch.insert_count, 1);
        assert_eq!(batch.statements[0], "DELETE FROM `shop`.`users` WHERE `id` = 1 LIMIT 1");
        assert_eq!(
            batch.statements[1],
            "UPDATE `shop`.`users` SET `name` = 'Bob O''Neil' WHERE `id` = 2 LIMIT 1"
        );
        assert_eq!(
            batch.statements[2],
            "INSERT INTO `shop`.`users` (`name`) VALUES ('Charlie')"
        );
    }

    #[test]
    fn sql_server_batch_uses_brackets_top_and_unicode_literals_without_backslash_escaping() {
        let cols = vec![
            ColumnMeta {
                name: "id".into(),
                org_name: Some("id".into()),
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "int".into(),
                is_primary_key: true,
                is_unique_key: false,
                is_nullable: false,
                is_auto_increment: true,
                is_numeric: true,
                is_binary: false,
            },
            ColumnMeta {
                name: "path".into(),
                org_name: Some("path".into()),
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "nvarchar(100)".into(),
                is_primary_key: false,
                is_unique_key: false,
                is_nullable: true,
                is_auto_increment: false,
                is_numeric: false,
                is_binary: false,
            },
        ];
        let mut page = ColumnarPage::new(cols, "SELECT 1".into());
        page.push_row(&[CellValue::Text("7".into()), CellValue::Text("old".into())]);

        let mut mutator = StagedMutator::new();
        mutator.set_cell(&page, 0, 1, CellValue::Text("C:\\temp\\it's".into()));
        mutator.insert_blank_row(&page);

        let batch = mutator
            .build_pending_sql(SqlDialect::SqlServer, &page, "shop", "users", &["id".into()])
            .unwrap();
        assert_eq!(
            batch.statements[0],
            "UPDATE TOP (1) [shop].[dbo].[users] SET [path] = N'C:\\temp\\it''s' WHERE [id] = 7"
        );
        assert_eq!(batch.statements[1], "INSERT INTO [shop].[dbo].[users] ([path]) VALUES (NULL)");
    }

    #[test]
    fn uses_original_column_names_default_expr_bit_hex_and_rejects_not_null_violation() {
        let cols = vec![
            ColumnMeta {
                name: "user_id".into(),
                org_name: Some("id".into()),
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "int".into(),
                is_primary_key: true,
                is_unique_key: false,
                is_nullable: false,
                is_auto_increment: true,
                is_numeric: true,
                is_binary: false,
            },
            ColumnMeta {
                name: "display_name".into(),
                org_name: Some("name".into()),
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "varchar(64)".into(),
                is_primary_key: false,
                is_unique_key: false,
                is_nullable: false,
                is_auto_increment: false,
                is_numeric: false,
                is_binary: false,
            },
            ColumnMeta {
                name: "flags".into(),
                org_name: Some("flags".into()),
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "bit".into(),
                is_primary_key: false,
                is_unique_key: false,
                is_nullable: true,
                is_auto_increment: false,
                is_numeric: false,
                is_binary: false,
            },
            ColumnMeta {
                name: "digest".into(),
                org_name: Some("digest".into()),
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "blob".into(),
                is_primary_key: false,
                is_unique_key: false,
                is_nullable: true,
                is_auto_increment: false,
                is_numeric: false,
                is_binary: true,
            },
        ];
        let mut page = ColumnarPage::new(cols, "SELECT id AS user_id, name AS display_name, flags, digest FROM users".into());
        page.push_row(&[
            CellValue::Text("10".into()),
            CellValue::Text("Old".into()),
            CellValue::Text("b'00000001'".into()),
            CellValue::Text("0x00FF".into()),
        ]);

        let mut mutator = StagedMutator::new();
        mutator.set_cell(&page, 0, 1, CellValue::DefaultExpr);
        mutator.set_cell(&page, 0, 2, CellValue::Text("b'00000010'".into()));
        mutator.set_cell(&page, 0, 3, CellValue::Text("0xCAFE".into()));
        let batch = mutator
            .build_pending_sql(SqlDialect::MySql, &page, "shop", "users", &["id".into()])
            .unwrap();
        assert_eq!(
            batch.statements[0],
            "UPDATE `shop`.`users` SET `name` = DEFAULT, `flags` = b'00000010', `digest` = 0xCAFE WHERE `id` = 10 LIMIT 1"
        );

        mutator.set_cell(&page, 0, 1, CellValue::Null);
        assert!(mutator
            .build_pending_sql(SqlDialect::MySql, &page, "shop", "users", &["id".into()])
            .is_err());

        mutator.revert_row(0);
        assert!(!mutator.has_changes());
    }
}
