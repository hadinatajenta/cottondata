use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowSelectMode {
    Replace,
    Toggle,
    Extend,
}

impl RowSelectMode {
    pub(super) fn from_modifiers(modifiers: eframe::egui::Modifiers) -> Self {
        if modifiers.shift {
            Self::Extend
        } else if modifiers.command {
            Self::Toggle
        } else {
            Self::Replace
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct RowSelection {
    rows: BTreeSet<usize>,
    anchor: Option<usize>,
}

impl RowSelection {
    pub(super) fn select(&mut self, row: usize, mode: RowSelectMode) {
        match (mode, self.anchor) {
            (RowSelectMode::Extend, Some(anchor)) => {
                self.rows = (anchor.min(row)..=anchor.max(row)).collect();
                return;
            }
            (RowSelectMode::Toggle, _) => {
                if !self.rows.remove(&row) {
                    self.rows.insert(row);
                }
            }
            _ => {
                self.rows.clear();
                self.rows.insert(row);
            }
        }
        self.anchor = Some(row);
    }

    pub(super) fn select_all(&mut self, row_count: usize) {
        self.rows = (0..row_count).collect();
        self.anchor = (row_count > 0).then_some(0);
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.anchor = None;
    }

    pub(super) fn contains(&self, row: usize) -> bool {
        self.rows.contains(&row)
    }

    pub(super) fn sorted_rows_below(&self, row_limit: usize) -> Vec<usize> {
        self.rows.range(..row_limit).copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_keeps_only_clicked_row() {
        let mut selection = RowSelection::default();
        selection.select(3, RowSelectMode::Replace);
        selection.select(5, RowSelectMode::Replace);
        assert_eq!(selection.sorted_rows_below(100), vec![5]);
    }

    #[test]
    fn toggle_adds_and_removes_rows_independently() {
        let mut selection = RowSelection::default();
        selection.select(1, RowSelectMode::Replace);
        selection.select(4, RowSelectMode::Toggle);
        selection.select(1, RowSelectMode::Toggle);
        assert_eq!(selection.sorted_rows_below(100), vec![4]);
    }

    #[test]
    fn extend_selects_inclusive_range_from_anchor_in_either_direction() {
        let mut selection = RowSelection::default();
        selection.select(6, RowSelectMode::Replace);
        selection.select(3, RowSelectMode::Extend);
        assert_eq!(selection.sorted_rows_below(100), vec![3, 4, 5, 6]);
        selection.select(8, RowSelectMode::Extend);
        assert_eq!(selection.sorted_rows_below(100), vec![6, 7, 8]);
    }

    #[test]
    fn extend_without_anchor_behaves_like_replace() {
        let mut selection = RowSelection::default();
        selection.select(2, RowSelectMode::Extend);
        assert_eq!(selection.sorted_rows_below(100), vec![2]);
    }

    #[test]
    fn select_all_then_limit_excludes_staged_rows_beyond_page() {
        let mut selection = RowSelection::default();
        selection.select_all(4);
        selection.select(5, RowSelectMode::Toggle);
        assert_eq!(selection.sorted_rows_below(4), vec![0, 1, 2, 3]);
        selection.clear();
        assert!(!selection.contains(0));
    }
}
