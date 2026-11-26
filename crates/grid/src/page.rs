use cottondata_core::{SqlDialect, MAX_INLINE_CELL_BYTES};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    #[serde(default)]
    pub org_name: Option<String>,
    pub table: Option<String>,
    pub schema: Option<String>,
    pub type_name: String,
    pub is_primary_key: bool,
    #[serde(default)]
    pub is_unique_key: bool,
    pub is_nullable: bool,
    pub is_auto_increment: bool,
    pub is_numeric: bool,
    #[serde(default)]
    pub is_binary: bool,
}

impl ColumnMeta {
    pub fn sql_column_name(&self) -> &str {
        self.org_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.name)
    }

    pub fn is_computed(&self) -> bool {
        self.table.is_some() && self.org_name.as_deref().is_some_and(str::is_empty)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellValue {
    Null,
    DefaultExpr,
    Text(String),
    Truncated { preview: String, total_bytes: usize },
}

impl CellValue {
    pub fn from_text(mut s: String) -> Self {
        if s.len() > MAX_INLINE_CELL_BYTES {
            let total_bytes = s.len();
            let mut end = MAX_INLINE_CELL_BYTES;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            Self::Truncated {
                preview: s,
                total_bytes,
            }
        } else {
            Self::Text(s)
        }
    }

    pub fn as_display_str(&self) -> &str {
        match self {
            Self::Null => "NULL",
            Self::DefaultExpr => "DEFAULT",
            Self::Text(s) => s.as_str(),
            Self::Truncated { preview, .. } => preview.as_str(),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Self::DefaultExpr)
    }

    pub fn is_truncated(&self) -> bool {
        matches!(self, Self::Truncated { .. })
    }
}

impl fmt::Display for CellValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("NULL"),
            Self::DefaultExpr => f.write_str("DEFAULT"),
            Self::Text(s) => f.write_str(s),
            Self::Truncated { preview, total_bytes } => {
                write!(f, "{preview}… [{total_bytes} B]")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysetSeek {
    pub column: String,
    pub direction: SortDirection,
    pub last_value_sql: String,
}

pub fn format_keyset_literal(dialect: SqlDialect, column: &ColumnMeta, raw: &str) -> Option<String> {
    if column.is_binary {
        return None;
    }
    if column.is_numeric {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let is_valid_number = trimmed.parse::<i128>().is_ok()
            || trimmed.parse::<u128>().is_ok()
            || trimmed.parse::<f64>().is_ok_and(|value| value.is_finite());
        return is_valid_number.then(|| trimmed.to_string());
    }
    Some(dialect.string_literal(raw))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageFilterSort {
    pub where_clause: String,
    pub order_by: Option<(String, SortDirection)>,
    pub page_index: u64,
    pub page_size: u32,
}

impl PageFilterSort {
    pub fn offset(&self) -> u64 {
        self.page_index.saturating_mul(self.page_size.max(1) as u64)
    }

    pub fn build_select_sql(&self, dialect: SqlDialect, schema: &str, table: &str) -> String {
        self.build_select_sql_with_default_order(dialect, schema, table, None)
    }

    pub fn build_select_sql_with_default_order(
        &self,
        dialect: SqlDialect,
        schema: &str,
        table: &str,
        default_order_column: Option<&str>,
    ) -> String {
        let fetch_limit = self.page_size.max(1).saturating_add(1);
        self.build_offset_select_sql(dialect, schema, table, default_order_column, fetch_limit, self.offset())
    }

    pub fn build_offset_select_sql(
        &self,
        dialect: SqlDialect,
        schema: &str,
        table: &str,
        default_order_column: Option<&str>,
        limit: u32,
        offset: u64,
    ) -> String {
        let order_by = self
            .order_by
            .as_ref()
            .map(|(column, direction)| {
                let direction_sql = match direction {
                    SortDirection::Asc => "ASC",
                    SortDirection::Desc => "DESC",
                };
                format!("{} {direction_sql}", dialect.quote_ident(column))
            })
            .or_else(|| {
                default_order_column
                    .filter(|column| !column.is_empty())
                    .map(|column| format!("{} ASC", dialect.quote_ident(column)))
            });
        dialect.paged_select(
            &dialect.qualified_table(schema, table),
            self.where_filter(),
            order_by.as_deref(),
            limit.max(1),
            offset,
        )
    }

    pub fn build_keyset_select_sql(
        &self,
        dialect: SqlDialect,
        schema: &str,
        table: &str,
        seek: &KeysetSeek,
        limit: u32,
    ) -> String {
        let quoted_column = dialect.quote_ident(&seek.column);
        let (op, direction_sql) = match seek.direction {
            SortDirection::Asc => (">", "ASC"),
            SortDirection::Desc => ("<", "DESC"),
        };
        let seek_predicate = format!("{quoted_column} {op} {}", seek.last_value_sql);
        let combined_where = match self.where_filter() {
            Some(user_where) => format!("({user_where}) AND ({seek_predicate})"),
            None => seek_predicate,
        };
        let order_by = format!("{quoted_column} {direction_sql}");
        dialect.paged_select(
            &dialect.qualified_table(schema, table),
            Some(&combined_where),
            Some(&order_by),
            limit.max(1),
            0,
        )
    }

    pub fn build_count_sql(&self, dialect: SqlDialect, schema: &str, table: &str) -> String {
        dialect.count_rows(&dialect.qualified_table(schema, table), self.where_filter())
    }

    fn where_filter(&self) -> Option<&str> {
        Some(self.where_clause.trim()).filter(|clause| !clause.is_empty())
    }
}

#[derive(Debug, Clone, Copy)]
struct ArenaSlice {
    offset: u32,
    len: u32,
    truncated_total: u32,
}

#[derive(Debug, Clone)]
pub struct ColumnarPage {
    pub columns: Vec<ColumnMeta>,
    pub row_count: usize,
    pub has_more_rows: bool,
    pub total_rows_known: Option<u64>,
    pub elapsed_ms: u64,
    pub executed_sql: String,
    null_bitmap: Vec<u64>,
    cells: Vec<Vec<ArenaSlice>>,
    text_arena: Vec<u8>,
}

impl ColumnarPage {
    pub fn new(columns: Vec<ColumnMeta>, executed_sql: String) -> Self {
        let col_count = columns.len();
        Self {
            columns,
            row_count: 0,
            has_more_rows: false,
            total_rows_known: None,
            elapsed_ms: 0,
            executed_sql,
            null_bitmap: Vec::new(),
            cells: (0..col_count).map(|_| Vec::new()).collect(),
            text_arena: Vec::new(),
        }
    }

    pub fn push_row(&mut self, values: &[CellValue]) {
        let col_count = self.columns.len();
        let row_idx = self.row_count;
        let needed_bits = (row_idx + 1) * col_count;
        let needed_words = needed_bits.div_ceil(64);
        if self.null_bitmap.len() < needed_words {
            self.null_bitmap.resize(needed_words, 0);
        }

        for (col_idx, col_vec) in self.cells.iter_mut().enumerate() {
            let val = values.get(col_idx).unwrap_or(&CellValue::Null);
            let bit_index = row_idx * col_count + col_idx;
            match val {
                CellValue::Null | CellValue::DefaultExpr => {
                    self.null_bitmap[bit_index / 64] |= 1u64 << (bit_index % 64);
                    col_vec.push(ArenaSlice {
                        offset: 0,
                        len: 0,
                        truncated_total: 0,
                    });
                }
                CellValue::Text(s) => {
                    let bytes = s.as_bytes();
                    let offset = self.text_arena.len() as u32;
                    let capped_len = bytes.len().min(MAX_INLINE_CELL_BYTES);
                    let safe_len = clamp_utf8_boundary(s, capped_len);
                    self.text_arena.extend_from_slice(&bytes[..safe_len]);
                    let truncated_total = if bytes.len() > safe_len {
                        bytes.len() as u32
                    } else {
                        0
                    };
                    col_vec.push(ArenaSlice {
                        offset,
                        len: safe_len as u32,
                        truncated_total,
                    });
                }
                CellValue::Truncated { preview, total_bytes } => {
                    let bytes = preview.as_bytes();
                    let offset = self.text_arena.len() as u32;
                    self.text_arena.extend_from_slice(bytes);
                    col_vec.push(ArenaSlice {
                        offset,
                        len: bytes.len() as u32,
                        truncated_total: (*total_bytes).min(u32::MAX as usize) as u32,
                    });
                }
            }
        }

        self.row_count += 1;
    }

    pub fn is_null(&self, row: usize, col: usize) -> bool {
        if row >= self.row_count || col >= self.columns.len() {
            return true;
        }
        let bit_index = row * self.columns.len() + col;
        let word = self.null_bitmap.get(bit_index / 64).copied().unwrap_or(0);
        (word & (1u64 << (bit_index % 64))) != 0
    }

    pub fn cell_str(&self, row: usize, col: usize) -> Option<&str> {
        if self.is_null(row, col) {
            return None;
        }
        let slice = self.cells.get(col)?.get(row)?;
        let start = slice.offset as usize;
        let end = start + slice.len as usize;
        std::str::from_utf8(self.text_arena.get(start..end)?).ok()
    }

    pub fn cell_truncated_bytes(&self, row: usize, col: usize) -> Option<usize> {
        let slice = self.cells.get(col)?.get(row)?;
        if slice.truncated_total > 0 {
            Some(slice.truncated_total as usize)
        } else {
            None
        }
    }

    pub fn cell_value(&self, row: usize, col: usize) -> CellValue {
        if self.is_null(row, col) {
            return CellValue::Null;
        }
        let text = self.cell_str(row, col).unwrap_or_default().to_string();
        if let Some(total_bytes) = self.cell_truncated_bytes(row, col) {
            CellValue::Truncated {
                preview: text,
                total_bytes,
            }
        } else {
            CellValue::Text(text)
        }
    }

    pub fn append_page(&mut self, other: &Self) -> Result<usize, &'static str> {
        let col_count = self.columns.len();
        if col_count != other.columns.len() {
            return Err("Result column count changed while loading more rows");
        }
        if other.row_count == 0 {
            self.has_more_rows = false;
            self.elapsed_ms = other.elapsed_ms;
            return Ok(0);
        }
        let base_arena_offset = u32::try_from(self.text_arena.len())
            .map_err(|_| "In-memory result text buffer limit reached")?;
        let new_total_arena = self.text_arena.len().saturating_add(other.text_arena.len());
        if u32::try_from(new_total_arena).is_err() {
            return Err("In-memory result text buffer limit reached");
        }

        let old_row_count = self.row_count;
        let new_row_count = old_row_count + other.row_count;
        let needed_bits = new_row_count * col_count;
        let needed_words = needed_bits.div_ceil(64);
        if self.null_bitmap.len() < needed_words {
            self.null_bitmap.resize(needed_words, 0);
        }

        self.text_arena.extend_from_slice(&other.text_arena);
        for (col_idx, dst_col) in self.cells.iter_mut().enumerate() {
            let src_col = &other.cells[col_idx];
            dst_col.reserve(src_col.len());
            for (src_row, slice) in src_col.iter().enumerate() {
                if other.is_null(src_row, col_idx) {
                    let dst_bit = (old_row_count + src_row) * col_count + col_idx;
                    self.null_bitmap[dst_bit / 64] |= 1u64 << (dst_bit % 64);
                    dst_col.push(ArenaSlice {
                        offset: 0,
                        len: 0,
                        truncated_total: 0,
                    });
                } else {
                    dst_col.push(ArenaSlice {
                        offset: slice.offset + base_arena_offset,
                        len: slice.len,
                        truncated_total: slice.truncated_total,
                    });
                }
            }
        }

        self.row_count = new_row_count;
        self.has_more_rows = other.has_more_rows;
        self.elapsed_ms = other.elapsed_ms;
        Ok(other.row_count)
    }

    pub fn approx_bytes(&self) -> usize {
        let slices_bytes: usize = self
            .cells
            .iter()
            .map(|c| c.capacity() * std::mem::size_of::<ArenaSlice>())
            .sum();
        let bitmap_bytes = self.null_bitmap.capacity() * std::mem::size_of::<u64>();
        let meta_bytes = self.columns.len() * 96;
        self.text_arena.capacity() + slices_bytes + bitmap_bytes + meta_bytes + self.executed_sql.capacity()
    }

    pub fn shrink_to_fit(&mut self) {
        for col in &mut self.columns {
            col.name.shrink_to_fit();
            if let Some(s) = &mut col.org_name {
                s.shrink_to_fit();
            }
            if let Some(s) = &mut col.table {
                s.shrink_to_fit();
            }
            if let Some(s) = &mut col.schema {
                s.shrink_to_fit();
            }
            col.type_name.shrink_to_fit();
        }
        self.columns.shrink_to_fit();
        self.executed_sql.shrink_to_fit();
        self.null_bitmap.shrink_to_fit();
        self.cells.shrink_to_fit();
        for col in &mut self.cells {
            col.shrink_to_fit();
        }
        self.text_arena.shrink_to_fit();
    }

    pub fn sorted_row_indices(&self, col_idx: usize, direction: SortDirection) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.row_count).collect();
        if col_idx >= self.columns.len() || self.row_count <= 1 {
            return indices;
        }
        if self.columns[col_idx].is_numeric {
            let keys: Vec<Option<SortNumber<'_>>> = (0..self.row_count)
                .map(|row| self.cell_str(row, col_idx).map(parse_sort_number))
                .collect();
            indices.sort_by(|&a, &b| match (&keys[a], &keys[b]) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(ka), Some(kb)) => {
                    let ord = cmp_sort_number(ka, kb);
                    match direction {
                        SortDirection::Asc => ord,
                        SortDirection::Desc => ord.reverse(),
                    }
                }
            });
        } else {
            indices.sort_by(|&a, &b| match (self.cell_str(a, col_idx), self.cell_str(b, col_idx)) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(va), Some(vb)) => {
                    let ord = cmp_ascii_ignore_case(va, vb);
                    match direction {
                        SortDirection::Asc => ord,
                        SortDirection::Desc => ord.reverse(),
                    }
                }
            });
        }
        indices
    }

    pub fn reorder_rows(&mut self, permutation: &[usize]) {
        if permutation.len() != self.row_count {
            return;
        }
        let mut seen = vec![false; self.row_count];
        for &old_row in permutation {
            if old_row >= self.row_count || seen[old_row] {
                return;
            }
            seen[old_row] = true;
        }
        if self.row_count <= 1 {
            self.shrink_to_fit();
            return;
        }

        let col_count = self.columns.len();
        let needed_words = (self.row_count * col_count).div_ceil(64);
        let mut new_null_bitmap = vec![0u64; needed_words];
        let mut new_cells: Vec<Vec<ArenaSlice>> = (0..col_count)
            .map(|_| Vec::with_capacity(self.row_count))
            .collect();
        let mut new_text_arena = Vec::with_capacity(self.text_arena.len());

        for (new_row, &old_row) in permutation.iter().enumerate() {
            for (col_idx, dst_col) in new_cells.iter_mut().enumerate() {
                if self.is_null(old_row, col_idx) {
                    let dst_bit = new_row * col_count + col_idx;
                    new_null_bitmap[dst_bit / 64] |= 1u64 << (dst_bit % 64);
                    dst_col.push(ArenaSlice {
                        offset: 0,
                        len: 0,
                        truncated_total: 0,
                    });
                } else {
                    let old_slice = self.cells[col_idx][old_row];
                    let start = old_slice.offset as usize;
                    let end = start + old_slice.len as usize;
                    let new_offset = new_text_arena.len() as u32;
                    new_text_arena.extend_from_slice(&self.text_arena[start..end]);
                    dst_col.push(ArenaSlice {
                        offset: new_offset,
                        len: old_slice.len,
                        truncated_total: old_slice.truncated_total,
                    });
                }
            }
        }

        self.null_bitmap = new_null_bitmap;
        self.cells = new_cells;
        self.text_arena = new_text_arena;
        self.shrink_to_fit();
    }

    pub fn sort_by_column(&mut self, col_idx: usize, direction: SortDirection) {
        if col_idx >= self.columns.len() {
            return;
        }
        let permutation = self.sorted_row_indices(col_idx, direction);
        self.reorder_rows(&permutation);
    }
}

