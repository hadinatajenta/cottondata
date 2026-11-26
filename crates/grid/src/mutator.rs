use crate::page::{escape_ident, CellValue, ColumnarPage};
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

    pub fn revert_all(&mut self) {
        self.modified.clear();
        self.deleted_rows.clear();
        self.inserted_rows.clear();
    }

    pub fn build_pending_sql(
        &self,
        page: &ColumnarPage,
        schema: &str,
        table: &str,
        primary_key_cols: &[String],
    ) -> Result<PendingSqlBatch, String> {
        let fq_table = format!("`{}`.`{}`", escape_ident(schema), escape_ident(table));
        let pk_indices: Vec<usize> = primary_key_cols
            .iter()
            .filter_map(|pk| page.columns.iter().position(|c| c.name.eq_ignore_ascii_case(pk)))
            .collect();

        if (!self.deleted_rows.is_empty() || !self.modified.is_empty()) && pk_indices.is_empty() {
            return Err("Safe inline UPDATE/DELETE requires a Primary Key in the result set.".into());
        }

        let mut statements = Vec::new();
        let mut delete_count = 0;
        let mut update_count = 0;
        let mut insert_count = 0;

        for &row in &self.deleted_rows {
            let where_clause = build_pk_predicate(page, row, &pk_indices)?;
            statements.push(format!("DELETE FROM {fq_table} WHERE {where_clause} LIMIT 1"));
            delete_count += 1;
        }

        let mut updates_by_row: BTreeMap<usize, Vec<(usize, &CellValue)>> = BTreeMap::new();
        for (&(row, col), val) in &self.modified {
            if !self.deleted_rows.contains(&row) {
                updates_by_row.entry(row).or_default().push((col, val));
            }
        }

        for (row, changes) in updates_by_row {
            let set_clause = changes
                .iter()
                .map(|(col, val)| {
                    let meta = &page.columns[*col];
                    format!(
                        "`{}` = {}",
                        escape_ident(&meta.name),
                        format_sql_literal(val, meta.is_numeric)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let where_clause = build_pk_predicate(page, row, &pk_indices)?;
            statements.push(format!(
                "UPDATE {fq_table} SET {set_clause} WHERE {where_clause} LIMIT 1"
            ));
            update_count += 1;
        }

        for inserted in &self.inserted_rows {
            let mut cols = Vec::new();
            let mut vals = Vec::new();
            for (idx, meta) in page.columns.iter().enumerate() {
                let val = inserted.get(idx).unwrap_or(&CellValue::Null);
                if meta.is_auto_increment && val.is_null() {
                    continue;
                }
                cols.push(format!("`{}`", escape_ident(&meta.name)));
                vals.push(format_sql_literal(val, meta.is_numeric));
            }
            if cols.is_empty() {
                statements.push(format!("INSERT INTO {fq_table} () VALUES ()"));
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

fn build_pk_predicate(page: &ColumnarPage, row: usize, pk_indices: &[usize]) -> Result<String, String> {
    let mut parts = Vec::with_capacity(pk_indices.len());
    for &idx in pk_indices {
        let meta = &page.columns[idx];
        let val = page.cell_value(row, idx);
        if val.is_null() {
            parts.push(format!("`{}` IS NULL", escape_ident(&meta.name)));
        } else {
            parts.push(format!(
                "`{}` = {}",
                escape_ident(&meta.name),
                format_sql_literal(&val, meta.is_numeric)
            ));
        }
    }
    Ok(parts.join(" AND "))
}

pub fn format_sql_literal(val: &CellValue, is_numeric: bool) -> String {
    match val {
        CellValue::Null => "NULL".to_string(),
        CellValue::Text(s) | CellValue::Truncated { preview: s, .. } => {
            if is_numeric && is_plain_number(s) {
                s.clone()
            } else {
                let escaped = s.replace('\\', "\\\\").replace('\'', "''");
                format!("'{escaped}'")
            }
        }
    }
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
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "int".into(),
                is_primary_key: true,
                is_nullable: false,
                is_auto_increment: true,
                is_numeric: true,
            },
            ColumnMeta {
                name: "name".into(),
                table: Some("users".into()),
                schema: Some("shop".into()),
                type_name: "varchar(100)".into(),
                is_primary_key: false,
                is_nullable: false,
                is_auto_increment: false,
                is_numeric: false,
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
            .build_pending_sql(&page, "shop", "users", &["id".into()])
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
}
