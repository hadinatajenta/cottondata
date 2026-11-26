#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditResult {
    pub sql: String,
    pub cursor_byte: usize,
    pub selection: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
}

#[derive(Debug, Clone, Copy)]
struct LineSpan {
    start: usize,
    end: usize,
}

fn clamp_char_boundary(s: &str, byte_idx: usize) -> usize {
    let mut idx = byte_idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn normalize_selection(sql: &str, selection: Option<(usize, usize)>) -> Option<(usize, usize)> {
    let (a, b) = selection?;
    let lo = clamp_char_boundary(sql, a.min(b));
    let hi = clamp_char_boundary(sql, a.max(b));
    if lo < hi {
        Some((lo, hi))
    } else {
        None
    }
}

fn split_line_spans(sql: &str) -> Vec<LineSpan> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (idx, b) in sql.bytes().enumerate() {
        if b == b'\n' {
            spans.push(LineSpan { start, end: idx });
            start = idx + 1;
        }
    }
    spans.push(LineSpan {
        start,
        end: sql.len(),
    });
    spans
}

fn line_index_at(spans: &[LineSpan], byte: usize) -> usize {
    spans.partition_point(|span| span.end < byte)
}

fn target_line_range(
    spans: &[LineSpan],
    cursor: usize,
    selection: Option<(usize, usize)>,
) -> (usize, usize) {
    if let Some((lo, hi)) = selection {
        let first = line_index_at(spans, lo);
        let mut last = line_index_at(spans, hi);
        if last > first && hi == spans[last].start {
            last -= 1;
        }
        (first, last)
    } else {
        let line = line_index_at(spans, cursor);
        (line, line)
    }
}

fn leading_indent_len(line: &str) -> usize {
    line.bytes().take_while(|&b| b == b' ' || b == b'\t').count()
}

fn is_blank_line(line: &str) -> bool {
    line.trim().is_empty()
}

pub fn toggle_line_comment(
    sql: &str,
    cursor_byte: usize,
    selection: Option<(usize, usize)>,
) -> EditResult {
    let cursor = clamp_char_boundary(sql, cursor_byte);
    let norm_sel = normalize_selection(sql, selection);
    let spans = split_line_spans(sql);
    let (first_line, last_line) = target_line_range(&spans, cursor, norm_sel);

    let mut has_non_blank = false;
    let mut all_commented = true;
    let mut min_indent = usize::MAX;

    for span in &spans[first_line..=last_line] {
        let line = &sql[span.start..span.end];
        if !is_blank_line(line) {
            has_non_blank = true;
            let indent = leading_indent_len(line);
            if indent < min_indent {
                min_indent = indent;
            }
            if !line[indent..].starts_with("--") {
                all_commented = false;
            }
        }
    }

    let uncomment = has_non_blank && all_commented;
    let comment_indent = if has_non_blank { min_indent } else { 0 };

    let block_start = spans[first_line].start;
    let block_end = spans[last_line].end;
    let mut out = String::with_capacity(sql.len() + (last_line - first_line + 1) * 3);
    out.push_str(&sql[..block_start]);

    let mut new_cursor = cursor;

    for (offset_idx, span) in spans[first_line..=last_line].iter().enumerate() {
        if offset_idx > 0 {
            out.push('\n');
        }
        let line = &sql[span.start..span.end];
        let line_new_start = out.len();

        if uncomment {
            let indent = leading_indent_len(line);
            let rest = &line[indent..];
            let remove_len = if rest.starts_with("-- ") {
                3
            } else if rest.starts_with("--") {
                2
            } else {
                0
            };

            out.push_str(&line[..indent]);
            out.push_str(&line[indent + remove_len..]);

            if norm_sel.is_none() && first_line + offset_idx == first_line {
                let col = cursor.saturating_sub(span.start);
                let new_col = if col <= indent {
                    col
                } else if col <= indent + remove_len {
                    indent
                } else {
                    col - remove_len
                };
                new_cursor = line_new_start + new_col;
            }
        } else {
            let insert_col = comment_indent.min(leading_indent_len(line));
            out.push_str(&line[..insert_col]);
            out.push_str("-- ");
            out.push_str(&line[insert_col..]);

            if norm_sel.is_none() && first_line + offset_idx == first_line {
                let col = cursor.saturating_sub(span.start);
                let new_col = if col >= insert_col { col + 3 } else { col };
                new_cursor = line_new_start + new_col;
            }
        }
    }

    let edited_end = out.len();
    out.push_str(&sql[block_end..]);

    if norm_sel.is_some() {
        let new_selection = if block_start < edited_end {
            Some((block_start, edited_end))
        } else {
            None
        };
        EditResult {
            sql: out,
            cursor_byte: edited_end,
            selection: new_selection,
        }
    } else {
        EditResult {
            sql: out,
            cursor_byte: new_cursor,
            selection: None,
        }
    }
}