enum SortNumber<'a> {
    Int(i128),
    Float(f64),
    Text(&'a str),
}

fn parse_sort_number(raw: &str) -> SortNumber<'_> {
    let trimmed = raw.trim();
    if let Ok(value) = trimmed.parse::<i128>() {
        return SortNumber::Int(value);
    }
    if let Ok(value) = trimmed.parse::<f64>() {
        if value.is_finite() {
            let normalized = if value == 0.0 { 0.0 } else { value };
            return SortNumber::Float(normalized);
        }
    }
    SortNumber::Text(raw)
}

fn cmp_i128_f64(i: i128, f: f64) -> Ordering {
    const TWO_127: f64 = (1u128 << 127) as f64;
    if f < -TWO_127 {
        return Ordering::Greater;
    }
    if f >= TWO_127 {
        return Ordering::Less;
    }
    let trunc = f.trunc();
    let f_int = trunc as i128;
    match i.cmp(&f_int) {
        Ordering::Equal => {
            let frac = f - trunc;
            if frac > 0.0 {
                Ordering::Less
            } else if frac < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        other => other,
    }
}

fn cmp_sort_number(a: &SortNumber<'_>, b: &SortNumber<'_>) -> Ordering {
    match (a, b) {
        (SortNumber::Int(x), SortNumber::Int(y)) => x.cmp(y),
        (SortNumber::Float(x), SortNumber::Float(y)) => x.total_cmp(y),
        (SortNumber::Int(x), SortNumber::Float(y)) => cmp_i128_f64(*x, *y),
        (SortNumber::Float(x), SortNumber::Int(y)) => cmp_i128_f64(*y, *x).reverse(),
        (SortNumber::Int(_) | SortNumber::Float(_), SortNumber::Text(_)) => Ordering::Less,
        (SortNumber::Text(_), SortNumber::Int(_) | SortNumber::Float(_)) => Ordering::Greater,
        (SortNumber::Text(x), SortNumber::Text(y)) => cmp_ascii_ignore_case(x, y),
    }
}

fn cmp_ascii_ignore_case(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(b.bytes().map(|byte| byte.to_ascii_lowercase()))
}

fn clamp_utf8_boundary(s: &str, max_len: usize) -> usize {
    let mut end = max_len.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cols() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta {
                name: "id".into(),
                org_name: Some("id".into()),
                table: Some("users".into()),
                schema: Some("app".into()),
                type_name: "bigint unsigned".into(),
                is_primary_key: true,
                is_unique_key: false,
                is_nullable: false,
                is_auto_increment: true,
                is_numeric: true,
                is_binary: false,
            },
            ColumnMeta {
                name: "email".into(),
                org_name: Some("email".into()),
                table: Some("users".into()),
                schema: Some("app".into()),
                type_name: "varchar(255)".into(),
                is_primary_key: false,
                is_unique_key: false,
                is_nullable: true,
                is_auto_increment: false,
                is_numeric: false,
                is_binary: false,
            },
        ]
    }

    #[test]
    fn stores_rows_in_compact_arena_with_null_bitmap_and_truncation() {
        let mut page = ColumnarPage::new(sample_cols(), "SELECT * FROM users".into());
        page.push_row(&[CellValue::Text("1".into()), CellValue::Text("ada@example.com".into())]);
        page.push_row(&[CellValue::Text("2".into()), CellValue::Null]);
        let giant = "x".repeat(MAX_INLINE_CELL_BYTES + 250);
        page.push_row(&[CellValue::Text("3".into()), CellValue::from_text(giant)]);

        assert_eq!(page.row_count, 3);
        assert_eq!(page.cell_str(0, 1), Some("ada@example.com"));
        assert!(page.is_null(1, 1));
        assert_eq!(page.cell_str(1, 1), None);
        assert_eq!(page.cell_truncated_bytes(2, 1), Some(MAX_INLINE_CELL_BYTES + 250));
        assert!(page.approx_bytes() < 16 * 1024);
    }

    #[test]
    fn appends_pages_preserving_null_bitmap_truncation_and_arena_offsets() {
        let mut first = ColumnarPage::new(sample_cols(), "SELECT * FROM users".into());
        for i in 0..40 {
            let email = if i % 3 == 0 {
                CellValue::Null
            } else {
                CellValue::Text(format!("user{i}@example.com"))
            };
            first.push_row(&[CellValue::Text(i.to_string()), email]);
        }
        first.has_more_rows = true;

        let mut second = ColumnarPage::new(sample_cols(), "SELECT * FROM users LIMIT 10 OFFSET 40".into());
        let giant = "z".repeat(MAX_INLINE_CELL_BYTES + 42);
        second.push_row(&[CellValue::Text("40".into()), CellValue::Null]);
        second.push_row(&[CellValue::Text("41".into()), CellValue::from_text(giant)]);
        second.push_row(&[CellValue::Text("42".into()), CellValue::Text("ada@example.com".into())]);
        second.has_more_rows = false;
        second.elapsed_ms = 7;

        let added = first.append_page(&second).unwrap();
        assert_eq!(added, 3);
        assert_eq!(first.row_count, 43);
        assert!(!first.has_more_rows);
        assert_eq!(first.elapsed_ms, 7);
        assert!(first.is_null(0, 1));
        assert_eq!(first.cell_str(1, 1), Some("user1@example.com"));
        assert!(first.is_null(40, 1));
        assert_eq!(first.cell_truncated_bytes(41, 1), Some(MAX_INLINE_CELL_BYTES + 42));
        assert_eq!(first.cell_str(42, 1), Some("ada@example.com"));

        let wrong_cols = ColumnarPage::new(vec![sample_cols().remove(0)], "SELECT id FROM users".into());
        assert!(first.append_page(&wrong_cols).is_err());
    }

    #[test]
    fn builds_keyset_seek_queries_with_and_without_user_where() {
        let filter = PageFilterSort {
            where_clause: "active = 1".into(),
            order_by: None,
            page_index: 0,
            page_size: 100,
        };
        let seek = KeysetSeek {
            column: "id".into(),
            direction: SortDirection::Asc,
            last_value_sql: "100".into(),
        };
        assert_eq!(
            filter.build_keyset_select_sql(SqlDialect::MySql, "app", "users", &seek, 101),
            "SELECT * FROM `app`.`users` WHERE (active = 1) AND (`id` > 100) ORDER BY `id` ASC LIMIT 101 OFFSET 0"
        );
        let desc_seek = KeysetSeek {
            column: "id".into(),
            direction: SortDirection::Desc,
            last_value_sql: "'usr_99'".into(),
        };
        let empty_filter = PageFilterSort {
            where_clause: String::new(),
            order_by: None,
            page_index: 0,
            page_size: 100,
        };
        assert_eq!(
            empty_filter.build_keyset_select_sql(SqlDialect::PostgreSql, "public", "users", &desc_seek, 101),
            "SELECT * FROM \"public\".\"users\" WHERE \"id\" < 'usr_99' ORDER BY \"id\" DESC LIMIT 101 OFFSET 0"
        );
        assert_eq!(
            format_keyset_literal(SqlDialect::MySql, &sample_cols()[0], "42"),
            Some("42".into())
        );
        assert_eq!(
            format_keyset_literal(SqlDialect::MySql, &sample_cols()[1], "a'b"),
            Some("'a''b'".into())
        );
    }

    #[test]
    fn sorts_numeric_columns_with_large_ints_negatives_floats_and_nulls_last() {
        let mut page = ColumnarPage::new(sample_cols(), "SELECT id, email FROM users".into());
        let rows = [
            (CellValue::Text("10".into()), CellValue::Text("r0".into())),
            (CellValue::Null, CellValue::Text("r1".into())),
            (CellValue::Text("2".into()), CellValue::Text("r2".into())),
            (CellValue::Text("-5.5".into()), CellValue::Text("r3".into())),
            (CellValue::Text("-5".into()), CellValue::Text("r4".into())),
            (CellValue::Text("9007199254740993".into()), CellValue::Text("r5".into())),
            (CellValue::Text("9007199254740992".into()), CellValue::Text("r6".into())),
            (CellValue::Text("9007199254740992.0".into()), CellValue::Text("r7".into())),
            (CellValue::Text("2.5".into()), CellValue::Text("r8".into())),
            (CellValue::Null, CellValue::Text("r9".into())),
        ];
        for (id, tag) in rows {
            page.push_row(&[id, tag]);
        }

        let asc = page.sorted_row_indices(0, SortDirection::Asc);
        let asc_ids: Vec<Option<&str>> = asc.iter().map(|&r| page.cell_str(r, 0)).collect();
        assert_eq!(
            asc_ids,
            vec![
                Some("-5.5"),
                Some("-5"),
                Some("2"),
                Some("2.5"),
                Some("10"),
                Some("9007199254740992"),
                Some("9007199254740992.0"),
                Some("9007199254740993"),
                None,
                None,
            ]
        );
        assert_eq!(page.cell_str(asc[8], 1), Some("r1"));
        assert_eq!(page.cell_str(asc[9], 1), Some("r9"));

        let desc = page.sorted_row_indices(0, SortDirection::Desc);
        let desc_ids: Vec<Option<&str>> = desc.iter().map(|&r| page.cell_str(r, 0)).collect();
        assert_eq!(
            desc_ids,
            vec![
                Some("9007199254740993"),
                Some("9007199254740992"),
                Some("9007199254740992.0"),
                Some("10"),
                Some("2.5"),
                Some("2"),
                Some("-5"),
                Some("-5.5"),
                None,
                None,
            ]
        );
    }

    #[test]
    fn sorts_text_case_insensitively_stably_and_compacts_buffers() {
        let mut page = ColumnarPage::new(sample_cols(), "SELECT id, email FROM users".into());
        let giant = "m".repeat(MAX_INLINE_CELL_BYTES + 128);
        page.push_row(&[CellValue::Text("1".into()), CellValue::Text("charlie".into())]);
        page.push_row(&[CellValue::Text("2".into()), CellValue::Null]);
        page.push_row(&[CellValue::Text("3".into()), CellValue::Text("Alpha".into())]);
        page.push_row(&[CellValue::Text("4".into()), CellValue::Text("alpha".into())]);
        page.push_row(&[CellValue::Text("5".into()), CellValue::from_text(giant)]);
        page.push_row(&[CellValue::Text("6".into()), CellValue::Text("Beta".into())]);

        let bytes_before = page.approx_bytes();
        page.sort_by_column(1, SortDirection::Asc);
        let emails_asc: Vec<Option<&str>> = (0..page.row_count).map(|r| page.cell_str(r, 1)).collect();
        let ids_asc: Vec<Option<&str>> = (0..page.row_count).map(|r| page.cell_str(r, 0)).collect();
        assert_eq!(ids_asc, vec![Some("3"), Some("4"), Some("6"), Some("1"), Some("5"), Some("2")]);
        assert_eq!(emails_asc[0], Some("Alpha"));
        assert_eq!(emails_asc[1], Some("alpha"));
        assert_eq!(emails_asc[2], Some("Beta"));
        assert_eq!(emails_asc[3], Some("charlie"));
        assert_eq!(page.cell_truncated_bytes(4, 1), Some(MAX_INLINE_CELL_BYTES + 128));
        assert!(page.is_null(5, 1));
        assert!(page.approx_bytes() <= bytes_before);

        page.sort_by_column(1, SortDirection::Desc);
        let expected_desc = vec![Some("5"), Some("1"), Some("6"), Some("3"), Some("4"), Some("2")];
        let ids_desc: Vec<Option<&str>> = (0..page.row_count).map(|r| page.cell_str(r, 0)).collect();
        assert_eq!(ids_desc, expected_desc);
        assert!(page.is_null(5, 1));

        page.reorder_rows(&[0, 0, 1, 2, 3, 4]);
        let ids_unchanged: Vec<Option<&str>> = (0..page.row_count).map(|r| page.cell_str(r, 0)).collect();
        assert_eq!(ids_unchanged, expected_desc);
    }
}
