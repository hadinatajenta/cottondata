use cottondata_core::SqlDialect;
use cottondata_sql::{
    compute_fold_regions, compute_line_spans, gutter_fold_regions, innermost_region_at_line,
    line_index_for_byte, lint_sql, FoldKind, FoldRegion, SchemaTableHint, SqlDiagnostic,
};
use eframe::egui::TextBuffer;
use std::collections::BTreeSet;
use std::ops::Range;

const MAX_UNDO_SNAPSHOTS: usize = 120;
const UNDO_COALESCE_WINDOW_SECS: f64 = 0.65;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueryExecOutcome {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct QueryExecIndicator {
    pub(super) start_line: usize,
    pub(super) outcome: QueryExecOutcome,
    pub(super) summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UndoSnapshot {
    pub(super) sql: String,
    pub(super) cursor_byte: usize,
    pub(super) selection: Option<(usize, usize)>,
}

#[derive(Clone, Default)]
pub(super) struct SqlUndoStack {
    undo: Vec<UndoSnapshot>,
    redo: Vec<UndoSnapshot>,
    last_edit_time: f64,
}

impl SqlUndoStack {
    pub(super) fn record_before_edit(
        &mut self,
        sql: &str,
        cursor_byte: usize,
        selection: Option<(usize, usize)>,
        now_secs: f64,
        force_new_group: bool,
    ) {
        let should_push = force_new_group
            || self.undo.is_empty()
            || (now_secs - self.last_edit_time) >= UNDO_COALESCE_WINDOW_SECS;

        self.last_edit_time = now_secs;
        self.redo.clear();

        if !should_push {
            return;
        }
        if self.undo.last().is_some_and(|last| last.sql == sql) {
            return;
        }
        self.undo.push(UndoSnapshot {
            sql: sql.to_string(),
            cursor_byte,
            selection,
        });
        if self.undo.len() > MAX_UNDO_SNAPSHOTS {
            self.undo.remove(0);
        }
    }

    pub(super) fn undo(
        &mut self,
        current_sql: &str,
        current_cursor: usize,
        current_selection: Option<(usize, usize)>,
    ) -> Option<UndoSnapshot> {
        let prev = self.undo.pop()?;
        self.redo.push(UndoSnapshot {
            sql: current_sql.to_string(),
            cursor_byte: current_cursor,
            selection: current_selection,
        });
        if self.redo.len() > MAX_UNDO_SNAPSHOTS {
            self.redo.remove(0);
        }
        self.last_edit_time = 0.0;
        Some(prev)
    }

    pub(super) fn redo(
        &mut self,
        current_sql: &str,
        current_cursor: usize,
        current_selection: Option<(usize, usize)>,
    ) -> Option<UndoSnapshot> {
        let next = self.redo.pop()?;
        self.undo.push(UndoSnapshot {
            sql: current_sql.to_string(),
            cursor_byte: current_cursor,
            selection: current_selection,
        });
        if self.undo.len() > MAX_UNDO_SNAPSHOTS {
            self.undo.remove(0);
        }
        self.last_edit_time = 0.0;
        Some(next)
    }
}

#[derive(Clone, Default)]
pub(super) struct SqlFoldState {
    pub(super) collapsed: BTreeSet<usize>,
    cached_sql_hash: u64,
    cached_sql_len: usize,
    cached_dialect: SqlDialect,
    cached_regions: Vec<FoldRegion>,
    cache_populated: bool,
}

impl SqlFoldState {
    pub(super) fn from_saved_lines(lines: &[usize]) -> Self {
        Self {
            collapsed: lines.iter().copied().collect(),
            ..Self::default()
        }
    }

    pub(super) fn saved_lines(&self) -> Vec<usize> {
        self.collapsed.iter().copied().collect()
    }

    pub(super) fn has_collapsed(&self) -> bool {
        !self.collapsed.is_empty()
    }

    pub(super) fn regions<'a>(&'a mut self, sql: &str, dialect: SqlDialect) -> &'a [FoldRegion] {
        self.refresh_cache(sql, dialect);
        &self.cached_regions
    }

    pub(super) fn refresh_cache(&mut self, sql: &str, dialect: SqlDialect) {
        let hash = fnv1a64(sql.as_bytes());
        if self.cache_populated
            && self.cached_sql_hash == hash
            && self.cached_sql_len == sql.len()
            && self.cached_dialect == dialect
        {
            return;
        }
        self.cached_regions = compute_fold_regions(sql, dialect);
        self.cached_sql_hash = hash;
        self.cached_sql_len = sql.len();
        self.cached_dialect = dialect;
        self.cache_populated = true;
        self.prune_against_cached();
    }

    fn prune_against_cached(&mut self) {
        if self.collapsed.is_empty() {
            return;
        }
        let valid_starts: BTreeSet<usize> = self.cached_regions.iter().map(|r| r.start_line).collect();
        self.collapsed.retain(|line| valid_starts.contains(line));
    }

    pub(super) fn toggle_line(&mut self, start_line: usize) {
        if !self.collapsed.remove(&start_line) {
            self.collapsed.insert(start_line);
        }
    }

    pub(super) fn toggle_line_recursive(&mut self, sql: &str, dialect: SqlDialect, start_line: usize) {
        self.refresh_cache(sql, dialect);
        let Some(target) = gutter_fold_regions(&self.cached_regions)
            .into_iter()
            .find(|r| r.start_line == start_line)
            .cloned()
        else {
            return;
        };
        let is_currently_collapsed = self.collapsed.contains(&start_line);
        let child_starts: Vec<usize> = gutter_fold_regions(&self.cached_regions)
            .into_iter()
            .filter(|r| r.start_line >= target.start_line && r.end_line <= target.end_line)
            .map(|r| r.start_line)
            .collect();
        if is_currently_collapsed {
            for line in child_starts {
                self.collapsed.remove(&line);
            }
        } else {
            for line in child_starts {
                self.collapsed.insert(line);
            }
        }
    }

    pub(super) fn fold_at_line(&mut self, sql: &str, dialect: SqlDialect, line: usize) -> bool {
        self.refresh_cache(sql, dialect);
        let mut candidates: Vec<&FoldRegion> = self
            .cached_regions
            .iter()
            .filter(|r| line >= r.start_line && line <= r.end_line && !self.collapsed.contains(&r.start_line))
            .collect();
        candidates.sort_by_key(|r| (r.end_line - r.start_line, std::cmp::Reverse(r.depth)));
        if let Some(region) = candidates.first() {
            self.collapsed.insert(region.start_line);
            return true;
        }
        false
    }

    pub(super) fn unfold_at_line(&mut self, sql: &str, dialect: SqlDialect, line: usize) -> bool {
        self.refresh_cache(sql, dialect);
        if self.collapsed.remove(&line) {
            return true;
        }
        let enclosing = self
            .cached_regions
            .iter()
            .filter(|r| line >= r.start_line && line <= r.end_line && self.collapsed.contains(&r.start_line))
            .min_by_key(|r| r.end_line - r.start_line)
            .map(|r| r.start_line);
        if let Some(start) = enclosing {
            self.collapsed.remove(&start);
            return true;
        }
        if let Some(inner) = innermost_region_at_line(&self.cached_regions, line) {
            return self.collapsed.remove(&inner.start_line);
        }
        false
    }

    pub(super) fn fold_all_queries(&mut self, sql: &str, dialect: SqlDialect) {
        self.refresh_cache(sql, dialect);
        for region in gutter_fold_regions(&self.cached_regions) {
            if matches!(
                region.kind,
                FoldKind::CustomRegion
                    | FoldKind::Statement
                    | FoldKind::ProceduralBlock
                    | FoldKind::DollarQuoted
                    | FoldKind::BlockComment
            ) && region.depth == 0
            {
                self.collapsed.insert(region.start_line);
            }
        }
        if self.collapsed.is_empty() {
            self.fold_all(sql, dialect);
        }
    }

    pub(super) fn fold_all(&mut self, sql: &str, dialect: SqlDialect) {
        self.refresh_cache(sql, dialect);
        for region in gutter_fold_regions(&self.cached_regions) {
            self.collapsed.insert(region.start_line);
        }
    }

    pub(super) fn unfold_all(&mut self) {
        self.collapsed.clear();
    }

    pub(super) fn unfold_enclosing_byte(&mut self, sql: &str, dialect: SqlDialect, doc_byte: usize) -> bool {
        if self.collapsed.is_empty() {
            return false;
        }
        self.refresh_cache(sql, dialect);
        let spans = compute_line_spans(sql);
        let target_line = line_index_for_byte(&spans, doc_byte);
        let to_open: Vec<usize> = self
            .effective_collapsed_from_cache()
            .into_iter()
            .filter(|r| target_line > r.start_line && target_line <= r.end_line)
            .map(|r| r.start_line)
            .collect();
        if to_open.is_empty() {
            return false;
        }
        let mut changed = false;
        for start in to_open {
            changed |= self.collapsed.remove(&start);
        }
        // Also unfold any nested collapsed region that still hides target_line
        let nested: Vec<usize> = self
            .cached_regions
            .iter()
            .filter(|r| {
                self.collapsed.contains(&r.start_line)
                    && target_line > r.start_line
                    && target_line <= r.end_line
            })
            .map(|r| r.start_line)
            .collect();
        for start in nested {
            changed |= self.collapsed.remove(&start);
        }
        changed
    }

    pub(super) fn collapsed_region_starting_at(
        &mut self,
        sql: &str,
        dialect: SqlDialect,
        start_line: usize,
    ) -> Option<FoldRegion> {
        if !self.collapsed.contains(&start_line) {
            return None;
        }
        self.refresh_cache(sql, dialect);
        self.primary_region_at_line(start_line).cloned()
    }

    pub(super) fn primary_region_at_line(&self, start_line: usize) -> Option<&FoldRegion> {
        self.cached_regions.iter().find(|r| r.start_line == start_line)
    }

    pub(super) fn effective_collapsed_regions(&mut self, sql: &str, dialect: SqlDialect) -> Vec<FoldRegion> {
        self.refresh_cache(sql, dialect);
        self.effective_collapsed_from_cache()
    }

    fn effective_collapsed_from_cache(&self) -> Vec<FoldRegion> {
        if self.collapsed.is_empty() {
            return Vec::new();
        }
        let mut selected: Vec<FoldRegion> = Vec::new();
        for &start_line in &self.collapsed {
            if let Some(region) = self.primary_region_at_line(start_line) {
                selected.push(region.clone());
            }
        }
        selected.sort_by(|a, b| a.start_line.cmp(&b.start_line).then_with(|| b.end_line.cmp(&a.end_line)));

        let mut merged: Vec<FoldRegion> = Vec::with_capacity(selected.len());
        for region in selected {
            if let Some(prev) = merged.last() {
                if region.start_line <= prev.end_line {
                    continue;
                }
            }
            merged.push(region);
        }
        merged
    }

    pub(super) fn on_doc_edit(
        &mut self,
        old_sql: &str,
        edit_start: usize,
        deleted_len: usize,
        inserted: &str,
    ) {
        if self.collapsed.is_empty() {
            self.cache_populated = false;
            return;
        }
        let spans = compute_line_spans(old_sql);
        let edit_end = (edit_start + deleted_len).min(old_sql.len());
        let start_line = line_index_for_byte(&spans, edit_start.min(old_sql.len()));
        let end_line = line_index_for_byte(&spans, edit_end);
        let removed_newlines = old_sql
            .get(edit_start.min(old_sql.len())..edit_end)
            .map(|s| s.bytes().filter(|&b| b == b'\n').count())
            .unwrap_or(0);
        let inserted_newlines = inserted.bytes().filter(|&b| b == b'\n').count();
        let line_delta = inserted_newlines as isize - removed_newlines as isize;

        let mut next = BTreeSet::new();
        for &fold_start in &self.collapsed {
            if fold_start < start_line {
                next.insert(fold_start);
            } else if fold_start > end_line {
                let shifted = (fold_start as isize + line_delta).max(0) as usize;
                next.insert(shifted);
            } else if fold_start == start_line && deleted_len == 0 && inserted_newlines == 0 {
                next.insert(fold_start);
            }
        }
        self.collapsed = next;
        self.cache_populated = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleSegment {
    pub doc_start: usize,
    pub doc_end: usize,
    pub display_start: usize,
    pub display_end: usize,
    pub display_char_start: usize,
    pub display_char_end: usize,
}

#[derive(Debug, Clone)]
pub(super) struct FoldBoundary {
    pub(super) header_display_row: usize,
    pub(super) region: FoldRegion,
    pub(super) display_nl_char: usize,
    pub(super) has_next_line: bool,
    pub(super) hidden_doc_start: usize,
    pub(super) hidden_doc_end: usize,
}

#[derive(Debug, Clone)]
pub(super) struct FoldProjection {
    pub(super) display_text: String,
    pub(super) visible_lines: Vec<usize>,
    pub(super) segments: Vec<VisibleSegment>,
    pub(super) boundaries: Vec<FoldBoundary>,
}

impl FoldProjection {
    pub(super) fn build(full_sql: &str, effective_folds: &[FoldRegion]) -> Self {
        let spans = compute_line_spans(full_sql);
        let total_lines = spans.len();

        let mut visible_lines = Vec::with_capacity(total_lines);
        let mut fold_idx = 0usize;
        let mut line = 0usize;
        while line < total_lines {
            visible_lines.push(line);
            if let Some(fold) = effective_folds.get(fold_idx) {
                if fold.start_line == line {
                    line = (fold.end_line + 1).min(total_lines);
                    fold_idx += 1;
                    continue;
                }
            }
            line += 1;
        }

        let mut display_text = String::with_capacity(full_sql.len());
        let mut segments: Vec<VisibleSegment> = Vec::new();
        let mut boundaries: Vec<FoldBoundary> = Vec::new();
        let mut display_chars = 0usize;

        let mut f_idx = 0usize;
        let mut run_doc_start: Option<usize> = None;
        let mut run_display_start = 0usize;
        let mut run_display_char_start = 0usize;

        for (vis_row, &doc_line) in visible_lines.iter().enumerate() {
            let span = spans[doc_line];
            let line_slice = &full_sql[span.start..span.end];
            if run_doc_start.is_none() {
                run_doc_start = Some(span.start);
                run_display_start = display_text.len();
                run_display_char_start = display_chars;
            }
            display_text.push_str(line_slice);
            display_chars += line_slice.chars().count();

            let starts_fold = effective_folds
                .get(f_idx)
                .filter(|f| f.start_line == doc_line)
                .cloned();
            let next_doc_line = visible_lines.get(vis_row + 1).copied();

            if let Some(fold) = starts_fold {
                f_idx += 1;
                segments.push(VisibleSegment {
                    doc_start: run_doc_start.take().unwrap_or(span.start),
                    doc_end: span.end,
                    display_start: run_display_start,
                    display_end: display_text.len(),
                    display_char_start: run_display_char_start,
                    display_char_end: display_chars,
                });

                let display_nl_char = display_chars;
                let has_next_line = next_doc_line.is_some();
                let hidden_doc_end = next_doc_line
                    .map(|nl| spans[nl].start)
                    .unwrap_or(full_sql.len());

                boundaries.push(FoldBoundary {
                    header_display_row: vis_row,
                    region: fold,
                    display_nl_char,
                    has_next_line,
                    hidden_doc_start: span.end,
                    hidden_doc_end,
                });

                if has_next_line {
                    display_text.push('\n');
                    display_chars += 1;
                }
            } else if let Some(next_line) = next_doc_line {
                if next_line == doc_line + 1 {
                    let nl_slice = &full_sql[span.end..spans[next_line].start];
                    display_text.push_str(nl_slice);
                    display_chars += nl_slice.chars().count();
                } else {
                    segments.push(VisibleSegment {
                        doc_start: run_doc_start.take().unwrap_or(span.start),
                        doc_end: span.end,
                        display_start: run_display_start,
                        display_end: display_text.len(),
                        display_char_start: run_display_char_start,
                        display_char_end: display_chars,
                    });
                    display_text.push('\n');
                    display_chars += 1;
                }
            } else {
                segments.push(VisibleSegment {
                    doc_start: run_doc_start.take().unwrap_or(span.start),
                    doc_end: span.end,
                    display_start: run_display_start,
                    display_end: display_text.len(),
                    display_char_start: run_display_char_start,
                    display_char_end: display_chars,
                });
            }
        }

        if segments.is_empty() {
            segments.push(VisibleSegment {
                doc_start: 0,
                doc_end: 0,
                display_start: 0,
                display_end: 0,
                display_char_start: 0,
                display_char_end: 0,
            });
        }

        Self {
            display_text,
            visible_lines,
            segments,
            boundaries,
        }
    }

    pub(super) fn fold_hash(&self) -> u64 {
        const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
        const PRIME: u64 = 0x100000001b3;
        let mut hash = OFFSET_BASIS;
        let mut feed = |v: u64| {
            for b in v.to_le_bytes() {
                hash ^= u64::from(b);
                hash = hash.wrapping_mul(PRIME);
            }
        };
        feed(self.boundaries.len() as u64);
        for b in &self.boundaries {
            feed(b.region.start_line as u64);
            feed(b.region.end_line as u64);
        }
        hash
    }

    pub(super) fn doc_byte_to_display_byte(&self, doc_byte: usize) -> usize {
        for seg in &self.segments {
            if doc_byte < seg.doc_start {
                return seg.display_start;
            }
            if doc_byte <= seg.doc_end {
                return seg.display_start + (doc_byte - seg.doc_start);
            }
        }
        self.display_text.len()
    }

    pub(super) fn doc_byte_to_display_char(&self, doc_byte: usize) -> usize {
        let disp_byte = self.doc_byte_to_display_byte(doc_byte).min(self.display_text.len());
        let mut clamped = disp_byte;
        while clamped > 0 && !self.display_text.is_char_boundary(clamped) {
            clamped -= 1;
        }
        self.display_text[..clamped].chars().count()
    }

    pub(super) fn display_char_to_doc_byte(&self, display_char: usize) -> usize {
        for seg in &self.segments {
            if display_char < seg.display_char_start {
                return seg.doc_start;
            }
            if display_char <= seg.display_char_end {
                let rel_chars = display_char - seg.display_char_start;
                let seg_slice = &self.display_text[seg.display_start..seg.display_end];
                let rel_bytes = char_index_to_byte(seg_slice, rel_chars);
                return seg.doc_start + rel_bytes;
            }
        }
        self.segments.last().map(|s| s.doc_end).unwrap_or(0)
    }

    pub(super) fn display_char_range_to_doc_byte_range(&self, start_char: usize, end_char: usize) -> (usize, usize) {
        let lo = start_char.min(end_char);
        let hi = start_char.max(end_char);
        let doc_lo = self.display_char_to_doc_byte(lo);
        let doc_hi = self.display_char_to_doc_byte(hi);
        (doc_lo, doc_hi.max(doc_lo))
    }

    pub(super) fn map_doc_ranges_to_display_bytes(&self, doc_ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for &(lo, hi) in doc_ranges {
            if lo >= hi {
                continue;
            }
            for seg in &self.segments {
                if seg.doc_end <= lo {
                    continue;
                }
                if seg.doc_start >= hi {
                    break;
                }
                let s = lo.max(seg.doc_start);
                let e = hi.min(seg.doc_end);
                if s < e {
                    let ds = seg.display_start + (s - seg.doc_start);
                    let de = seg.display_start + (e - seg.doc_start);
                    out.push((ds, de));
                }
            }
        }
        out
    }

    pub(super) fn doc_line_to_display_row(&self, doc_line: usize) -> Option<usize> {
        self.visible_lines.binary_search(&doc_line).ok()
    }

    pub(super) fn boundary_at_display_row(&self, display_row: usize) -> Option<&FoldBoundary> {
        self.boundaries
            .iter()
            .find(|b| b.header_display_row == display_row)
    }
}

pub(super) struct FoldedTextBuffer<'a> {
    pub(super) doc_sql: &'a mut String,
    pub(super) fold_state: &'a mut SqlFoldState,
    pub(super) dialect: SqlDialect,
    pub(super) projection: FoldProjection,
    pub(super) mutated: bool,
    pub(super) unfolded_boundary_line: Option<usize>,
}

impl<'a> FoldedTextBuffer<'a> {
    pub(super) fn new(
        doc_sql: &'a mut String,
        fold_state: &'a mut SqlFoldState,
        dialect: SqlDialect,
        projection: FoldProjection,
    ) -> Self {
        Self {
            doc_sql,
            fold_state,
            dialect,
            projection,
            mutated: false,
            unfolded_boundary_line: None,
        }
    }

    fn rebuild_projection(&mut self) {
        let effective = self.fold_state.effective_collapsed_regions(self.doc_sql, self.dialect);
        self.projection = FoldProjection::build(self.doc_sql, &effective);
    }
}

impl TextBuffer for FoldedTextBuffer<'_> {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        &self.projection.display_text
    }

    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        if text.is_empty() {
            return 0;
        }
        if text.contains('\n') {
            if let Some(boundary) = self
                .projection
                .boundaries
                .iter()
                .find(|b| b.display_nl_char == char_index)
            {
                let start_line = boundary.region.start_line;
                self.fold_state.collapsed.remove(&start_line);
                self.unfolded_boundary_line = Some(start_line);
            }
        }
        let doc_byte = self.projection.display_char_to_doc_byte(char_index);
        let old_snapshot = self.doc_sql.clone();
        self.doc_sql.insert_str(doc_byte, text);
        self.fold_state.on_doc_edit(&old_snapshot, doc_byte, 0, text);
        self.rebuild_projection();
        self.mutated = true;
        text.chars().count()
    }

    fn delete_char_range(&mut self, char_range: Range<usize>) {
        if char_range.start >= char_range.end {
            return;
        }
        if char_range.end - char_range.start == 1 {
            if let Some(boundary) = self
                .projection
                .boundaries
                .iter()
                .find(|b| b.has_next_line && b.display_nl_char == char_range.start)
            {
                let start_line = boundary.region.start_line;
                self.fold_state.collapsed.remove(&start_line);
                self.rebuild_projection();
                self.unfolded_boundary_line = Some(start_line);
                return;
            }
        }

        let (doc_lo, doc_hi) = self
            .projection
            .display_char_range_to_doc_byte_range(char_range.start, char_range.end);
        if doc_lo >= doc_hi || doc_hi > self.doc_sql.len() {
            return;
        }
        let old_snapshot = self.doc_sql.clone();
        self.doc_sql.drain(doc_lo..doc_hi);
        self.fold_state
            .on_doc_edit(&old_snapshot, doc_lo, doc_hi - doc_lo, "");
        self.rebuild_projection();
        self.mutated = true;
    }

    fn replace_with(&mut self, text: &str) {
        self.doc_sql.clear();
        self.doc_sql.push_str(text);
        self.fold_state.unfold_all();
        self.rebuild_projection();
        self.mutated = true;
    }
}

