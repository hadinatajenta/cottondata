use crate::mutator::format_sql_text_literal;
use cottondata_core::SqlDialect;
use serde::de::{DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

const MAX_SAMPLE_ERRORS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    Csv,
    Tsv,
    JsonArray,
    JsonLines,
}

impl ImportFormat {
    pub fn detect_from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref()
        {
            Some("tsv") => Self::Tsv,
            Some("jsonl" | "ndjson") => Self::JsonLines,
            Some("json") => Self::JsonArray,
            _ => Self::Csv,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV (.csv)",
            Self::Tsv => "TSV (.tsv)",
            Self::JsonArray => "JSON Array (.json)",
            Self::JsonLines => "JSON Lines (.jsonl)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportPreview {
    pub format: ImportFormat,
    pub headers: Vec<String>,
    pub sample_rows: Vec<Vec<Option<String>>>,
}

#[derive(Debug, Clone)]
pub struct TargetColumnSpec {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub is_numeric: bool,
    pub has_default: bool,
}

#[derive(Debug, Clone)]
pub struct ColumnMapping {
    pub source_index: usize,
    pub source_name: String,
    pub target_column: Option<String>,
    pub empty_as_null: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DryRunReport {
    pub total_rows: u64,
    pub valid_rows: u64,
    pub error_count: u64,
    pub sample_errors: Vec<String>,
}

pub fn auto_map_columns(headers: &[String], target_cols: &[TargetColumnSpec]) -> Vec<ColumnMapping> {
    let mut claimed = vec![false; target_cols.len()];
    let mut matched_targets: Vec<Option<String>> = vec![None; headers.len()];

    for (header_idx, header) in headers.iter().enumerate() {
        if let Some((target_idx, target)) = target_cols
            .iter()
            .enumerate()
            .find(|(idx, col)| !claimed[*idx] && col.name.eq_ignore_ascii_case(header))
        {
            claimed[target_idx] = true;
            matched_targets[header_idx] = Some(target.name.clone());
        }
    }

    let normalized_targets: Vec<String> = target_cols
        .iter()
        .map(|col| normalize_column_name(&col.name))
        .collect();

    for (header_idx, header) in headers.iter().enumerate() {
        if matched_targets[header_idx].is_some() {
            continue;
        }
        let normalized_header = normalize_column_name(header);
        if normalized_header.is_empty() {
            continue;
        }
        if let Some((target_idx, target)) = target_cols
            .iter()
            .enumerate()
            .find(|(idx, _)| !claimed[*idx] && normalized_targets[*idx] == normalized_header)
        {
            claimed[target_idx] = true;
            matched_targets[header_idx] = Some(target.name.clone());
        }
    }

    headers
        .iter()
        .enumerate()
        .zip(matched_targets)
        .map(|((source_index, source_name), target_column)| ColumnMapping {
            source_index,
            source_name: source_name.clone(),
            target_column,
            empty_as_null: true,
        })
        .collect()
}

pub fn inspect_import_file(
    path: &Path,
    format: ImportFormat,
    max_sample_rows: usize,
) -> Result<ImportPreview, String> {
    match format {
        ImportFormat::Csv => inspect_delimited_file(path, format, b',', max_sample_rows),
        ImportFormat::Tsv => inspect_delimited_file(path, format, b'\t', max_sample_rows),
        ImportFormat::JsonArray => inspect_json_array_file(path, format, max_sample_rows),
        ImportFormat::JsonLines => inspect_json_lines_file(path, format, max_sample_rows),
    }
}

pub fn dry_run_import(
    path: &Path,
    format: ImportFormat,
    mappings: &[ColumnMapping],
    target_cols: &[TargetColumnSpec],
) -> Result<DryRunReport, String> {
    let active = resolve_active_mappings(mappings, target_cols)?;
    let mut report = DryRunReport::default();

    stream_file_rows(path, format, |row_num, row| {
        report.total_rows += 1;
        let mut row_has_error = false;

        for mapped in &active {
            let value = extract_cell_value(row, mapped);
            match value {
                None => {
                    if !mapped.is_nullable && !mapped.has_default {
                        row_has_error = true;
                        report.error_count += 1;
                        if report.sample_errors.len() < MAX_SAMPLE_ERRORS {
                            report.sample_errors.push(format!(
                                "Row {row_num}, column `{}`: NULL is not allowed for non-nullable column",
                                mapped.target_name
                            ));
                        }
                    }
                }
                Some(text) => {
                    if mapped.is_numeric && !is_valid_numeric(&text) {
                        row_has_error = true;
                        report.error_count += 1;
                        if report.sample_errors.len() < MAX_SAMPLE_ERRORS {
                            report.sample_errors.push(format!(
                                "Row {row_num}, column `{}`: expected numeric value, got '{}'",
                                mapped.target_name, text
                            ));
                        }
                    }
                }
            }
        }

        if !row_has_error {
            report.valid_rows += 1;
        }
        Ok(true)
    })?;

    Ok(report)
}

#[allow(clippy::too_many_arguments)]
pub fn stream_import_batches<F>(
    path: &Path,
    format: ImportFormat,
    dialect: SqlDialect,
    schema: &str,
    table: &str,
    mappings: &[ColumnMapping],
    target_cols: &[TargetColumnSpec],
    batch_size: usize,
    mut on_batch: F,
) -> Result<u64, String>
where
    F: FnMut(String, usize) -> Result<bool, String>,
{
    let active = resolve_active_mappings(mappings, target_cols)?;
    let effective_batch_size = batch_size.clamp(1, 1000);
    let fq_table = dialect.qualified_table(schema, table);
    let quoted_cols = active
        .iter()
        .map(|mapped| dialect.quote_ident(&mapped.target_name))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_prefix = format!("INSERT INTO {fq_table} ({quoted_cols}) VALUES ");

    let mut sql = String::new();
    let mut batch_rows: usize = 0;
    let mut total_emitted: u64 = 0;
    let mut stopped = false;

    stream_file_rows(path, format, |_row_num, row| {
        if batch_rows == 0 {
            sql.push_str(&insert_prefix);
        } else {
            sql.push_str(", ");
        }
        sql.push('(');
        for (col_idx, mapped) in active.iter().enumerate() {
            if col_idx > 0 {
                sql.push_str(", ");
            }
            match extract_cell_value(row, mapped) {
                None => sql.push_str("NULL"),
                Some(text) => {
                    let literal = format_sql_text_literal(dialect, &text, mapped.is_numeric);
                    sql.push_str(&literal);
                }
            }
        }
        sql.push(')');
        batch_rows += 1;

        if batch_rows >= effective_batch_size {
            sql.push(';');
            let statement = std::mem::take(&mut sql);
            let count = batch_rows;
            batch_rows = 0;
            total_emitted += count as u64;
            let keep_going = on_batch(statement, count)?;
            if !keep_going {
                stopped = true;
                return Ok(false);
            }
        }
        Ok(true)
    })?;

    if !stopped && batch_rows > 0 {
        sql.push(';');
        let count = batch_rows;
        total_emitted += count as u64;
        let _ = on_batch(sql, count)?;
    }

    Ok(total_emitted)
}

struct ActiveMapping {
    source_index: usize,
    source_name: String,
    target_name: String,
    empty_as_null: bool,
    is_nullable: bool,
    is_numeric: bool,
    has_default: bool,
}

#[derive(Clone, Copy)]
enum RowRef<'a> {
    Delimited(&'a [String]),
    JsonObject(&'a serde_json::Map<String, serde_json::Value>),
}

fn resolve_active_mappings(
    mappings: &[ColumnMapping],
    target_cols: &[TargetColumnSpec],
) -> Result<Vec<ActiveMapping>, String> {
    let mut active = Vec::new();
    let mut seen_targets: HashSet<String> = HashSet::new();

    for mapping in mappings {
        let Some(target_col) = mapping.target_column.as_deref() else {
            continue;
        };
        if target_col.is_empty() {
            continue;
        }
        let spec = target_cols
            .iter()
            .find(|col| col.name.eq_ignore_ascii_case(target_col))
            .ok_or_else(|| format!("Mapped target column `{target_col}` does not exist in target table."))?;

        let canonical_key = spec.name.to_ascii_lowercase();
        if !seen_targets.insert(canonical_key) {
            return Err(format!("Target column `{}` is mapped more than once.", spec.name));
        }

        active.push(ActiveMapping {
            source_index: mapping.source_index,
            source_name: mapping.source_name.clone(),
            target_name: spec.name.clone(),
            empty_as_null: mapping.empty_as_null,
            is_nullable: spec.is_nullable,
            is_numeric: spec.is_numeric,
            has_default: spec.has_default,
        });
    }

    if active.is_empty() {
        return Err("At least one column must be mapped to a target column.".to_string());
    }

    Ok(active)
}

fn normalize_column_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|ch| {
            if ch == ' ' || ch == '-' {
                '_'
            } else {
                ch.to_ascii_lowercase()
            }
        })
        .collect()
}

fn is_valid_numeric(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed.parse::<f64>().is_ok_and(f64::is_finite)
}

fn extract_cell_value<'a>(row: RowRef<'a>, mapping: &ActiveMapping) -> Option<Cow<'a, str>> {
    let raw = match row {
        RowRef::Delimited(fields) => fields.get(mapping.source_index).map(|s| Cow::Borrowed(s.as_str())),
        RowRef::JsonObject(map) => map.get(&mapping.source_name).and_then(json_value_to_cow),
    };
    match raw {
        None => None,
        Some(text) if text.is_empty() && mapping.empty_as_null => None,
        Some(text) => Some(text),
    }
}

