use crate::mutator::{format_sql_literal, format_sql_text_literal};
use crate::page::{ColumnMeta, ColumnarPage};
use cottondata_core::SqlDialect;
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Tsv,
    JsonLines,
    JsonArray,
    SqlInserts,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 5] = [Self::Csv, Self::Tsv, Self::JsonLines, Self::JsonArray, Self::SqlInserts];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::JsonLines => "jsonl",
            Self::JsonArray => "json",
            Self::SqlInserts => "sql",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV (.csv)",
            Self::Tsv => "TSV (.tsv)",
            Self::JsonLines => "JSON Lines (.jsonl)",
            Self::JsonArray => "JSON Array (.json)",
            Self::SqlInserts => "SQL INSERT (.sql)",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Tsv => "TSV",
            Self::JsonLines => "JSON Lines",
            Self::JsonArray => "JSON",
            Self::SqlInserts => "SQL INSERT",
        }
    }
}

pub struct SqlInsertTarget<'a> {
    pub dialect: SqlDialect,
    pub table_name: &'a str,
}

impl SqlInsertTarget<'_> {
    pub fn mysql(table_name: &str) -> SqlInsertTarget<'_> {
        SqlInsertTarget {
            dialect: SqlDialect::MySql,
            table_name,
        }
    }
}

pub fn export_rows<W: Write>(
    page: &ColumnarPage,
    rows: &[usize],
    format: ExportFormat,
    target: &SqlInsertTarget,
    writer: &mut W,
) -> io::Result<()> {
    let rows: Vec<usize> = rows.iter().copied().filter(|&row| row < page.row_count).collect();
    match format {
        ExportFormat::Csv => write_delimited(page, &rows, b',', writer),
        ExportFormat::Tsv => write_delimited(page, &rows, b'\t', writer),
        ExportFormat::JsonLines => write_json_lines(page, &rows, writer),
        ExportFormat::JsonArray => write_json_array(page, &rows, writer),
        ExportFormat::SqlInserts => write_sql_inserts(page, &rows, target, writer),
    }
}

pub fn export_csv<W: Write>(page: &ColumnarPage, writer: &mut W) -> io::Result<()> {
    export_rows(page, &all_rows(page), ExportFormat::Csv, &SqlInsertTarget::mysql(""), writer)
}

pub fn export_json_lines<W: Write>(page: &ColumnarPage, writer: &mut W) -> io::Result<()> {
    export_rows(page, &all_rows(page), ExportFormat::JsonLines, &SqlInsertTarget::mysql(""), writer)
}

pub fn export_json_array<W: Write>(page: &ColumnarPage, writer: &mut W) -> io::Result<()> {
    export_rows(page, &all_rows(page), ExportFormat::JsonArray, &SqlInsertTarget::mysql(""), writer)
}

pub fn export_sql_inserts<W: Write>(page: &ColumnarPage, table_name: &str, writer: &mut W) -> io::Result<()> {
    export_rows(page, &all_rows(page), ExportFormat::SqlInserts, &SqlInsertTarget::mysql(table_name), writer)
}

pub struct StreamExporter<W: Write> {
    writer: W,
    format: ExportFormat,
    columns: Vec<ColumnMeta>,
    dialect: SqlDialect,
    quoted_table: String,
    quoted_columns: String,
    rows_written: u64,
}

impl<W: Write> StreamExporter<W> {
    pub fn new(
        mut writer: W,
        format: ExportFormat,
        columns: Vec<ColumnMeta>,
        dialect: SqlDialect,
        table_name: String,
    ) -> io::Result<Self> {
        match format {
            ExportFormat::Csv => write_delimited_header(&columns, b',', &mut writer)?,
            ExportFormat::Tsv => write_delimited_header(&columns, b'\t', &mut writer)?,
            ExportFormat::JsonArray => writer.write_all(b"[\n")?,
            ExportFormat::JsonLines | ExportFormat::SqlInserts => {}
        }

        let (quoted_table, quoted_columns) = if format == ExportFormat::SqlInserts {
            let quoted_table = dialect.quote_ident(&table_name);
            let quoted_columns = columns
                .iter()
                .map(|column| dialect.quote_ident(&column.name))
                .collect::<Vec<_>>()
                .join(", ");
            (quoted_table, quoted_columns)
        } else {
            (String::new(), String::new())
        };

        Ok(Self {
            writer,
            format,
            columns,
            dialect,
            quoted_table,
            quoted_columns,
            rows_written: 0,
        })
    }

