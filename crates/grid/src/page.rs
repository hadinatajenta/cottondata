use cottondata_core::MAX_INLINE_CELL_BYTES;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub table: Option<String>,
    pub schema: Option<String>,
    pub type_name: String,
    pub is_primary_key: bool,
    pub is_nullable: bool,
    pub is_auto_increment: bool,
    pub is_numeric: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellValue {
    Null,
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
            Self::Text(s) => s.as_str(),
            Self::Truncated { preview, .. } => preview.as_str(),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

impl fmt::Display for CellValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("NULL"),
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

    pub fn build_select_sql(&self, schema: &str, table: &str) -> String {
        let mut sql = format!(
            "SELECT * FROM `{}`.`{}`",
            escape_ident(schema),
            escape_ident(table)
        );
        let where_trimmed = self.where_clause.trim();
        if !where_trimmed.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(where_trimmed);
        }
        if let Some((col, dir)) = &self.order_by {
            sql.push_str(" ORDER BY `");
            sql.push_str(&escape_ident(col));
            sql.push('`');
            sql.push_str(match dir {
                SortDirection::Asc => " ASC",
                SortDirection::Desc => " DESC",
            });
        }
        let fetch_limit = self.page_size.max(1) as u64 + 1;
        sql.push_str(&format!(" LIMIT {fetch_limit} OFFSET {}", self.offset()));
        sql
    }

    pub fn build_count_sql(&self, schema: &str, table: &str) -> String {
        let mut sql = format!(
            "SELECT COUNT(*) FROM `{}`.`{}`",
            escape_ident(schema),
            escape_ident(table)
        );
        let where_trimmed = self.where_clause.trim();
        if !where_trimmed.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(where_trimmed);
        }
        sql
    }
}

pub fn escape_ident(ident: &str) -> String {
    ident.replace('`', "``")
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
                CellValue::Null => {
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
                table: Some("users".into()),
                schema: Some("app".into()),
                type_name: "bigint unsigned".into(),
                is_primary_key: true,
                is_nullable: false,
                is_auto_increment: true,
                is_numeric: true,
            },
            ColumnMeta {
                name: "email".into(),
                table: Some("users".into()),
                schema: Some("app".into()),
                type_name: "varchar(255)".into(),
                is_primary_key: false,
                is_nullable: true,
                is_auto_increment: false,
                is_numeric: false,
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
}