fn json_value_to_cow(value: &serde_json::Value) -> Option<Cow<'_, str>> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(Cow::Borrowed(s.as_str())),
        serde_json::Value::Number(n) => Some(Cow::Owned(n.to_string())),
        serde_json::Value::Bool(b) => Some(Cow::Owned(b.to_string())),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Some(Cow::Owned(serde_json::to_string(value).unwrap_or_default()))
        }
    }
}

fn open_bom_stripped_reader(path: &Path) -> Result<BufReader<BomStrippingReader<File>>, String> {
    let file = File::open(path).map_err(|err| format!("Failed to open '{}': {err}", path.display()))?;
    Ok(BufReader::new(BomStrippingReader::new(file)))
}

struct BomStrippingReader<R> {
    inner: R,
    checked: bool,
    prefix: [u8; 3],
    prefix_pos: usize,
    prefix_len: usize,
}

impl<R> BomStrippingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            checked: false,
            prefix: [0; 3],
            prefix_pos: 0,
            prefix_len: 0,
        }
    }
}

impl<R: Read> Read for BomStrippingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if !self.checked {
            self.checked = true;
            let mut filled = 0;
            while filled < 3 {
                match self.inner.read(&mut self.prefix[filled..3]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => return Err(err),
                }
            }
            if filled == 3 && self.prefix == [0xEF, 0xBB, 0xBF] {
                self.prefix_len = 0;
            } else {
                self.prefix_len = filled;
            }
            self.prefix_pos = 0;
        }
        if self.prefix_pos < self.prefix_len {
            let available = self.prefix_len - self.prefix_pos;
            let to_copy = available.min(buf.len());
            buf[..to_copy].copy_from_slice(&self.prefix[self.prefix_pos..self.prefix_pos + to_copy]);
            self.prefix_pos += to_copy;
            return Ok(to_copy);
        }
        self.inner.read(buf)
    }
}