    pub fn write_row(&mut self, values: &[Option<String>]) -> io::Result<()> {
        match self.format {
            ExportFormat::Csv => write_delimited_values_row(&self.columns, values, b',', &mut self.writer)?,
            ExportFormat::Tsv => write_delimited_values_row(&self.columns, values, b'\t', &mut self.writer)?,
            ExportFormat::JsonLines => {
                write_json_values_row(&self.columns, values, &mut self.writer)?;
                self.writer.write_all(b"\n")?;
            }
            ExportFormat::JsonArray => {
                if self.rows_written > 0 {
                    self.writer.write_all(b",\n")?;
                }
                self.writer.write_all(b"  ")?;
                write_json_values_row(&self.columns, values, &mut self.writer)?;
            }
            ExportFormat::SqlInserts => {
                if !self.columns.is_empty() {
                    write!(
                        self.writer,
                        "INSERT INTO {} ({}) VALUES (",
                        self.quoted_table, self.quoted_columns
                    )?;
                    for (column_index, meta) in self.columns.iter().enumerate() {
                        if column_index > 0 {
                            self.writer.write_all(b", ")?;
                        }
                        match values.get(column_index).and_then(Option::as_deref) {
                            Some(text) => {
                                let literal = format_sql_text_literal(self.dialect, text, meta.is_numeric);
                                self.writer.write_all(literal.as_bytes())?;
                            }
                            None => self.writer.write_all(b"NULL")?,
                        }
                    }
                    self.writer.write_all(b");\n")?;
                }
            }
        }
        self.rows_written += 1;
        Ok(())
    }

    pub fn rows_written(&self) -> u64 {
        self.rows_written
    }

    pub fn finish(mut self) -> io::Result<u64> {
        if self.format == ExportFormat::JsonArray {
            if self.rows_written > 0 {
                self.writer.write_all(b"\n]\n")?;
            } else {
                self.writer.write_all(b"]\n")?;
            }
        }
        self.writer.flush()?;
        Ok(self.rows_written)
    }
}

fn all_rows(page: &ColumnarPage) -> Vec<usize> {
    (0..page.row_count).collect()
}

fn write_delimited_header<W: Write>(columns: &[ColumnMeta], delimiter: u8, writer: &mut W) -> io::Result<()> {
    for (column_index, column) in columns.iter().enumerate() {
        if column_index > 0 {
            writer.write_all(&[delimiter])?;
        }
        write_delimited_field(writer, &column.name, delimiter)?;
    }
    writer.write_all(b"\n")
}

fn write_delimited_values_row<W: Write>(
    columns: &[ColumnMeta],
    values: &[Option<String>],
    delimiter: u8,
    writer: &mut W,
) -> io::Result<()> {
    for column_index in 0..columns.len() {
        if column_index > 0 {
            writer.write_all(&[delimiter])?;
        }
        if let Some(value) = values.get(column_index).and_then(Option::as_deref) {
            write_delimited_field(writer, value, delimiter)?;
        }
    }
    writer.write_all(b"\n")
}

fn write_delimited<W: Write>(page: &ColumnarPage, rows: &[usize], delimiter: u8, writer: &mut W) -> io::Result<()> {
    write_delimited_header(&page.columns, delimiter, writer)?;

    for &row in rows {
        for column_index in 0..page.columns.len() {
            if column_index > 0 {
                writer.write_all(&[delimiter])?;
            }
            if let Some(value) = page.cell_str(row, column_index) {
                write_delimited_field(writer, value, delimiter)?;
            }
        }
        writer.write_all(b"\n")?;
    }
    Ok(())
}