pub fn duplicate_lines(
    sql: &str,
    cursor_byte: usize,
    selection: Option<(usize, usize)>,
) -> EditResult {
    let cursor = clamp_char_boundary(sql, cursor_byte);
    let norm_sel = normalize_selection(sql, selection);
    let spans = split_line_spans(sql);
    let (first_line, last_line) = target_line_range(&spans, cursor, norm_sel);

    let block_start = spans[first_line].start;
    let block_end = spans[last_line].end;
    let block = &sql[block_start..block_end];
    let delta = block.len() + 1;

    let mut out = String::with_capacity(sql.len() + delta);
    out.push_str(&sql[..block_end]);
    out.push('\n');
    out.push_str(block);
    out.push_str(&sql[block_end..]);

    if let Some((lo, hi)) = norm_sel {
        let new_lo = lo + delta;
        let new_hi = (hi + delta).min(out.len());
        let new_cursor = if cursor >= lo && cursor <= hi {
            (cursor + delta).min(out.len())
        } else {
            new_hi
        };
        EditResult {
            sql: out,
            cursor_byte: new_cursor,
            selection: Some((new_lo, new_hi)),
        }
    } else {
        EditResult {
            sql: out,
            cursor_byte: cursor + delta,
            selection: None,
        }
    }
}

pub fn move_lines_up(
    sql: &str,
    cursor_byte: usize,
    selection: Option<(usize, usize)>,
) -> EditResult {
    let cursor = clamp_char_boundary(sql, cursor_byte);
    let norm_sel = normalize_selection(sql, selection);
    let spans = split_line_spans(sql);
    let (first_line, last_line) = target_line_range(&spans, cursor, norm_sel);

    if first_line == 0 {
        return EditResult {
            sql: sql.to_string(),
            cursor_byte: cursor,
            selection: norm_sel,
        };
    }

    let above = spans[first_line - 1];
    let block_start = spans[first_line].start;
    let block_end = spans[last_line].end;
    let shift = (above.end - above.start) + 1;

    let mut out = String::with_capacity(sql.len());
    out.push_str(&sql[..above.start]);
    out.push_str(&sql[block_start..block_end]);
    out.push('\n');
    out.push_str(&sql[above.start..above.end]);
    out.push_str(&sql[block_end..]);

    if let Some((lo, hi)) = norm_sel {
        let new_lo = lo - shift;
        let new_hi = hi - shift;
        let new_cursor = if cursor >= lo && cursor <= hi {
            cursor - shift
        } else {
            new_hi
        };
        EditResult {
            sql: out,
            cursor_byte: new_cursor,
            selection: Some((new_lo, new_hi)),
        }
    } else {
        EditResult {
            sql: out,
            cursor_byte: cursor - shift,
            selection: None,
        }
    }
}