struct DelimitedReader<R> {
    reader: R,
    delimiter: u8,
    record_index: u64,
}

impl<R: BufRead> DelimitedReader<R> {
    fn new(reader: R, delimiter: u8) -> Self {
        Self {
            reader,
            delimiter,
            record_index: 0,
        }
    }

    fn peek_byte(&mut self) -> io::Result<Option<u8>> {
        let buf = self.reader.fill_buf()?;
        Ok(buf.first().copied())
    }

    fn next_byte(&mut self) -> io::Result<Option<u8>> {
        let buf = self.reader.fill_buf()?;
        if let Some(&b) = buf.first() {
            self.reader.consume(1);
            Ok(Some(b))
        } else {
            Ok(None)
        }
    }

    fn next_record(&mut self) -> Result<Option<Vec<String>>, String> {
        if self
            .peek_byte()
            .map_err(|err| format!("Failed reading delimited stream: {err}"))?
            .is_none()
        {
            return Ok(None);
        }

        self.record_index += 1;
        let mut fields = Vec::new();
        let mut field_bytes = Vec::new();

        loop {
            field_bytes.clear();
            let starts_quoted = self
                .peek_byte()
                .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?
                == Some(b'"');

            let ended_record = if starts_quoted {
                let _ = self
                    .next_byte()
                    .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?;
                loop {
                    match self
                        .next_byte()
                        .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?
                    {
                        None => {
                            return Err(format!(
                                "Unterminated quoted field at record {}",
                                self.record_index
                            ));
                        }
                        Some(b'"') => {
                            if self
                                .peek_byte()
                                .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?
                                == Some(b'"')
                            {
                                let _ = self
                                    .next_byte()
                                    .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?;
                                field_bytes.push(b'"');
                            } else {
                                break;
                            }
                        }
                        Some(byte) => field_bytes.push(byte),
                    }
                }
                loop {
                    match self
                        .next_byte()
                        .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?
                    {
                        Some(byte) if byte == self.delimiter => break false,
                        Some(b'\r') => {
                            if self
                                .peek_byte()
                                .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?
                                == Some(b'\n')
                            {
                                let _ = self
                                    .next_byte()
                                    .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?;
                            }
                            break true;
                        }
                        Some(b'\n') | None => break true,
                        Some(byte) => field_bytes.push(byte),
                    }
                }
            } else {
                loop {
                    match self
                        .next_byte()
                        .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?
                    {
                        Some(byte) if byte == self.delimiter => break false,
                        Some(b'\r') => {
                            if self
                                .peek_byte()
                                .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?
                                == Some(b'\n')
                            {
                                let _ = self
                                    .next_byte()
                                    .map_err(|err| format!("Failed reading record {}: {err}", self.record_index))?;
                            }
                            break true;
                        }
                        Some(b'\n') | None => break true,
                        Some(byte) => field_bytes.push(byte),
                    }
                }
            };

            let field = String::from_utf8(std::mem::take(&mut field_bytes))
                .map_err(|err| format!("Invalid UTF-8 in record {}: {err}", self.record_index))?;
            fields.push(field);

            if ended_record {
                return Ok(Some(fields));
            }
        }
    }
}