#[derive(Clone, Default)]
pub(super) struct SqlLintCache {
    cached_sql_hash: u64,
    cached_sql_len: usize,
    cached_dialect: SqlDialect,
    cached_catalog_hash: u64,
    cached_schema_loaded: bool,
    diagnostics: Vec<SqlDiagnostic>,
    populated: bool,
}

impl SqlLintCache {
    pub(super) fn diagnostics(
        &mut self,
        sql: &str,
        dialect: SqlDialect,
        tables: &[SchemaTableHint],
        schema_loaded: bool,
    ) -> &[SqlDiagnostic] {
        let sql_hash = fnv1a64(sql.as_bytes());
        let cat_hash = hash_schema_hints(tables);
        if !self.populated
            || self.cached_sql_hash != sql_hash
            || self.cached_sql_len != sql.len()
            || self.cached_dialect != dialect
            || self.cached_catalog_hash != cat_hash
            || self.cached_schema_loaded != schema_loaded
        {
            self.diagnostics = lint_sql(sql, dialect, tables, schema_loaded);
            self.cached_sql_hash = sql_hash;
            self.cached_sql_len = sql.len();
            self.cached_dialect = dialect;
            self.cached_catalog_hash = cat_hash;
            self.cached_schema_loaded = schema_loaded;
            self.populated = true;
        }
        &self.diagnostics
    }
}