pub fn move_lines_down(
    sql: &str,
    cursor_byte: usize,
    selection: Option<(usize, usize)>,
) -> EditResult {
    let cursor = clamp_char_boundary(sql, cursor_byte);
    let norm_sel = normalize_selection(sql, selection);
    let spans = split_line_spans(sql);
    let (first_line, last_line) = target_line_range(&spans, cursor, norm_sel);

    if last_line + 1 >= spans.len() {
        return EditResult {
            sql: sql.to_string(),
            cursor_byte: cursor,
            selection: norm_sel,
        };
    }

    let block_start = spans[first_line].start;
    let block_end = spans[last_line].end;
    let below = spans[last_line + 1];
    let shift = (below.end - below.start) + 1;

    let mut out = String::with_capacity(sql.len());
    out.push_str(&sql[..block_start]);
    out.push_str(&sql[below.start..below.end]);
    out.push('\n');
    out.push_str(&sql[block_start..block_end]);
    out.push_str(&sql[below.end..]);

    if let Some((lo, hi)) = norm_sel {
        let new_lo = lo + shift;
        let new_hi = (hi + shift).min(out.len());
        let new_cursor = if cursor >= lo && cursor <= hi {
            (cursor + shift).min(out.len())
        } else {
            new_hi
        };
        EditResult {
            sql: out,
            cursor_byte: new_cursor,
            selection: Some((new_lo, new_hi)),
        }
    } else {
        EditResult {
            sql: out,
            cursor_byte: cursor + shift,
            selection: None,
        }
    }
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn is_word_boundary(haystack: &str, start: usize, end: usize) -> bool {
    let prev_ok = haystack[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !is_word_char(c));
    let next_ok = haystack[end..]
        .chars()
        .next()
        .is_none_or(|c| !is_word_char(c));
    prev_ok && next_ok
}

pub fn find_matches(haystack: &str, needle: &str, opts: SearchOptions) -> Vec<(usize, usize)> {
    if needle.is_empty() || haystack.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();

    if opts.case_sensitive {
        let mut pos = 0;
        while pos < haystack.len() {
            let Some(rel) = haystack[pos..].find(needle) else {
                break;
            };
            let start = pos + rel;
            let end = start + needle.len();
            if !opts.whole_word || is_word_boundary(haystack, start, end) {
                matches.push((start, end));
                pos = end;
            } else {
                let step = haystack[start..].chars().next().map_or(1, char::len_utf8);
                pos = start + step;
            }
        }
        return matches;
    }

    if haystack.is_ascii() && needle.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        if n.len() > h.len() {
            return matches;
        }
        let mut pos = 0;
        while pos + n.len() <= h.len() {
            if h[pos..pos + n.len()].eq_ignore_ascii_case(n) {
                let end = pos + n.len();
                if !opts.whole_word || is_word_boundary(haystack, pos, end) {
                    matches.push((pos, end));
                    pos = end;
                    continue;
                }
            }
            pos += 1;
        }
        return matches;
    }

    let needle_lower: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle_lower.is_empty() {
        return matches;
    }

    let mut lowered: Vec<(char, usize, usize)> = Vec::with_capacity(haystack.len());
    for (orig_start, ch) in haystack.char_indices() {
        let orig_end = orig_start + ch.len_utf8();
        for lower_ch in ch.to_lowercase() {
            lowered.push((lower_ch, orig_start, orig_end));
        }
    }

    let nlen = needle_lower.len();
    if nlen > lowered.len() {
        return matches;
    }

    let mut i = 0;
    while i + nlen <= lowered.len() {
        let starts_at_orig = i == 0 || lowered[i - 1].1 != lowered[i].1;
        let last_idx = i + nlen - 1;
        let ends_at_orig = last_idx + 1 == lowered.len() || lowered[last_idx + 1].1 != lowered[last_idx].1;

        if starts_at_orig
            && ends_at_orig
            && lowered[i..=last_idx]
                .iter()
                .zip(needle_lower.iter())
                .all(|((hc, _, _), nc)| hc == nc)
        {
            let start = lowered[i].1;
            let end = lowered[last_idx].2;
            if !opts.whole_word || is_word_boundary(haystack, start, end) {
                matches.push((start, end));
                i += nlen;
                continue;
            }
        }
        i += 1;
    }

    matches
}

pub fn replace_match(
    haystack: &str,
    range: (usize, usize),
    replacement: &str,
) -> (String, usize) {
    let lo = clamp_char_boundary(haystack, range.0.min(range.1));
    let hi = clamp_char_boundary(haystack, range.0.max(range.1));
    let mut out = String::with_capacity(haystack.len() - (hi - lo) + replacement.len());
    out.push_str(&haystack[..lo]);
    out.push_str(replacement);
    out.push_str(&haystack[hi..]);
    (out, lo + replacement.len())
}