struct OrderedJsonObject(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for OrderedJsonObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ObjectVisitor;

        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = OrderedJsonObject;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries: Vec<(String, serde_json::Value)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
                    if let Some((_, existing)) = entries.iter_mut().find(|(k, _)| k == &key) {
                        *existing = value;
                    } else {
                        entries.push((key, value));
                    }
                }
                Ok(OrderedJsonObject(entries))
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}

fn stream_json_array_elements<T, R, F>(reader: R, mut on_element: F) -> Result<(), String>
where
    T: for<'de> Deserialize<'de>,
    R: Read,
    F: FnMut(u64, T) -> Result<bool, String>,
{
    struct ArrayStreamSeed<'a, T, F> {
        on_element: &'a mut F,
        stopped: &'a mut bool,
        callback_err: &'a mut Option<String>,
        row_num: &'a mut u64,
        marker: std::marker::PhantomData<T>,
    }

    impl<'de, T, F> DeserializeSeed<'de> for ArrayStreamSeed<'_, T, F>
    where
        T: for<'a> Deserialize<'a>,
        F: FnMut(u64, T) -> Result<bool, String>,
    {
        type Value = ();

        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct ArrayVisitor<'a, T, F> {
                on_element: &'a mut F,
                stopped: &'a mut bool,
                callback_err: &'a mut Option<String>,
                row_num: &'a mut u64,
                marker: std::marker::PhantomData<T>,
            }

            impl<'de, T, F> Visitor<'de> for ArrayVisitor<'_, T, F>
            where
                T: for<'a> Deserialize<'a>,
                F: FnMut(u64, T) -> Result<bool, String>,
            {
                type Value = ();

                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("a JSON array of objects")
                }

                fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
                where
                    A: SeqAccess<'de>,
                {
                    while let Some(element) = seq.next_element::<T>()? {
                        *self.row_num += 1;
                        match (self.on_element)(*self.row_num, element) {
                            Ok(true) => {}
                            Ok(false) => {
                                *self.stopped = true;
                                return Err(A::Error::custom("stopped"));
                            }
                            Err(err) => {
                                *self.callback_err = Some(err);
                                return Err(A::Error::custom("callback"));
                            }
                        }
                    }
                    Ok(())
                }
            }

            deserializer.deserialize_seq(ArrayVisitor {
                on_element: self.on_element,
                stopped: self.stopped,
                callback_err: self.callback_err,
                row_num: self.row_num,
                marker: self.marker,
            })
        }
    }

    let mut stopped = false;
    let mut callback_err: Option<String> = None;
    let mut row_num: u64 = 0;

    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let seed = ArrayStreamSeed {
        on_element: &mut on_element,
        stopped: &mut stopped,
        callback_err: &mut callback_err,
        row_num: &mut row_num,
        marker: std::marker::PhantomData::<T>,
    };

    let result = seed.deserialize(&mut deserializer);

    if let Some(err) = callback_err {
        return Err(err);
    }
    if stopped {
        return Ok(());
    }
    result.map_err(|err| format!("Failed to parse JSON array near row {}: {err}", row_num + 1))?;
    deserializer
        .end()
        .map_err(|err| format!("Trailing content after JSON array: {err}"))?;
    Ok(())
}

