use crate::mutator::build_row_key_predicate;
use crate::page::ColumnarPage;
use cottondata_core::SqlDialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKeyKind {
    PrimaryKey,
    UniqueNotNull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyReason {
    ServerReadOnly,
    NoBaseTable,
    MultiTableQuery,
    ViewNotEditable,
    MissingRowKey,
}

impl ReadOnlyReason {
    pub fn message(self) -> &'static str {
        match self {
            Self::ServerReadOnly => "Server is configured as Read-Only",
            Self::NoBaseTable => "Result set is not backed by a table",
            Self::MultiTableQuery => "Multi-table JOIN results are read-only",
            Self::ViewNotEditable => "Views are read-only in grid editor",
            Self::MissingRowKey => "No Primary Key or Unique NOT NULL key in result set",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableTarget {
    pub schema: String,
    pub table: String,
    pub key_columns: Vec<String>,
    pub key_kind: RowKeyKind,
    pub editable_columns: Vec<bool>,
}

impl EditableTarget {
    pub fn is_column_editable(&self, col_idx: usize) -> bool {
        self.editable_columns.get(col_idx).copied().unwrap_or(false)
    }

    pub fn build_key_where(&self, dialect: SqlDialect, page: &ColumnarPage, row: usize) -> Result<String, String> {
        build_row_key_predicate(dialect, page, row, &self.key_columns)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Editability {
    Editable(EditableTarget),
    ReadOnly(ReadOnlyReason),
}

impl Editability {
    pub fn is_editable(&self) -> bool {
        matches!(self, Self::Editable(_))
    }

    pub fn target(&self) -> Option<&EditableTarget> {
        match self {
            Self::Editable(target) => Some(target),
            Self::ReadOnly(_) => None,
        }
    }

    pub fn reason(&self) -> Option<ReadOnlyReason> {
        match self {
            Self::Editable(_) => None,
            Self::ReadOnly(reason) => Some(*reason),
        }
    }

    pub fn is_column_editable(&self, col_idx: usize) -> bool {
        self.target()
            .is_some_and(|target| target.is_column_editable(col_idx))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TableEditMetadata<'a> {
    pub is_view: bool,
    pub primary_keys: &'a [String],
    pub unique_key: &'a [String],
}

pub fn infer_result_table(
    page: &ColumnarPage,
    bound_table: Option<(&str, &str)>,
    fallback_schema: Option<&str>,
) -> Result<(String, String), ReadOnlyReason> {
    if let Some((schema, table)) = bound_table {
        return Ok((schema.to_string(), table.to_string()));
    }

    let mut found: Option<(String, String)> = None;
    for col in &page.columns {
        let Some(table) = col.table.as_deref().filter(|t| !t.is_empty()) else {
            continue;
        };
        let schema = col
            .schema
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(fallback_schema)
            .unwrap_or("");
        if schema.is_empty() {
            continue;
        }
        match &found {
            None => found = Some((schema.to_string(), table.to_string())),
            Some((prev_s, prev_t)) => {
                if !prev_s.eq_ignore_ascii_case(schema) || !prev_t.eq_ignore_ascii_case(table) {
                    return Err(ReadOnlyReason::MultiTableQuery);
                }
            }
        }
    }

    found.ok_or(ReadOnlyReason::NoBaseTable)
}

pub fn resolve_editability(
    page: &ColumnarPage,
    bound_table: Option<(&str, &str)>,
    fallback_schema: Option<&str>,
    server_read_only: bool,
    catalog_meta: Option<TableEditMetadata<'_>>,
) -> Editability {
    if server_read_only {
        return Editability::ReadOnly(ReadOnlyReason::ServerReadOnly);
    }

    let (schema, table) = match infer_result_table(page, bound_table, fallback_schema) {
        Ok(pair) => pair,
        Err(reason) => return Editability::ReadOnly(reason),
    };

    if catalog_meta.is_some_and(|meta| meta.is_view) {
        return Editability::ReadOnly(ReadOnlyReason::ViewNotEditable);
    }

    let is_bound = bound_table.is_some();
    let column_present = |col_name: &str| {
        page.columns
            .iter()
            .any(|c| c.sql_column_name().eq_ignore_ascii_case(col_name))
    };

    let mut resolved_key: Option<(Vec<String>, RowKeyKind)> = None;

    if let Some(meta) = catalog_meta {
        if !meta.primary_keys.is_empty() && meta.primary_keys.iter().all(|pk| column_present(pk)) {
            resolved_key = Some((meta.primary_keys.to_vec(), RowKeyKind::PrimaryKey));
        } else if !meta.unique_key.is_empty() && meta.unique_key.iter().all(|uk| column_present(uk)) {
            resolved_key = Some((meta.unique_key.to_vec(), RowKeyKind::UniqueNotNull));
        }
    }

    if resolved_key.is_none() {
        let pk_from_wire: Vec<String> = page
            .columns
            .iter()
            .filter(|c| c.is_primary_key && !c.is_computed())
            .map(|c| c.sql_column_name().to_string())
            .collect();
        if !pk_from_wire.is_empty() {
            resolved_key = Some((pk_from_wire, RowKeyKind::PrimaryKey));
        } else {
            let uk_from_wire: Vec<String> = page
                .columns
                .iter()
                .filter(|c| c.is_unique_key && !c.is_nullable && !c.is_computed())
                .map(|c| c.sql_column_name().to_string())
                .collect();
            if !uk_from_wire.is_empty() {
                resolved_key = Some((vec![uk_from_wire[0].clone()], RowKeyKind::UniqueNotNull));
            }
        }
    }

    let Some((key_columns, key_kind)) = resolved_key else {
        return Editability::ReadOnly(ReadOnlyReason::MissingRowKey);
    };

    let editable_columns = page
        .columns
        .iter()
        .map(|col| {
            if is_bound {
                !col.is_computed()
            } else {
                col.table.as_deref().is_some_and(|t| !t.is_empty())
                    && col.org_name.as_deref().is_some_and(|n| !n.is_empty())
            }
        })
        .collect();

    Editability::Editable(EditableTarget {
        schema,
        table,
        key_columns,
        key_kind,
        editable_columns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::ColumnMeta;

    fn col(name: &str, org_name: Option<&str>, table: Option<&str>, pk: bool) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            org_name: org_name.map(Into::into),
            table: table.map(Into::into),
            schema: table.map(|_| "shop".into()),
            type_name: "varchar(64)".into(),
            is_primary_key: pk,
            is_unique_key: false,
            is_nullable: !pk,
            is_auto_increment: pk,
            is_numeric: pk,
            is_binary: false,
        }
    }

    #[test]
    fn resolves_console_select_with_alias_and_computed_column() {
        let page = ColumnarPage::new(
            vec![
                col("id", Some("id"), Some("users"), true),
                col("full_name", Some("name"), Some("users"), false),
                col("upper_name", None, None, false),
            ],
            "SELECT id, name AS full_name, UPPER(name) AS upper_name FROM users".into(),
        );
        let edit = resolve_editability(&page, None, Some("shop"), false, None);
        let target = edit.target().expect("console select should be editable");
        assert_eq!(target.schema, "shop");
        assert_eq!(target.table, "users");
        assert_eq!(target.key_columns, vec!["id".to_string()]);
        assert_eq!(target.editable_columns, vec![true, true, false]);
    }

    #[test]
    fn rejects_multi_table_join_and_missing_key() {
        let join_page = ColumnarPage::new(
            vec![
                col("id", Some("id"), Some("orders"), true),
                col("email", Some("email"), Some("users"), false),
            ],
            "SELECT o.id, u.email FROM orders o JOIN users u ON o.user_id = u.id".into(),
        );
        assert_eq!(
            resolve_editability(&join_page, None, Some("shop"), false, None),
            Editability::ReadOnly(ReadOnlyReason::MultiTableQuery)
        );

        let no_pk_page = ColumnarPage::new(
            vec![col("name", Some("name"), Some("users"), false)],
            "SELECT name FROM users".into(),
        );
        assert_eq!(
            resolve_editability(&no_pk_page, None, Some("shop"), false, None),
            Editability::ReadOnly(ReadOnlyReason::MissingRowKey)
        );
    }

    #[test]
    fn falls_back_to_unique_not_null_key() {
        let mut email_col = col("email", Some("email"), Some("users"), false);
        email_col.is_unique_key = true;
        email_col.is_nullable = false;
        let page = ColumnarPage::new(
            vec![email_col, col("name", Some("name"), Some("users"), false)],
            "SELECT email, name FROM users".into(),
        );
        let edit = resolve_editability(&page, None, Some("shop"), false, None);
        let target = edit.target().expect("unique not null key should allow editing");
        assert_eq!(target.key_kind, RowKeyKind::UniqueNotNull);
        assert_eq!(target.key_columns, vec!["email".to_string()]);
    }
}