pub fn replace_all_matches(
    haystack: &str,
    needle: &str,
    replacement: &str,
    opts: SearchOptions,
) -> (String, usize) {
    let matches = find_matches(haystack, needle, opts);
    if matches.is_empty() {
        return (haystack.to_string(), 0);
    }

    let mut out = String::with_capacity(haystack.len());
    let mut last = 0;
    for &(start, end) in &matches {
        out.push_str(&haystack[last..start]);
        out.push_str(replacement);
        last = end;
    }
    out.push_str(&haystack[last..]);
    (out, matches.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_comment_single_line_and_roundtrip() {
        let sql = "SELECT * FROM users";
        let commented = toggle_line_comment(sql, 7, None);
        assert_eq!(commented.sql, "-- SELECT * FROM users");
        assert_eq!(commented.cursor_byte, 10);
        assert_eq!(commented.selection, None);

        let uncommented = toggle_line_comment(&commented.sql, commented.cursor_byte, None);
        assert_eq!(uncommented.sql, sql);
        assert_eq!(uncommented.cursor_byte, 7);
        assert_eq!(uncommented.selection, None);
    }

    #[test]
    fn toggle_comment_empty_string_and_blank_line() {
        let res = toggle_line_comment("", 0, None);
        assert_eq!(res.sql, "-- ");
        assert_eq!(res.cursor_byte, 3);
        assert_eq!(res.selection, None);

        let un = toggle_line_comment(&res.sql, res.cursor_byte, None);
        assert_eq!(un.sql, "");
        assert_eq!(un.cursor_byte, 0);

        let blank = toggle_line_comment("   ", 2, None);
        assert_eq!(blank.sql, "--    ");
        assert_eq!(blank.cursor_byte, 5);
    }

    #[test]
    fn toggle_comment_preserves_indent_and_handles_no_space_after_dashes() {
        let sql = "    SELECT 1";
        let res = toggle_line_comment(sql, 2, None);
        assert_eq!(res.sql, "    -- SELECT 1");
        assert_eq!(res.cursor_byte, 2);

        let tight = "  --SELECT 1";
        let un = toggle_line_comment(tight, 6, None);
        assert_eq!(un.sql, "  SELECT 1");
        assert_eq!(un.cursor_byte, 4);

        let inside = toggle_line_comment("  -- SELECT 1", 4, None);
        assert_eq!(inside.sql, "  SELECT 1");
        assert_eq!(inside.cursor_byte, 2);
    }

    #[test]
    fn toggle_comment_multiline_selection_common_indent_and_trailing_newline() {
        let sql = "  SELECT 1\n    FROM users\n  WHERE id = 1";
        let sel_end = "  SELECT 1\n    FROM users\n".len();
        let res = toggle_line_comment(sql, sel_end, Some((0, sel_end)));
        assert_eq!(res.sql, "  -- SELECT 1\n  --   FROM users\n  WHERE id = 1");
        assert_eq!(
            res.selection,
            Some((0, "  -- SELECT 1\n  --   FROM users".len()))
        );
        assert_eq!(res.cursor_byte, "  -- SELECT 1\n  --   FROM users".len());

        let un = toggle_line_comment(&res.sql, res.cursor_byte, res.selection);
        assert_eq!(un.sql, sql);
        assert_eq!(un.selection, Some((0, "  SELECT 1\n    FROM users".len())));
    }

    #[test]
    fn toggle_comment_mixed_commented_and_uncommented_comments_all() {
        let sql = "-- SELECT 1\nFROM users";
        let res = toggle_line_comment(sql, sql.len(), Some((0, sql.len())));
        assert_eq!(res.sql, "-- -- SELECT 1\n-- FROM users");

        let un = toggle_line_comment(&res.sql, res.cursor_byte, res.selection);
        assert_eq!(un.sql, sql);
    }

    #[test]
    fn toggle_comment_multiline_with_blank_line_between() {
        let sql = "  SELECT 1\n\n  FROM t";
        let res = toggle_line_comment(sql, sql.len(), Some((0, sql.len())));
        assert_eq!(res.sql, "  -- SELECT 1\n-- \n  -- FROM t");

        let un = toggle_line_comment(&res.sql, res.cursor_byte, res.selection);
        assert_eq!(un.sql, sql);
    }

    #[test]
    fn toggle_comment_clamps_invalid_utf8_and_oob_offsets() {
        let sql = "SELECT '🦀'";
        let res = toggle_line_comment(sql, 9, None);
        assert_eq!(res.sql, "-- SELECT '🦀'");
        assert!(res.sql.is_char_boundary(res.cursor_byte));
        assert_eq!(res.cursor_byte, 11);

        let oob = toggle_line_comment(sql, 999, None);
        assert_eq!(oob.sql, "-- SELECT '🦀'");
        assert_eq!(oob.cursor_byte, oob.sql.len());
    }

    #[test]
    fn duplicate_single_line_and_empty_string() {
        let res = duplicate_lines("SELECT 1", 3, None);
        assert_eq!(res.sql, "SELECT 1\nSELECT 1");
        assert_eq!(res.cursor_byte, 12);
        assert_eq!(res.selection, None);

        let empty = duplicate_lines("", 0, None);
        assert_eq!(empty.sql, "\n");
        assert_eq!(empty.cursor_byte, 1);
    }

    #[test]
    fn duplicate_middle_line_and_multiline_selection() {
        let sql = "SELECT 1\nFROM users\nWHERE id = 1";
        let res = duplicate_lines(sql, 13, None);
        assert_eq!(res.sql, "SELECT 1\nFROM users\nFROM users\nWHERE id = 1");
        assert_eq!(res.cursor_byte, 24);

        let multi = duplicate_lines(sql, 19, Some((0, 19)));
        assert_eq!(
            multi.sql,
            "SELECT 1\nFROM users\nSELECT 1\nFROM users\nWHERE id = 1"
        );
        assert_eq!(multi.selection, Some((20, 39)));
        assert_eq!(multi.cursor_byte, 39);
        assert_eq!(&multi.sql[20..39], "SELECT 1\nFROM users");
    }

    #[test]
    fn duplicate_selection_ending_at_next_line_col_zero() {
        let sql = "aaa\nbbb\nccc";
        let res = duplicate_lines(sql, 4, Some((0, 4)));
        assert_eq!(res.sql, "aaa\naaa\nbbb\nccc");
        assert_eq!(res.selection, Some((4, 8)));
        assert_eq!(res.cursor_byte, 8);
    }

    #[test]
    fn move_lines_up_single_multi_and_top_boundary() {
        let sql = "aaa\nbbb\nccc";
        let noop = move_lines_up(sql, 2, None);
        assert_eq!(noop.sql, sql);
        assert_eq!(noop.cursor_byte, 2);
        assert_eq!(noop.selection, None);

        let moved = move_lines_up(sql, 5, None);
        assert_eq!(moved.sql, "bbb\naaa\nccc");
        assert_eq!(moved.cursor_byte, 1);

        let multi = move_lines_up(sql, 10, Some((5, 10)));
        assert_eq!(multi.sql, "bbb\nccc\naaa");
        assert_eq!(multi.selection, Some((1, 6)));
        assert_eq!(multi.cursor_byte, 6);
        assert_eq!(&multi.sql[1..6], "bb\ncc");
    }

    #[test]
    fn move_lines_down_single_multi_and_bottom_boundary() {
        let sql = "aaa\nbbb\nccc";
        let noop = move_lines_down(sql, 9, None);
        assert_eq!(noop.sql, sql);
        assert_eq!(noop.cursor_byte, 9);
        assert_eq!(noop.selection, None);

        let moved = move_lines_down(sql, 1, None);
        assert_eq!(moved.sql, "bbb\naaa\nccc");
        assert_eq!(moved.cursor_byte, 5);

        let multi = move_lines_down(sql, 6, Some((1, 6)));
        assert_eq!(multi.sql, "ccc\naaa\nbbb");
        assert_eq!(multi.selection, Some((5, 10)));
        assert_eq!(multi.cursor_byte, 10);
        assert_eq!(&multi.sql[5..10], "aa\nbb");

        let trailing = move_lines_down("aaa\nbbb", 4, Some((0, 4)));
        assert_eq!(trailing.sql, "bbb\naaa");
        assert_eq!(trailing.selection, Some((4, 7)));
        assert_eq!(trailing.cursor_byte, 7);
    }

    #[test]
    fn move_lines_utf8_clamping() {
        let sql = "a🦀\nb🔥";
        let up = move_lines_up(sql, 8, None);
        assert_eq!(up.sql, "b🔥\na🦀");
        assert!(up.sql.is_char_boundary(up.cursor_byte));
        assert_eq!(up.cursor_byte, 1);
    }

    #[test]
    fn find_matches_case_sensitivity_and_non_overlapping() {
        assert!(find_matches("SELECT 1", "", SearchOptions::default()).is_empty());
        assert!(find_matches("", "a", SearchOptions::default()).is_empty());

        let ci = find_matches(
            "SELECT select SeLeCt",
            "select",
            SearchOptions {
                case_sensitive: false,
                whole_word: false,
            },
        );
        assert_eq!(ci, vec![(0, 6), (7, 13), (14, 20)]);

        let cs = find_matches(
            "SELECT select SeLeCt",
            "select",
            SearchOptions {
                case_sensitive: true,
                whole_word: false,
            },
        );
        assert_eq!(cs, vec![(7, 13)]);

        let non_overlap = find_matches("aaaa", "aa", SearchOptions::default());
        assert_eq!(non_overlap, vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn find_matches_whole_word() {
        let sql = "id user_id id2 (id) id";
        let matches = find_matches(
            sql,
            "id",
            SearchOptions {
                case_sensitive: false,
                whole_word: true,
            },
        );
        assert_eq!(matches, vec![(0, 2), (16, 18), (20, 22)]);
        for (s, e) in matches {
            assert_eq!(&sql[s..e], "id");
        }

        let cs_ww = find_matches(
            "ID user_id id",
            "id",
            SearchOptions {
                case_sensitive: true,
                whole_word: true,
            },
        );
        assert_eq!(cs_ww, vec![(11, 13)]);
    }

    #[test]
    fn find_matches_unicode_case_insensitive_exact_byte_boundaries() {
        let haystack = "İSTANBUL café CAFÉ Kelvin";
        let m1 = find_matches(haystack, "café", SearchOptions::default());
        assert_eq!(m1.len(), 2);
        assert_eq!(&haystack[m1[0].0..m1[0].1], "café");
        assert_eq!(&haystack[m1[1].0..m1[1].1], "CAFÉ");

        let m2 = find_matches(haystack, "İ", SearchOptions::default());
        assert_eq!(m2, vec![(0, 'İ'.len_utf8())]);
        assert_eq!(&haystack[m2[0].0..m2[0].1], "İ");

        let m3 = find_matches(haystack, "i", SearchOptions::default());
        assert_eq!(m3.len(), 1);
        assert_eq!(&haystack[m3[0].0..m3[0].1], "i");

        let m4 = find_matches(haystack, "kelvin", SearchOptions::default());
        assert_eq!(m4.len(), 1);
        assert_eq!(&haystack[m4[0].0..m4[0].1], "Kelvin");
    }

    #[test]
    fn replace_match_basic_and_clamping() {
        let (out, cursor) = replace_match("SELECT id FROM users", (7, 9), "email");
        assert_eq!(out, "SELECT email FROM users");
        assert_eq!(cursor, 12);

        let (utf8_out, utf8_cursor) = replace_match("a🦀b", (2, 100), "!");
        assert_eq!(utf8_out, "a!");
        assert_eq!(utf8_cursor, 2);
    }

    #[test]
    fn replace_all_matches_basic_whole_word_and_empty() {
        let (out, count) = replace_all_matches(
            "SELECT id, user_id FROM t WHERE ID = 1",
            "id",
            "pk",
            SearchOptions {
                case_sensitive: false,
                whole_word: true,
            },
        );
        assert_eq!(out, "SELECT pk, user_id FROM t WHERE pk = 1");
        assert_eq!(count, 2);

        let (no_change, zero) = replace_all_matches("SELECT 1", "", "x", SearchOptions::default());
        assert_eq!(no_change, "SELECT 1");
        assert_eq!(zero, 0);

        let (grow, grow_count) =
            replace_all_matches("a a", "a", "aa", SearchOptions::default());
        assert_eq!(grow, "aa aa");
        assert_eq!(grow_count, 2);
    }
}