pub(super) fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn hash_schema_hints(tables: &[SchemaTableHint]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(PRIME);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(PRIME);
    };
    for t in tables {
        feed(t.name.as_bytes());
        for (c, _) in &t.columns {
            feed(c.as_bytes());
        }
    }
    hash
}

fn char_index_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_maps_coordinates_and_preserves_nested_folds() {
        let sql = "\
SELECT
  a,
  (
    SELECT 1
  ) AS x
FROM t;
SELECT 2;";

        let mut state = SqlFoldState::default();
        state.toggle_line(2); // inner subquery lines 2..=4
        state.toggle_line(0); // outer statement lines 0..=5

        let effective = state.effective_collapsed_regions(sql, SqlDialect::MySql);
        assert_eq!(effective.len(), 1);
        assert_eq!((effective[0].start_line, effective[0].end_line), (0, 5));

        let proj = FoldProjection::build(sql, &effective);
        assert_eq!(proj.display_text, "SELECT\nSELECT 2;");
        assert_eq!(proj.visible_lines, vec![0, 6]);

        let select2_doc_byte = sql.find("SELECT 2;").unwrap();
        let select2_disp_char = proj.doc_byte_to_display_char(select2_doc_byte);
        assert_eq!(proj.display_char_to_doc_byte(select2_disp_char), select2_doc_byte);

        // Unfolding outer keeps inner subquery folded
        state.toggle_line(0);
        let after_outer = state.effective_collapsed_regions(sql, SqlDialect::MySql);
        assert_eq!(after_outer.len(), 1);
        assert_eq!((after_outer[0].start_line, after_outer[0].end_line), (2, 4));
    }

    #[test]
    fn backspace_on_fold_boundary_unfolds_safely_without_data_loss() {
        let mut sql = "SELECT\n  a,\n  b\nFROM t;\nSELECT 2;".to_string();
        let original = sql.clone();
        let mut state = SqlFoldState::default();
        state.toggle_line(0);
        let eff = state.effective_collapsed_regions(&sql, SqlDialect::MySql);
        let proj = FoldProjection::build(&sql, &eff);
        let boundary_char = proj.boundaries[0].display_nl_char;

        let mut buf = FoldedTextBuffer::new(&mut sql, &mut state, SqlDialect::MySql, proj);
        buf.delete_char_range(boundary_char..boundary_char + 1);
        assert_eq!(buf.unfolded_boundary_line, Some(0));
        assert!(!buf.mutated);
        assert_eq!(*buf.doc_sql, original);
        assert!(!buf.fold_state.has_collapsed());
    }

    #[test]
    fn editing_above_fold_shifts_collapsed_line_numbers() {
        let mut sql = "SELECT 1;\nSELECT\n  a\nFROM t;".to_string();
        let mut state = SqlFoldState::default();
        state.toggle_line(1);
        let eff = state.effective_collapsed_regions(&sql, SqlDialect::MySql);
        let proj = FoldProjection::build(&sql, &eff);

        let mut buf = FoldedTextBuffer::new(&mut sql, &mut state, SqlDialect::MySql, proj);
        buf.insert_text("-- note\n", 0);
        assert!(buf.mutated);
        assert!(buf.fold_state.collapsed.contains(&2));
        assert!(!buf.fold_state.collapsed.contains(&1));
    }

    #[test]
    fn undo_stack_restores_full_document_including_hidden_lines() {
        let mut stack = SqlUndoStack::default();
        let full = "SELECT\n  a,\n  b\nFROM t;";
        stack.record_before_edit(full, 6, None, 1.0, true);
        let edited = "SELECT 2;";
        let undone = stack.undo(edited, 9, None).expect("undo");
        assert_eq!(undone.sql, full);
        let redone = stack.redo(&undone.sql, undone.cursor_byte, None).expect("redo");
        assert_eq!(redone.sql, edited);
    }
}