fn inspect_delimited_file(
    path: &Path,
    format: ImportFormat,
    delimiter: u8,
    max_sample_rows: usize,
) -> Result<ImportPreview, String> {
    let reader = open_bom_stripped_reader(path)?;
    let mut parser = DelimitedReader::new(reader, delimiter);

    let Some(headers) = parser.next_record()? else {
        return Ok(ImportPreview {
            format,
            headers: Vec::new(),
            sample_rows: Vec::new(),
        });
    };

    let mut sample_rows = Vec::with_capacity(max_sample_rows.min(64));
    while sample_rows.len() < max_sample_rows {
        let Some(record) = parser.next_record()? else {
            break;
        };
        let row = (0..headers.len()).map(|idx| record.get(idx).cloned()).collect();
        sample_rows.push(row);
    }

    Ok(ImportPreview {
        format,
        headers,
        sample_rows,
    })
}

fn inspect_json_array_file(
    path: &Path,
    format: ImportFormat,
    max_sample_rows: usize,
) -> Result<ImportPreview, String> {
    let reader = open_bom_stripped_reader(path)?;
    let mut objects: Vec<OrderedJsonObject> = Vec::with_capacity(max_sample_rows.min(64));

    if max_sample_rows > 0 {
        stream_json_array_elements::<OrderedJsonObject, _, _>(reader, |_row_num, obj| {
            objects.push(obj);
            Ok(objects.len() < max_sample_rows)
        })?;
    }

    Ok(build_json_preview(format, objects))
}

fn inspect_json_lines_file(
    path: &Path,
    format: ImportFormat,
    max_sample_rows: usize,
) -> Result<ImportPreview, String> {
    let mut reader = open_bom_stripped_reader(path)?;
    let mut objects: Vec<OrderedJsonObject> = Vec::with_capacity(max_sample_rows.min(64));
    let mut line = String::new();
    let mut line_num: u64 = 0;

    while objects.len() < max_sample_rows {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .map_err(|err| format!("Failed reading JSON Lines file: {err}"))?;
        if bytes_read == 0 {
            break;
        }
        line_num += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let obj: OrderedJsonObject = serde_json::from_str(trimmed)
            .map_err(|err| format!("Line {line_num}: expected JSON object ({err})"))?;
        objects.push(obj);
    }

    Ok(build_json_preview(format, objects))
}

fn build_json_preview(format: ImportFormat, objects: Vec<OrderedJsonObject>) -> ImportPreview {
    let mut headers = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for obj in &objects {
        for (key, _) in &obj.0 {
            if seen.insert(key.clone()) {
                headers.push(key.clone());
            }
        }
    }

    let sample_rows = objects
        .into_iter()
        .map(|obj| {
            headers
                .iter()
                .map(|header| {
                    obj.0
                        .iter()
                        .find(|(k, _)| k == header)
                        .and_then(|(_, val)| json_value_to_cow(val).map(Cow::into_owned))
                })
                .collect()
        })
        .collect();

    ImportPreview {
        format,
        headers,
        sample_rows,
    }
}

