use cottondata_grid::ColumnarPage;

#[derive(Debug, Clone, Default)]
pub struct GridFilterState {
    pub open: bool,
    pub query: String,
    pub focus_input: bool,
    pub cached_generation: u64,
    pub cached_query: String,
    pub matched_rows: Vec<usize>,
}

impl GridFilterState {
    pub fn ensure_synced<'a, F>(&mut self, generation: u64, page: &ColumnarPage, edited_cell: F)
    where
        F: Fn(usize, usize) -> Option<&'a str>,
    {
        if self.cached_generation == generation && self.cached_query == self.query {
            return;
        }
        self.matched_rows = compute_filtered_rows_with_edits(page, &self.query, edited_cell);
        self.cached_generation = generation;
        self.cached_query.clear();
        self.cached_query.push_str(&self.query);
    }

    #[allow(dead_code)]
    pub fn invalidate(&mut self) {
        self.cached_generation = u64::MAX;
        self.cached_query.clear();
    }
}

#[allow(dead_code)]
pub fn compute_filtered_rows(page: &ColumnarPage, query: &str) -> Vec<usize> {
    compute_filtered_rows_with_edits(page, query, |_, _| None)
}

pub fn compute_filtered_rows_with_edits<'a, F>(
    page: &ColumnarPage,
    query: &str,
    edited_cell: F,
) -> Vec<usize>
where
    F: Fn(usize, usize) -> Option<&'a str>,
{
    let needle = query.trim();
    if needle.is_empty() {
        return (0..page.row_count).collect();
    }

    let col_count = page.columns.len();
    let mut matched = Vec::new();

    if needle.is_ascii() {
        let needle_lower: Vec<u8> = needle.bytes().map(|b| b.to_ascii_lowercase()).collect();
        for row in 0..page.row_count {
            for col in 0..col_count {
                let cell_text = match edited_cell(row, col) {
                    Some(override_text) => Some(override_text),
                    None => page.cell_str(row, col),
                };
                if let Some(text) = cell_text {
                    if contains_ascii_case_insensitive(text.as_bytes(), &needle_lower) {
                        matched.push(row);
                        break;
                    }
                }
            }
        }
    } else {
        let needle_lower: Vec<char> = needle.to_lowercase().chars().collect();
        for row in 0..page.row_count {
            for col in 0..col_count {
                let cell_text = match edited_cell(row, col) {
                    Some(override_text) => Some(override_text),
                    None => page.cell_str(row, col),
                };
                if let Some(text) = cell_text {
                    if contains_unicode_case_insensitive(text, &needle_lower) {
                        matched.push(row);
                        break;
                    }
                }
            }
        }
    }

    matched
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle_lower: &[u8]) -> bool {
    let n_len = needle_lower.len();
    if n_len == 0 {
        return true;
    }
    if haystack.len() < n_len {
        return false;
    }
    let first = needle_lower[0];
    let limit = haystack.len() - n_len;
    let mut i = 0;
    while i <= limit {
        if haystack[i].to_ascii_lowercase() == first {
            let mut all_match = true;
            for j in 1..n_len {
                if haystack[i + j].to_ascii_lowercase() != needle_lower[j] {
                    all_match = false;
                    break;
                }
            }
            if all_match {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn contains_unicode_case_insensitive(haystack: &str, needle_lower: &[char]) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    for (byte_idx, _) in haystack.char_indices() {
        let mut h_lower = haystack[byte_idx..].chars().flat_map(|c| c.to_lowercase());
        let mut matched = true;
        for &target in needle_lower {
            if h_lower.next() != Some(target) {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use cottondata_grid::{CellValue, ColumnMeta};

    fn sample_page() -> ColumnarPage {
        let cols = vec![
            ColumnMeta {
                name: "id".into(),
                org_name: Some("id".into()),
                table: Some("users".into()),
                schema: Some("app".into()),
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
                schema: Some("app".into()),
                type_name: "varchar(100)".into(),
                is_primary_key: false,
                is_unique_key: false,
                is_nullable: true,
                is_auto_increment: false,
                is_numeric: false,
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
        ];
        let mut page = ColumnarPage::new(cols, "SELECT * FROM users".into());
        page.push_row(&[
            CellValue::Text("1".into()),
            CellValue::Text("Alice Lovelace".into()),
            CellValue::Text("alice@example.com".into()),
        ]);
        page.push_row(&[
            CellValue::Text("2".into()),
            CellValue::Null,
            CellValue::Text("bob@cotton.dev".into()),
        ]);
        page.push_row(&[
            CellValue::Text("3".into()),
            CellValue::Text("Café Owner".into()),
            CellValue::Null,
        ]);
        page
    }

    #[test]
    fn empty_query_returns_all_rows() {
        let page = sample_page();
        assert_eq!(compute_filtered_rows(&page, ""), vec![0, 1, 2]);
        assert_eq!(compute_filtered_rows(&page, "   "), vec![0, 1, 2]);
    }

    #[test]
    fn matches_case_insensitively_across_ascii_and_unicode() {
        let page = sample_page();
        assert_eq!(compute_filtered_rows(&page, "ALICE"), vec![0]);
        assert_eq!(compute_filtered_rows(&page, "cotton.DEV"), vec![1]);
        assert_eq!(compute_filtered_rows(&page, "CAFÉ"), vec![2]);
    }

    #[test]
    fn skips_null_cells_when_filtering() {
        let page = sample_page();
        assert_eq!(compute_filtered_rows(&page, "null"), Vec::<usize>::new());
    }

    #[test]
    fn respects_edited_cell_overrides() {
        let page = sample_page();
        let rows_for_replaced = compute_filtered_rows_with_edits(&page, "alice", |row, col| {
            if row == 0 && (col == 1 || col == 2) {
                Some("charlie")
            } else {
                None
            }
        });
        assert!(rows_for_replaced.is_empty());

        let rows_for_new = compute_filtered_rows_with_edits(&page, "CHARLIE", |row, col| {
            if row == 1 && col == 1 {
                Some("Charlie Pending")
            } else {
                None
            }
        });
        assert_eq!(rows_for_new, vec![1]);
    }
}
