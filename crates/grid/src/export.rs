use crate::mutator::format_sql_literal;
use crate::page::{escape_ident, ColumnarPage};
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    JsonLines,
    JsonArray,
    SqlInserts,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::JsonLines => "jsonl",
            Self::JsonArray => "json",
            Self::SqlInserts => "sql",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV (.csv)",
            Self::JsonLines => "JSON Lines (.jsonl)",
            Self::JsonArray => "JSON Array (.json)",
            Self::SqlInserts => "SQL INSERT (.sql)",
        }
    }
}

pub fn export_csv<W: Write>(page: &ColumnarPage, writer: &mut W) -> io::Result<()> {
    for (i, col) in page.columns.iter().enumerate() {
        if i > 0 {
            writer.write_all(b",")?;
        }
        write_csv_field(writer, &col.name)?;
    }
    writer.write_all(b"\n")?;

    for row in 0..page.row_count {
        for col in 0..page.columns.len() {
            if col > 0 {
                writer.write_all(b",")?;
            }
            if let Some(s) = page.cell_str(row, col) {
                write_csv_field(writer, s)?;
            }
        }
        writer.write_all(b"\n")?;
    }
    Ok(())
}

fn write_csv_field<W: Write>(writer: &mut W, value: &str) -> io::Result<()> {
    let needs_quotes = value.contains([',', '"', '\n', '\r']);
    if !needs_quotes {
        return writer.write_all(value.as_bytes());
    }
    writer.write_all(b"\"")?;
    for ch in value.chars() {
        if ch == '"' {
            writer.write_all(b"\"\"")?;
        } else {
            let mut buf = [0u8; 4];
            writer.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
        }
    }
    writer.write_all(b"\"")
}

pub fn export_json_lines<W: Write>(page: &ColumnarPage, writer: &mut W) -> io::Result<()> {
    for row in 0..page.row_count {
        write_json_row(page, row, writer)?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}

pub fn export_json_array<W: Write>(page: &ColumnarPage, writer: &mut W) -> io::Result<()> {
    writer.write_all(b"[\n")?;
    for row in 0..page.row_count {
        writer.write_all(b"  ")?;
        write_json_row(page, row, writer)?;
        if row + 1 < page.row_count {
            writer.write_all(b",")?;
        }
        writer.write_all(b"\n")?;
    }
    writer.write_all(b"]\n")
}

fn write_json_row<W: Write>(page: &ColumnarPage, row: usize, writer: &mut W) -> io::Result<()> {
    let mut map = serde_json::Map::with_capacity(page.columns.len());
    for (col_idx, col) in page.columns.iter().enumerate() {
        let json_val = match page.cell_str(row, col_idx) {
            None => serde_json::Value::Null,
            Some(s) => serde_json::Value::String(s.to_string()),
        };
        map.insert(col.name.clone(), json_val);
    }
    serde_json::to_writer(writer, &serde_json::Value::Object(map))
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

pub fn export_sql_inserts<W: Write>(
    page: &ColumnarPage,
    table_name: &str,
    writer: &mut W,
) -> io::Result<()> {
    if page.row_count == 0 || page.columns.is_empty() {
        return Ok(());
    }
    let col_list = page
        .columns
        .iter()
        .map(|c| format!("`{}`", escape_ident(&c.name)))
        .collect::<Vec<_>>()
        .join(", ");
    let escaped_table = escape_ident(table_name);

    for row in 0..page.row_count {
        let vals = page
            .columns
            .iter()
            .enumerate()
            .map(|(c, meta)| format_sql_literal(&page.cell_value(row, c), meta.is_numeric))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(writer, "INSERT INTO `{escaped_table}` ({col_list}) VALUES ({vals});")?;
    }
    Ok(())
}