fn write_delimited_field<W: Write>(writer: &mut W, value: &str, delimiter: u8) -> io::Result<()> {
    let needs_quotes = value.contains(['"', '\n', '\r']) || value.as_bytes().contains(&delimiter);
    if !needs_quotes {
        return writer.write_all(value.as_bytes());
    }
    writer.write_all(b"\"")?;
    writer.write_all(value.replace('"', "\"\"").as_bytes())?;
    writer.write_all(b"\"")
}

fn write_json_lines<W: Write>(page: &ColumnarPage, rows: &[usize], writer: &mut W) -> io::Result<()> {
    for &row in rows {
        write_json_row(page, row, writer)?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}

fn write_json_array<W: Write>(page: &ColumnarPage, rows: &[usize], writer: &mut W) -> io::Result<()> {
    writer.write_all(b"[\n")?;
    for (position, &row) in rows.iter().enumerate() {
        writer.write_all(b"  ")?;
        write_json_row(page, row, writer)?;
        if position + 1 < rows.len() {
            writer.write_all(b",")?;
        }
        writer.write_all(b"\n")?;
    }
    writer.write_all(b"]\n")
}

fn write_json_row<W: Write>(page: &ColumnarPage, row: usize, writer: &mut W) -> io::Result<()> {
    writer.write_all(b"{")?;
    for (column_index, column) in page.columns.iter().enumerate() {
        if column_index > 0 {
            writer.write_all(b",")?;
        }
        serde_json::to_writer(&mut *writer, &column.name).map_err(io::Error::other)?;
        writer.write_all(b":")?;
        let json_value = to_json_value(page.cell_str(row, column_index), column.is_numeric);
        serde_json::to_writer(&mut *writer, &json_value).map_err(io::Error::other)?;
    }
    writer.write_all(b"}")
}

fn write_json_values_row<W: Write>(
    columns: &[ColumnMeta],
    values: &[Option<String>],
    writer: &mut W,
) -> io::Result<()> {
    writer.write_all(b"{")?;
    for (column_index, column) in columns.iter().enumerate() {
        if column_index > 0 {
            writer.write_all(b",")?;
        }
        serde_json::to_writer(&mut *writer, &column.name).map_err(io::Error::other)?;
        writer.write_all(b":")?;
        let cell = values.get(column_index).and_then(Option::as_deref);
        let json_value = to_json_value(cell, column.is_numeric);
        serde_json::to_writer(&mut *writer, &json_value).map_err(io::Error::other)?;
    }
    writer.write_all(b"}")
}

fn to_json_value(cell: Option<&str>, is_numeric: bool) -> serde_json::Value {
    let Some(text) = cell else {
        return serde_json::Value::Null;
    };
    if is_numeric {
        if let Ok(integer) = text.parse::<i64>() {
            return serde_json::Value::from(integer);
        }
        if let Ok(unsigned) = text.parse::<u64>() {
            return serde_json::Value::from(unsigned);
        }
    }
    serde_json::Value::String(text.to_string())
}

fn write_sql_inserts<W: Write>(
    page: &ColumnarPage,
    rows: &[usize],
    target: &SqlInsertTarget,
    writer: &mut W,
) -> io::Result<()> {
    if rows.is_empty() || page.columns.is_empty() {
        return Ok(());
    }
    let dialect = target.dialect;
    let column_list = page
        .columns
        .iter()
        .map(|column| dialect.quote_ident(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let quoted_table = dialect.quote_ident(target.table_name);

    for &row in rows {
        let values = page
            .columns
            .iter()
            .enumerate()
            .map(|(column_index, meta)| format_sql_literal(dialect, &page.cell_value(row, column_index), meta.is_numeric))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(writer, "INSERT INTO {quoted_table} ({column_list}) VALUES ({values});")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{CellValue, ColumnMeta};

    fn column(name: &str, is_numeric: bool) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            org_name: Some(name.into()),
            table: Some("users".into()),
            schema: Some("app".into()),
            type_name: if is_numeric { "int".into() } else { "varchar(64)".into() },
            is_primary_key: false,
            is_unique_key: false,
            is_nullable: true,
            is_auto_increment: false,
            is_numeric,
            is_binary: false,
        }
    }

    fn sample_page() -> ColumnarPage {
        let mut page = ColumnarPage::new(vec![column("id", true), column("note", false)], "SELECT 1".into());
        page.push_row(&[CellValue::Text("1".into()), CellValue::Text("plain".into())]);
        page.push_row(&[
            CellValue::Text("2".into()),
            CellValue::Text("tab\there, \"quoted\"".into()),
        ]);
        page.push_row(&[CellValue::Text("3".into()), CellValue::Null]);
        page
    }

    fn render(rows: &[usize], format: ExportFormat) -> String {
        let mut buffer = Vec::new();
        export_rows(&sample_page(), rows, format, &SqlInsertTarget::mysql("users"), &mut buffer).expect("export succeeds");
        String::from_utf8(buffer).expect("utf8 output")
    }

    #[test]
    fn tsv_exports_only_selected_rows_in_given_order_and_quotes_tabs() {
        let output = render(&[2, 1], ExportFormat::Tsv);
        assert_eq!(output, "id\tnote\n3\t\n2\t\"tab\there, \"\"quoted\"\"\"\n");
    }

    #[test]
    fn csv_quotes_only_fields_containing_delimiter_quotes_or_newlines() {
        assert_eq!(render(&[0], ExportFormat::Csv), "id,note\n1,plain\n");
        assert!(render(&[1], ExportFormat::Csv).contains("\"tab\there, \"\"quoted\"\"\""));
    }

    #[test]
    fn json_array_maps_null_cells_to_json_null() {
        let output = render(&[0, 2], ExportFormat::JsonArray);
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid json");
        assert_eq!(parsed[0]["note"], "plain");
        assert!(parsed[1]["note"].is_null());
        assert_eq!(parsed.as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn json_keeps_column_order_and_emits_numeric_columns_as_numbers() {
        let output = render(&[0], ExportFormat::JsonLines);
        assert_eq!(output, "{\"id\":1,\"note\":\"plain\"}\n");
    }

    #[test]
    fn json_emits_only_exact_integers_as_numbers_and_keeps_decimals_as_strings() {
        assert_eq!(
            to_json_value(Some("18446744073709551615"), true).to_string(),
            "18446744073709551615"
        );
        assert_eq!(to_json_value(Some("-42"), true).to_string(), "-42");
        assert_eq!(
            to_json_value(Some("12.50"), true),
            serde_json::Value::String("12.50".into())
        );
        assert_eq!(to_json_value(Some("42"), false), serde_json::Value::String("42".into()));
        assert_eq!(to_json_value(None, true), serde_json::Value::Null);
    }

    #[test]
    fn sql_inserts_emit_one_statement_per_selected_row_with_null_literal() {
        let output = render(&[2], ExportFormat::SqlInserts);
        assert_eq!(output.lines().count(), 1);
        assert!(
            output.starts_with("INSERT INTO `users` (`id`, `note`) VALUES (3, NULL);"),
            "{output}"
        );
    }

    #[test]
    fn sql_inserts_follow_target_dialect_quoting() {
        let mut buffer = Vec::new();
        let target = SqlInsertTarget {
            dialect: SqlDialect::SqlServer,
            table_name: "users",
        };
        export_rows(&sample_page(), &[0], ExportFormat::SqlInserts, &target, &mut buffer).expect("export succeeds");
        assert_eq!(
            String::from_utf8(buffer).expect("utf8"),
            "INSERT INTO [users] ([id], [note]) VALUES (1, N'plain');\n"
        );
    }

    #[test]
    fn out_of_range_rows_are_ignored_and_empty_selection_writes_nothing_for_sql() {
        assert_eq!(render(&[99], ExportFormat::Tsv), "id\tnote\n");
        assert_eq!(render(&[], ExportFormat::SqlInserts), "");
    }

    fn stream_render(
        page: &ColumnarPage,
        rows: &[usize],
        format: ExportFormat,
        dialect: SqlDialect,
        table_name: &str,
    ) -> (String, u64) {
        let mut buffer = Vec::new();
        let mut exporter = StreamExporter::new(
            &mut buffer,
            format,
            page.columns.clone(),
            dialect,
            table_name.to_string(),
        )
        .expect("stream exporter init succeeds");
        for (idx, &row) in rows.iter().enumerate() {
            let values: Vec<Option<String>> = (0..page.columns.len())
                .map(|col| page.cell_str(row, col).map(str::to_string))
                .collect();
            exporter.write_row(&values).expect("stream write_row succeeds");
            assert_eq!(exporter.rows_written(), (idx + 1) as u64);
        }
        let written = exporter.finish().expect("stream finish succeeds");
        (String::from_utf8(buffer).expect("utf8 output"), written)
    }

    fn rich_page() -> ColumnarPage {
        let mut page = ColumnarPage::new(
            vec![
                column("id", true),
                column("amount", true),
                column("note", false),
                column("code", false),
            ],
            "SELECT id, amount, note, code FROM orders".into(),
        );
        page.push_row(&[
            CellValue::Text("1".into()),
            CellValue::Text("12.50".into()),
            CellValue::Text("plain text".into()),
            CellValue::Text("42".into()),
        ]);
        page.push_row(&[
            CellValue::Text("-42".into()),
            CellValue::Text("0.00".into()),
            CellValue::Text("comma, \"quote\", tab\there\nnewline".into()),
            CellValue::Text("it's ok".into()),
        ]);
        page.push_row(&[
            CellValue::Text("18446744073709551615".into()),
            CellValue::Null,
            CellValue::Null,
            CellValue::Text("".into()),
        ]);
        page
    }

    #[test]
    fn stream_exporter_matches_export_rows_across_all_formats_and_dialects() {
        let page = rich_page();
        let all: Vec<usize> = (0..page.row_count).collect();
        let selections: &[&[usize]] = &[&[], &[0], &[0, 2], &all];
        let dialects = [SqlDialect::MySql, SqlDialect::SqlServer];

        for &dialect in &dialects {
            let target = SqlInsertTarget {
                dialect,
                table_name: "orders",
            };
            for &format in &ExportFormat::ALL {
                for &rows in selections {
                    let mut batch_buffer = Vec::new();
                    export_rows(&page, rows, format, &target, &mut batch_buffer).expect("batch export succeeds");
                    let expected = String::from_utf8(batch_buffer).expect("utf8");

                    let (actual, written) = stream_render(&page, rows, format, dialect, "orders");
                    assert_eq!(written, rows.len() as u64);
                    assert_eq!(
                        actual, expected,
                        "mismatch for format={format:?} dialect={dialect:?} rows={rows:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn stream_exporter_zero_rows_formats_cleanly() {
        let page = sample_page();
        let (json_arr, count) = stream_render(&page, &[], ExportFormat::JsonArray, SqlDialect::MySql, "users");
        assert_eq!(count, 0);
        assert_eq!(json_arr, "[\n]\n");

        let (csv, _) = stream_render(&page, &[], ExportFormat::Csv, SqlDialect::MySql, "users");
        assert_eq!(csv, "id,note\n");

        let (tsv, _) = stream_render(&page, &[], ExportFormat::Tsv, SqlDialect::MySql, "users");
        assert_eq!(tsv, "id\tnote\n");

        let (jsonl, _) = stream_render(&page, &[], ExportFormat::JsonLines, SqlDialect::MySql, "users");
        assert_eq!(jsonl, "");

        let (sql, _) = stream_render(&page, &[], ExportFormat::SqlInserts, SqlDialect::SqlServer, "users");
        assert_eq!(sql, "");
    }
}
