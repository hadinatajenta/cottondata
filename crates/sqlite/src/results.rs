use crate::errors::sqlite_error;
use crate::values::{is_binary_declared_type, is_numeric_declared_type, type_label, value_text};
use cottondata_core::UNLIMITED_ROWS;
use cottondata_driver::{DbError, ServerHealthMetrics};
use cottondata_grid::{CellValue, ColumnMeta, ColumnarPage};
use rusqlite::{Connection, Statement};

pub(crate) fn describe_columns(statement: &Statement<'_>) -> Vec<ColumnMeta> {
    statement
        .columns()
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let declared_type = column.decl_type();
            let mut meta = ColumnMeta {
                name: column.name().to_string(),
                org_name: None,
                table: None,
                schema: None,
                type_name: type_label(declared_type),
                is_primary_key: false,
                is_unique_key: false,
                is_nullable: true,
                is_auto_increment: false,
                is_numeric: declared_type.is_some_and(is_numeric_declared_type),
                is_binary: declared_type.is_some_and(is_binary_declared_type),
            };
            if let Ok(Some((database, table, origin, _, _, is_not_null, is_primary_key, is_auto_increment))) =
                statement.column_metadata(index)
            {
                meta.schema = Some(database.to_string_lossy().into_owned());
                meta.table = Some(table.to_string_lossy().into_owned());
                meta.org_name = Some(origin.to_string_lossy().into_owned());
                meta.is_nullable = !is_not_null && !is_primary_key;
                meta.is_primary_key = is_primary_key;
                meta.is_auto_increment = is_auto_increment
                    || (is_primary_key && meta.type_name.eq_ignore_ascii_case("integer"));
            }
            meta
        })
        .collect()
}

pub(crate) fn read_page(mut statement: Statement<'_>, sql: &str, max_rows: u32, must_drain: bool) -> Result<ColumnarPage, DbError> {
    let row_cap = if max_rows == UNLIMITED_ROWS {
        usize::MAX
    } else {
        max_rows.max(1) as usize
    };
    let columns = describe_columns(&statement);
    let column_count = columns.len();
    let mut page = ColumnarPage::new(columns, sql.to_string());
    let mut rows = statement.raw_query();
    let mut row_buffer: Vec<CellValue> = Vec::with_capacity(column_count);
    while let Some(row) = rows.next().map_err(sqlite_error)? {
        if page.row_count >= row_cap {
            page.has_more_rows = true;
            if must_drain {
                continue;
            }
            break;
        }
        row_buffer.clear();
        for index in 0..column_count {
            let value = row.get_ref(index).map_err(sqlite_error)?;
            row_buffer.push(value_text(value).map(CellValue::from_text).unwrap_or(CellValue::Null));
        }
        page.push_row(&row_buffer);
    }
    Ok(page)
}

pub(crate) fn render_query_plan(rows: &[(i64, i64, String)]) -> String {
    fn render(rows: &[(i64, i64, String)], parent: i64, depth: usize, output: &mut Vec<String>) {
        for (id, row_parent, detail) in rows.iter().filter(|(_, row_parent, _)| *row_parent == parent) {
            let _ = row_parent;
            output.push(format!("{}{detail}", "  ".repeat(depth) + if depth > 0 { "└─ " } else { "" }));
            render(rows, *id, depth + 1, output);
        }
    }
    let mut output = Vec::new();
    render(rows, 0, 0, &mut output);
    output.join("\n")
}

pub(crate) fn load_query_plan(connection: &Connection, statement_body: &str) -> Result<Vec<(i64, i64, String)>, DbError> {
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {statement_body}"))
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(3)?)))
        .map_err(sqlite_error)?;
    rows.collect::<Result<_, _>>().map_err(sqlite_error)
}

pub(crate) fn collect_health_metrics(connection: &Connection, path: &str, version: &str) -> ServerHealthMetrics {
    let pragma = |name: &str| -> Option<String> {
        connection
            .query_row(&format!("PRAGMA {name}"), [], |row| {
                row.get_ref(0).map(|value| value_text(value).unwrap_or_default())
            })
            .ok()
    };
    let mut metrics = ServerHealthMetrics::default();
    metrics.push("File", path.to_string());
    if let Ok(file) = std::fs::metadata(path) {
        metrics.push("File size", format!("{:.1} KiB", file.len() as f64 / 1024.0));
    }
    for (label, name) in [
        ("Page size", "page_size"),
        ("Pages", "page_count"),
        ("Free pages", "freelist_count"),
        ("Journal mode", "journal_mode"),
        ("Foreign keys", "foreign_keys"),
    ] {
        if let Some(value) = pragma(name) {
            metrics.push(label, value);
        }
    }
    metrics.push("SQLite version", version.to_string());
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_plan_rows_render_as_an_indented_tree() {
        let plan = render_query_plan(&[
            (2, 0, "SCAN orders".into()),
            (5, 0, "SEARCH customers USING INTEGER PRIMARY KEY (rowid=?)".into()),
            (7, 5, "CORRELATED SCALAR SUBQUERY".into()),
        ]);
        assert_eq!(
            plan,
            "SCAN orders\nSEARCH customers USING INTEGER PRIMARY KEY (rowid=?)\n  └─ CORRELATED SCALAR SUBQUERY"
        );
    }
}