fn stream_file_rows<F>(path: &Path, format: ImportFormat, mut on_row: F) -> Result<(), String>
where
    F: FnMut(u64, RowRef<'_>) -> Result<bool, String>,
{
    match format {
        ImportFormat::Csv => stream_delimited_rows(path, b',', on_row),
        ImportFormat::Tsv => stream_delimited_rows(path, b'\t', on_row),
        ImportFormat::JsonArray => {
            let reader = open_bom_stripped_reader(path)?;
            stream_json_array_elements::<serde_json::Value, _, _>(reader, |row_num, value| match value {
                serde_json::Value::Object(map) => on_row(row_num, RowRef::JsonObject(&map)),
                _ => Err(format!("Row {row_num}: expected JSON object")),
            })
        }
        ImportFormat::JsonLines => {
            let mut reader = open_bom_stripped_reader(path)?;
            let mut line = String::new();
            let mut line_num: u64 = 0;
            let mut row_num: u64 = 0;

            loop {
                line.clear();
                let bytes_read = reader
                    .read_line(&mut line)
                    .map_err(|err| format!("Failed reading JSON Lines file: {err}"))?;
                if bytes_read == 0 {
                    break;
                }
                line_num += 1;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let value: serde_json::Value = serde_json::from_str(trimmed)
                    .map_err(|err| format!("Line {line_num}: invalid JSON: {err}"))?;
                let serde_json::Value::Object(map) = value else {
                    return Err(format!("Line {line_num}: expected JSON object"));
                };
                row_num += 1;
                if !on_row(row_num, RowRef::JsonObject(&map))? {
                    break;
                }
            }
            Ok(())
        }
    }
}

fn stream_delimited_rows<F>(path: &Path, delimiter: u8, mut on_row: F) -> Result<(), String>
where
    F: FnMut(u64, RowRef<'_>) -> Result<bool, String>,
{
    let reader = open_bom_stripped_reader(path)?;
    let mut parser = DelimitedReader::new(reader, delimiter);

    if parser.next_record()?.is_none() {
        return Ok(());
    }

    let mut row_num: u64 = 0;
    while let Some(record) = parser.next_record()? {
        row_num += 1;
        if !on_row(row_num, RowRef::Delimited(&record))? {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempFile {
        path: PathBuf,
    }

    impl TempFile {
        fn new(ext: &str, bytes: &[u8]) -> Self {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("cottondata_grid_import_{pid}_{id}.{ext}"));
            std::fs::write(&path, bytes).expect("write temp file");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn target_col(name: &str, is_nullable: bool, is_numeric: bool, has_default: bool) -> TargetColumnSpec {
        TargetColumnSpec {
            name: name.to_string(),
            data_type: if is_numeric {
                "decimal(10,2)".to_string()
            } else {
                "varchar(100)".to_string()
            },
            is_nullable,
            is_numeric,
            has_default,
        }
    }

    #[test]
    fn detect_format_from_path_and_labels() {
        assert_eq!(ImportFormat::detect_from_path(Path::new("orders.csv")), ImportFormat::Csv);
        assert_eq!(ImportFormat::detect_from_path(Path::new("orders.TSV")), ImportFormat::Tsv);
        assert_eq!(
            ImportFormat::detect_from_path(Path::new("orders.json")),
            ImportFormat::JsonArray
        );
        assert_eq!(
            ImportFormat::detect_from_path(Path::new("orders.jsonl")),
            ImportFormat::JsonLines
        );
        assert_eq!(
            ImportFormat::detect_from_path(Path::new("orders.ndjson")),
            ImportFormat::JsonLines
        );
        assert_eq!(ImportFormat::detect_from_path(Path::new("orders.txt")), ImportFormat::Csv);
        assert!(!ImportFormat::Csv.label().is_empty());
        assert!(!ImportFormat::Tsv.label().is_empty());
        assert!(!ImportFormat::JsonArray.label().is_empty());
        assert!(!ImportFormat::JsonLines.label().is_empty());
    }

    #[test]
    fn inspect_rfc4180_csv_with_quotes_escaped_quotes_newlines_crlf_and_bom() {
        let mut payload = Vec::from(&[0xEFu8, 0xBB, 0xBF][..]);
        payload.extend_from_slice(
            b"id,name,bio\r\n1,\"Alice \"\"Ace\"\"\",\"Line 1\r\nLine 2\"\r\n2,Bob,\"comma, inside\"\r\n3,Carol,\r\n",
        );
        let file = TempFile::new("csv", &payload);

        let preview = inspect_import_file(file.path(), ImportFormat::Csv, 10).expect("inspect csv");
        assert_eq!(preview.format, ImportFormat::Csv);
        assert_eq!(preview.headers, vec!["id", "name", "bio"]);
        assert_eq!(preview.sample_rows.len(), 3);
        assert_eq!(
            preview.sample_rows[0],
            vec![
                Some("1".to_string()),
                Some("Alice \"Ace\"".to_string()),
                Some("Line 1\r\nLine 2".to_string()),
            ]
        );
        assert_eq!(
            preview.sample_rows[1],
            vec![
                Some("2".to_string()),
                Some("Bob".to_string()),
                Some("comma, inside".to_string()),
            ]
        );
        assert_eq!(
            preview.sample_rows[2],
            vec![Some("3".to_string()), Some("Carol".to_string()), Some(String::new())]
        );
    }

    #[test]
    fn inspect_tsv_handles_tabs_and_quoted_tabs() {
        let file = TempFile::new(
            "tsv",
            b"id\tnote\n1\tplain\n2\t\"with\ttab and \"\"quote\"\"\"\n",
        );
        let preview = inspect_import_file(file.path(), ImportFormat::Tsv, 5).expect("inspect tsv");
        assert_eq!(preview.headers, vec!["id", "note"]);
        assert_eq!(preview.sample_rows.len(), 2);
        assert_eq!(
            preview.sample_rows[1],
            vec![Some("2".to_string()), Some("with\ttab and \"quote\"".to_string())]
        );
    }

    #[test]
    fn inspect_json_array_collects_union_of_keys_in_first_seen_order_and_formats_values() {
        let json = br#"[
            {"id": 1, "name": "Alice", "active": true, "meta": {"role": "admin"}},
            {"id": 2, "name": null, "tags": ["a", "b"], "score": 98.5},
            {"id": 3, "name": "Ignored Beyond Sample", "extra_key": 999}
        ]"#;
        let file = TempFile::new("json", json);

        let preview = inspect_import_file(file.path(), ImportFormat::JsonArray, 2).expect("inspect json array");
        assert_eq!(
            preview.headers,
            vec!["id", "name", "active", "meta", "tags", "score"]
        );
        assert_eq!(preview.sample_rows.len(), 2);
        assert_eq!(
            preview.sample_rows[0],
            vec![
                Some("1".to_string()),
                Some("Alice".to_string()),
                Some("true".to_string()),
                Some("{\"role\":\"admin\"}".to_string()),
                None,
                None,
            ]
        );
        assert_eq!(
            preview.sample_rows[1],
            vec![
                Some("2".to_string()),
                None,
                None,
                None,
                Some("[\"a\",\"b\"]".to_string()),
                Some("98.5".to_string()),
            ]
        );
    }

    #[test]
    fn inspect_json_lines_handles_bom_blank_lines_and_union_keys() {
        let mut payload = Vec::from(&[0xEFu8, 0xBB, 0xBF][..]);
        payload.extend_from_slice(
            b"{\"id\":1,\"email\":\"a@example.com\"}\n\n{\"id\":2,\"email\":null,\"verified\":false}\n",
        );
        let file = TempFile::new("jsonl", &payload);

        let preview = inspect_import_file(file.path(), ImportFormat::JsonLines, 10).expect("inspect jsonl");
        assert_eq!(preview.headers, vec!["id", "email", "verified"]);
        assert_eq!(preview.sample_rows.len(), 2);
        assert_eq!(
            preview.sample_rows[0],
            vec![Some("1".to_string()), Some("a@example.com".to_string()), None]
        );
        assert_eq!(
            preview.sample_rows[1],
            vec![Some("2".to_string()), None, Some("false".to_string())]
        );
    }

    #[test]
    fn auto_map_columns_prefers_case_insensitive_exact_then_normalized_fallback() {
        let headers = vec![
            "ID".to_string(),
            "First Name".to_string(),
            "unit-price".to_string(),
            "unmapped_col".to_string(),
        ];
        let targets = vec![
            target_col("id", false, true, false),
            target_col("first_name", false, false, false),
            target_col("unit_price", true, true, false),
        ];

        let mappings = auto_map_columns(&headers, &targets);
        assert_eq!(mappings.len(), 4);
        assert_eq!(mappings[0].target_column.as_deref(), Some("id"));
        assert!(mappings[0].empty_as_null);
        assert_eq!(mappings[1].target_column.as_deref(), Some("first_name"));
        assert_eq!(mappings[2].target_column.as_deref(), Some("unit_price"));
        assert_eq!(mappings[3].target_column, None);
    }

    #[test]
    fn dry_run_import_catches_not_null_and_numeric_violations_and_caps_sample_errors() {
        let mut csv = String::from("name,amount,status\n");
        csv.push_str("Alice,12.50,ok\n");
        csv.push_str(",42,ok\n");
        csv.push_str("Bob,abc,\n");
        for idx in 0..25 {
            csv.push_str(&format!("User{idx},bad_{idx},ok\n"));
        }
        let file = TempFile::new("csv", csv.as_bytes());

        let targets = vec![
            target_col("name", false, false, false),
            target_col("amount", true, true, false),
            target_col("status", false, false, true),
        ];
        let headers = vec!["name".to_string(), "amount".to_string(), "status".to_string()];
        let mappings = auto_map_columns(&headers, &targets);

        let report = dry_run_import(file.path(), ImportFormat::Csv, &mappings, &targets).expect("dry run");
        assert_eq!(report.total_rows, 28);
        assert_eq!(report.valid_rows, 1);
        assert_eq!(report.error_count, 27);
        assert_eq!(report.sample_errors.len(), 20);
        assert!(report.sample_errors[0].contains("Row 2, column `name`"));
        assert_eq!(
            report.sample_errors[1],
            "Row 3, column `amount`: expected numeric value, got 'abc'"
        );

        let unmapped = vec![ColumnMapping {
            source_index: 0,
            source_name: "name".to_string(),
            target_column: None,
            empty_as_null: true,
        }];
        assert!(dry_run_import(file.path(), ImportFormat::Csv, &unmapped, &targets).is_err());
    }

    #[test]
    fn stream_import_batches_emits_mysql_and_sql_server_batched_inserts_and_supports_early_stop() {
        let file = TempFile::new(
            "csv",
            b"id,name,note\n1,Alice,O'Neil\n2,Bob,\n3,Carol,C:\\temp\n4,Dave,done\n5,Eve,last\n",
        );
        let targets = vec![
            target_col("id", false, true, false),
            target_col("name", false, false, false),
            target_col("note", true, false, false),
        ];
        let headers = vec!["id".to_string(), "name".to_string(), "note".to_string()];
        let mappings = auto_map_columns(&headers, &targets);

        let mut mysql_batches = Vec::new();
        let emitted_mysql = stream_import_batches(
            file.path(),
            ImportFormat::Csv,
            SqlDialect::MySql,
            "shop",
            "users",
            &mappings,
            &targets,
            2,
            |sql, count| {
                mysql_batches.push((sql, count));
                Ok(true)
            },
        )
        .expect("mysql import");

        assert_eq!(emitted_mysql, 5);
        assert_eq!(mysql_batches.len(), 3);
        assert_eq!(
            mysql_batches[0],
            (
                "INSERT INTO `shop`.`users` (`id`, `name`, `note`) VALUES (1, 'Alice', 'O''Neil'), (2, 'Bob', NULL);"
                    .to_string(),
                2
            )
        );
        assert_eq!(
            mysql_batches[1],
            (
                "INSERT INTO `shop`.`users` (`id`, `name`, `note`) VALUES (3, 'Carol', 'C:\\\\temp'), (4, 'Dave', 'done');"
                    .to_string(),
                2
            )
        );
        assert_eq!(
            mysql_batches[2],
            (
                "INSERT INTO `shop`.`users` (`id`, `name`, `note`) VALUES (5, 'Eve', 'last');".to_string(),
                1
            )
        );

        let mut mssql_batches = Vec::new();
        let emitted_mssql = stream_import_batches(
            file.path(),
            ImportFormat::Csv,
            SqlDialect::SqlServer,
            "shop",
            "sales.users",
            &mappings,
            &targets,
            2,
            |sql, count| {
                mssql_batches.push((sql, count));
                Ok(mssql_batches.len() < 2)
            },
        )
        .expect("mssql import");

        assert_eq!(emitted_mssql, 4);
        assert_eq!(mssql_batches.len(), 2);
        assert_eq!(
            mssql_batches[0],
            (
                "INSERT INTO [shop].[sales].[users] ([id], [name], [note]) VALUES (1, N'Alice', N'O''Neil'), (2, N'Bob', NULL);"
                    .to_string(),
                2
            )
        );
        assert_eq!(
            mssql_batches[1],
            (
                "INSERT INTO [shop].[sales].[users] ([id], [name], [note]) VALUES (3, N'Carol', N'C:\\temp'), (4, N'Dave', N'done');"
                    .to_string(),
                2
            )
        );
    }

    #[test]
    fn stream_import_batches_works_for_json_array_and_json_lines_with_empty_as_null_toggle() {
        let json_file = TempFile::new(
            "json",
            br#"[{"id":10,"label":""},{"id":20,"label":"ok"}]"#,
        );
        let targets = vec![
            target_col("id", false, true, false),
            target_col("label", true, false, false),
        ];
        let mappings = vec![
            ColumnMapping {
                source_index: 0,
                source_name: "id".to_string(),
                target_column: Some("id".to_string()),
                empty_as_null: true,
            },
            ColumnMapping {
                source_index: 1,
                source_name: "label".to_string(),
                target_column: Some("label".to_string()),
                empty_as_null: false,
            },
        ];

        let mut batches = Vec::new();
        let emitted = stream_import_batches(
            json_file.path(),
            ImportFormat::JsonArray,
            SqlDialect::MySql,
            "app",
            "items",
            &mappings,
            &targets,
            10,
            |sql, count| {
                batches.push((sql, count));
                Ok(true)
            },
        )
        .expect("json array stream");

        assert_eq!(emitted, 2);
        assert_eq!(
            batches,
            vec![(
                "INSERT INTO `app`.`items` (`id`, `label`) VALUES (10, ''), (20, 'ok');".to_string(),
                2
            )]
        );

        let jsonl_file = TempFile::new(
            "jsonl",
            b"{\"id\":1,\"label\":\"first\"}\n{\"id\":2,\"label\":null}\n",
        );
        let mut jsonl_batches = Vec::new();
        let emitted_jsonl = stream_import_batches(
            jsonl_file.path(),
            ImportFormat::JsonLines,
            SqlDialect::SqlServer,
            "app",
            "items",
            &mappings,
            &targets,
            10,
            |sql, count| {
                jsonl_batches.push((sql, count));
                Ok(true)
            },
        )
        .expect("jsonl stream");

        assert_eq!(emitted_jsonl, 2);
        assert_eq!(
            jsonl_batches,
            vec![(
                "INSERT INTO [app].[dbo].[items] ([id], [label]) VALUES (1, N'first'), (2, NULL);".to_string(),
                2
            )]
        );
    }
}
