use crate::complete::SchemaTableHint;
use crate::fold::{compute_line_spans, line_index_for_byte};
use crate::lexer::{dollar_quote_tag_len, split_statements, tokenize, Token, TokenKind};
use cottondata_core::{DbDiagnostic, SqlDialect, SqlErrorObjectKind};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SqlDiagSeverity {
    Hint,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlDiagSource {
    StaticLint,
    ServerError { code: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlQuickFix {
    pub title: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlDiagnostic {
    pub start_byte: usize,
    pub end_byte: usize,
    pub severity: SqlDiagSeverity,
    pub source: SqlDiagSource,
    pub message: String,
    pub recommendation: Option<String>,
    pub fixes: Vec<SqlQuickFix>,
}

impl SqlDiagnostic {
    pub fn apply_fix(sql: &str, fix: &SqlQuickFix) -> Option<(String, usize)> {
        if fix.start_byte > fix.end_byte
            || fix.end_byte > sql.len()
            || !sql.is_char_boundary(fix.start_byte)
            || !sql.is_char_boundary(fix.end_byte)
        {
            return None;
        }
        let mut out = String::with_capacity(sql.len() + fix.replacement.len());
        out.push_str(&sql[..fix.start_byte]);
        out.push_str(&fix.replacement);
        let new_cursor = out.len();
        out.push_str(&sql[fix.end_byte..]);
        Some((out, new_cursor))
    }
}

const MAJOR_STATEMENT_KEYWORDS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP", "TRUNCATE", "REPLACE",
    "EXPLAIN", "DESCRIBE", "SHOW", "WITH", "BEGIN", "COMMIT", "ROLLBACK", "GRANT", "REVOKE",
    "CALL", "SET", "USE", "VALUES", "MERGE", "VACUUM", "ANALYZE", "PRAGMA",
];

const MAJOR_CLAUSE_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "JOIN", "LEFT",
    "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "UNION", "VALUES", "INTO", "SET", "DISTINCT",
    "RETURNING", "ON", "AND", "OR", "CASE", "WHEN", "THEN", "ELSE", "END", "BETWEEN", "LIKE",
    "EXISTS", "WINDOW", "FETCH", "PARTITION",
];

pub fn lint_sql(
    sql: &str,
    dialect: SqlDialect,
    tables: &[SchemaTableHint],
    schema_loaded: bool,
) -> Vec<SqlDiagnostic> {
    if sql.trim().is_empty() {
        return Vec::new();
    }

    let mut diags = Vec::new();
    let all_tokens = tokenize(sql);

    check_unclosed_lexical_tokens(sql, &all_tokens, &mut diags);

    for stmt in split_statements(sql) {
        let stmt_slice = &sql[stmt.start..stmt.end];
        let mut stmt_tokens = tokenize(stmt_slice);
        for tok in &mut stmt_tokens {
            tok.start += stmt.start;
            tok.end += stmt.start;
        }
        let sig: Vec<&Token<'_>> = stmt_tokens
            .iter()
            .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
            .collect();
        if sig.is_empty() {
            continue;
        }

        check_statement_structure(sql, &sig, &mut diags);
        check_syntax_and_clauses(sql, &sig, tables, &mut diags);
        check_dialect_rules(&sig, dialect, &mut diags);
        if schema_loaded && !tables.is_empty() {
            check_schema_references(&sig, tables, &mut diags);
        }
    }

    diags.sort_by(|a, b| {
        a.start_byte
            .cmp(&b.start_byte)
            .then_with(|| b.severity.cmp(&a.severity))
            .then_with(|| a.end_byte.cmp(&b.end_byte))
    });
    diags.dedup_by(|a, b| a.start_byte == b.start_byte && a.end_byte == b.end_byte && a.message == b.message);
    diags
}

pub fn resolve_server_error_diagnostic(
    full_sql: &str,
    stmt_span: (usize, usize),
    db_diag: &DbDiagnostic,
    dialect: SqlDialect,
    tables: &[SchemaTableHint],
) -> Option<SqlDiagnostic> {
    if full_sql.is_empty() {
        return None;
    }
    let lo = clamp_boundary(full_sql, stmt_span.0.min(stmt_span.1));
    let hi = clamp_boundary(full_sql, stmt_span.0.max(stmt_span.1));
    let (base, stmt_sql) = if lo < hi {
        (lo, &full_sql[lo..hi])
    } else {
        (0, full_sql)
    };

    let code = db_diag
        .sqlstate
        .as_deref()
        .map(str::to_string)
        .or_else(|| db_diag.code.map(|c| c.to_string()));
    let loc = db_diag.location.as_deref();

    let (rel_start, rel_end) = loc
        .and_then(|l| locate_error_span(stmt_sql, l))
        .unwrap_or_else(|| first_non_ws_line_span(stmt_sql));

    let abs_start = clamp_boundary(full_sql, base + rel_start);
    let abs_end = clamp_boundary(full_sql, (base + rel_end).max(abs_start));

    let mut fixes = Vec::new();
    let mut recommendation = db_diag.hint.clone();

    if let Some(l) = loc {
        if let Some(obj_name) = &l.object_name {
            let clean_obj = obj_name
                .trim_matches(|c| matches!(c, '`' | '"' | '\'' | '[' | ']'))
                .to_string();
            match l.object_kind {
                Some(SqlErrorObjectKind::Table) if !tables.is_empty() => {
                    let target = clean_obj.rsplit('.').next().unwrap_or(&clean_obj);
                    if let Some(best) = best_fuzzy_match(target, tables.iter().map(|t| t.name.as_str())) {
                        let replacement = format_ident_matching_style(&full_sql[abs_start..abs_end], best, dialect);
                        fixes.push(SqlQuickFix {
                            title: format!("Replace `{target}` with `{best}`"),
                            start_byte: abs_start,
                            end_byte: abs_end,
                            replacement,
                        });
                        recommendation = Some(format!(
                            "Did you mean table `{best}`? {}",
                            recommendation.unwrap_or_default()
                        ).trim().to_string());
                    }
                }
                Some(SqlErrorObjectKind::Column) if !tables.is_empty() => {
                    let (qualifier, col_only) = match clean_obj.rsplit_once('.') {
                        Some((q, c)) => (Some(q), c),
                        None => (None, clean_obj.as_str()),
                    };
                    let aliases = extract_stmt_aliases(stmt_sql);
                    let candidates = collect_candidate_columns(qualifier, &aliases, tables);
                    if let Some(best) = best_fuzzy_match(col_only, candidates.iter().map(String::as_str)) {
                        let replacement = match qualifier {
                            Some(q) if full_sql[abs_start..abs_end].contains('.') => format!("{q}.{best}"),
                            _ => format_ident_matching_style(&full_sql[abs_start..abs_end], best, dialect),
                        };
                        fixes.push(SqlQuickFix {
                            title: format!("Replace `{col_only}` with `{best}`"),
                            start_byte: abs_start,
                            end_byte: abs_end,
                            replacement,
                        });
                        recommendation = Some(format!(
                            "Did you mean column `{best}`? {}",
                            recommendation.unwrap_or_default()
                        ).trim().to_string());
                    }
                }
                Some(SqlErrorObjectKind::Syntax) => {}
                _ => {}
            }
        }

        if l.object_kind == Some(SqlErrorObjectKind::Syntax) {
            if let Some(near) = &l.near_token {
                let first_word = near
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .find(|w| !w.is_empty())
                    .unwrap_or("");
                if let Some(kw) = suggest_keyword_typo(first_word, MAJOR_CLAUSE_KEYWORDS)
                    .or_else(|| suggest_keyword_typo(first_word, MAJOR_STATEMENT_KEYWORDS))
                {
                    if full_sql[abs_start..abs_end].eq_ignore_ascii_case(first_word) {
                        fixes.push(SqlQuickFix {
                            title: format!("Replace `{first_word}` with `{kw}`"),
                            start_byte: abs_start,
                            end_byte: abs_end,
                            replacement: kw.to_string(),
                        });
                        recommendation = Some(format!("Keyword typo detected: did you mean `{kw}`?"));
                    }
                } else if is_missing_from_before_offset(full_sql, base, abs_start) {
                    fixes.push(SqlQuickFix {
                        title: format!("Insert `FROM` before `{first_word}`"),
                        start_byte: abs_start,
                        end_byte: abs_start,
                        replacement: "FROM ".into(),
                    });
                    recommendation = Some(format!("Missing `FROM` keyword before `{first_word}`."));
                }
            }
        }
    }

    Some(SqlDiagnostic {
        start_byte: abs_start,
        end_byte: abs_end.max((abs_start + 1).min(full_sql.len())),
        severity: SqlDiagSeverity::Error,
        source: SqlDiagSource::ServerError { code },
        message: db_diag.summary.clone(),
        recommendation,
        fixes,
    })
}

fn locate_error_span(stmt_sql: &str, loc: &cottondata_core::SqlErrorLocation) -> Option<(usize, usize)> {
    let spans = compute_line_spans(stmt_sql);
    let tokens = tokenize(stmt_sql);

    if let Some(char_offset) = loc.char_offset {
        let byte = char_offset_to_byte(stmt_sql, char_offset);
        if let Some(tok) = tokens
            .iter()
            .find(|t| !matches!(t.kind, TokenKind::Whitespace) && byte >= t.start && byte < t.end)
            .or_else(|| {
                tokens
                    .iter()
                    .find(|t| !matches!(t.kind, TokenKind::Whitespace) && t.start >= byte)
            })
        {
            return Some((tok.start, tok.end));
        }
        let line = line_index_for_byte(&spans, byte);
        return Some((spans[line].start, spans[line].end.max(spans[line].start)));
    }

    if let Some(obj_name) = &loc.object_name {
        let clean = obj_name.trim_matches(|c| matches!(c, '`' | '"' | '\'' | '[' | ']'));
        let short = clean.rsplit('.').next().unwrap_or(clean);
        let preferred_line = loc.line.and_then(|l| l.checked_sub(1));

        let find_ident = |restrict_line: Option<usize>| -> Option<(usize, usize)> {
            for (idx, tok) in tokens.iter().enumerate() {
                if !matches!(tok.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier | TokenKind::Keyword) {
                    continue;
                }
                if let Some(target_line) = restrict_line {
                    if line_index_for_byte(&spans, tok.start) != target_line {
                        continue;
                    }
                }
                let tok_clean = tok.text.trim_matches(|c| matches!(c, '`' | '"' | '[' | ']'));
                if tok_clean.eq_ignore_ascii_case(short) {
                    if let Some((qual, _)) = clean.rsplit_once('.') {
                        if idx >= 2 && tokens[idx - 1].text == "." {
                            let prev_clean = tokens[idx - 2].text.trim_matches(|c| matches!(c, '`' | '"' | '[' | ']'));
                            if prev_clean.eq_ignore_ascii_case(qual) {
                                return Some((tokens[idx - 2].start, tok.end));
                            }
                        }
                    }
                    return Some((tok.start, tok.end));
                }
            }
            None
        };

        if let Some(found) = preferred_line.and_then(|l| find_ident(Some(l))).or_else(|| find_ident(None)) {
            return Some(found);
        }
    }

    if let Some(near) = &loc.near_token {
        let first_piece = near
            .split_whitespace()
            .next()
            .unwrap_or(near)
            .trim_matches(|c| matches!(c, '\'' | '"'));
        if !first_piece.is_empty() {
            if let Some(line_num) = loc.line.and_then(|l| l.checked_sub(1)) {
                if let Some(span) = spans.get(line_num) {
                    let line_text = &stmt_sql[span.start..span.end];
                    if let Some(rel) = find_ci(line_text, first_piece) {
                        return Some((span.start + rel, span.start + rel + first_piece.len()));
                    }
                    if span.start < span.end {
                        return Some((span.start, span.end));
                    }
                }
            }
            if let Some(rel) = find_ci(stmt_sql, first_piece) {
                return Some((rel, rel + first_piece.len()));
            }
        }
    }

    if let Some(line_num) = loc.line.and_then(|l| l.checked_sub(1)) {
        if let Some(span) = spans.get(line_num) {
            let line_text = &stmt_sql[span.start..span.end];
            let trim_lead = line_text.len() - line_text.trim_start().len();
            let s = span.start + trim_lead;
            let e = span.end.max(s);
            return Some((s, e));
        }
    }

    None
}

fn first_non_ws_line_span(sql: &str) -> (usize, usize) {
    let spans = compute_line_spans(sql);
    for span in &spans {
        let line = &sql[span.start..span.end];
        if !line.trim().is_empty() {
            let lead = line.len() - line.trim_start().len();
            return (span.start + lead, span.end);
        }
    }
    (0, sql.len())
}

fn check_unclosed_lexical_tokens(sql: &str, tokens: &[Token<'_>], out: &mut Vec<SqlDiagnostic>) {
    let bytes = sql.as_bytes();
    for tok in tokens {
        match tok.kind {
            TokenKind::Comment if tok.text.starts_with("/*") => {
                if tok.text.len() < 4 || !tok.text.ends_with("*/") {
                    out.push(SqlDiagnostic {
                        start_byte: tok.start,
                        end_byte: (tok.start + 2).min(sql.len()),
                        severity: SqlDiagSeverity::Error,
                        source: SqlDiagSource::StaticLint,
                        message: "Unclosed block comment `/*`".into(),
                        recommendation: Some("Close the block comment with `*/`.".into()),
                        fixes: vec![SqlQuickFix {
                            title: "Append ` */`".into(),
                            start_byte: tok.end,
                            end_byte: tok.end,
                            replacement: " */".into(),
                        }],
                    });
                }
            }
            TokenKind::StringLiteral => {
                if let Some(tag_len) = dollar_quote_tag_len(bytes, tok.start) {
                    let tag = &sql[tok.start..tok.start + tag_len];
                    let closed = tok.text.len() >= tag_len * 2 && tok.text.ends_with(tag);
                    if !closed {
                        out.push(SqlDiagnostic {
                            start_byte: tok.start,
                            end_byte: (tok.start + tag_len).min(sql.len()),
                            severity: SqlDiagSeverity::Error,
                            source: SqlDiagSource::StaticLint,
                            message: format!("Unclosed dollar-quoted string `{tag}`"),
                            recommendation: Some(format!("Close the block with matching `{tag}`.")),
                            fixes: vec![SqlQuickFix {
                                title: format!("Append `{tag}`"),
                                start_byte: tok.end,
                                end_byte: tok.end,
                                replacement: tag.to_string(),
                            }],
                        });
                    }
                } else if let Some(first) = tok.text.bytes().next() {
                    if (first == b'\'' || first == b'"') && !is_properly_closed_quote(tok.text, first) {
                        let q = first as char;
                        let end_highlight = tok
                            .text
                            .find('\n')
                            .map(|o| tok.start + o)
                            .unwrap_or(tok.end);
                        out.push(SqlDiagnostic {
                            start_byte: tok.start,
                            end_byte: end_highlight.max((tok.start + 1).min(sql.len())),
                            severity: SqlDiagSeverity::Error,
                            source: SqlDiagSource::StaticLint,
                            message: format!("Unclosed string/quoted literal `{q}`"),
                            recommendation: Some(format!("Add a matching closing `{q}` quote.")),
                            fixes: vec![SqlQuickFix {
                                title: format!("Insert closing `{q}`"),
                                start_byte: end_highlight,
                                end_byte: end_highlight,
                                replacement: q.to_string(),
                            }],
                        });
                    }
                }
            }
            TokenKind::QuotedIdentifier => {
                if !is_properly_closed_backtick(tok.text) {
                    let end_highlight = tok
                        .text
                        .find('\n')
                        .map(|o| tok.start + o)
                        .unwrap_or(tok.end);
                    out.push(SqlDiagnostic {
                        start_byte: tok.start,
                        end_byte: end_highlight.max((tok.start + 1).min(sql.len())),
                        severity: SqlDiagSeverity::Error,
                        source: SqlDiagSource::StaticLint,
                        message: "Unclosed backtick identifier `` ` ``".into(),
                        recommendation: Some("Add a closing backtick `` ` ``.".into()),
                        fixes: vec![SqlQuickFix {
                            title: "Insert closing `` ` ``".into(),
                            start_byte: end_highlight,
                            end_byte: end_highlight,
                            replacement: "`".into(),
                        }],
                    });
                }
            }
            _ => {}
        }
    }
}

fn is_properly_closed_quote(text: &str, quote: u8) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 2 || bytes[0] != quote {
        return false;
    }
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
        } else if bytes[i] == quote {
            if i + 1 < bytes.len() && bytes[i + 1] == quote {
                i += 2;
            } else {
                return i + 1 == bytes.len();
            }
        } else {
            i += 1;
        }
    }
    false
}

fn is_properly_closed_backtick(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'`' {
        return false;
    }
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'`' {
                i += 2;
            } else {
                return i + 1 == bytes.len();
            }
        } else {
            i += 1;
        }
    }
    false
}

fn check_statement_structure(sql: &str, sig: &[&Token<'_>], out: &mut Vec<SqlDiagnostic>) {
    let mut paren_stack: Vec<&Token<'_>> = Vec::new();
    let mut block_stack: Vec<(&Token<'_>, &'static str)> = Vec::new();

    let mut i = 0usize;
    while i < sig.len() {
        let tok = sig[i];
        if tok.kind == TokenKind::Punctuation && tok.text == "(" {
            paren_stack.push(tok);
        } else if tok.kind == TokenKind::Punctuation && tok.text == ")" {
            if paren_stack.pop().is_none() {
                out.push(SqlDiagnostic {
                    start_byte: tok.start,
                    end_byte: tok.end,
                    severity: SqlDiagSeverity::Error,
                    source: SqlDiagSource::StaticLint,
                    message: "Unmatched closing parenthesis `)`".into(),
                    recommendation: Some("Remove the extra `)` or add a matching opening `(`.".into()),
                    fixes: vec![SqlQuickFix {
                        title: "Remove unmatched `)`".into(),
                        start_byte: tok.start,
                        end_byte: tok.end,
                        replacement: String::new(),
                    }],
                });
            }
        } else if matches!(tok.kind, TokenKind::Keyword | TokenKind::Identifier) {
            if tok.text.eq_ignore_ascii_case("CASE") {
                block_stack.push((tok, "CASE"));
            } else if tok.text.eq_ignore_ascii_case("BEGIN") {
                let is_tx = sig.get(i + 1).is_none_or(|n| {
                    n.text == ";"
                        || n.text.eq_ignore_ascii_case("TRANSACTION")
                        || n.text.eq_ignore_ascii_case("TRAN")
                        || n.text.eq_ignore_ascii_case("WORK")
                        || n.text.eq_ignore_ascii_case("DEFERRED")
                        || n.text.eq_ignore_ascii_case("IMMEDIATE")
                        || n.text.eq_ignore_ascii_case("EXCLUSIVE")
                        || n.text.eq_ignore_ascii_case("ISOLATION")
                        || n.text.eq_ignore_ascii_case("READ")
                        || n.text.eq_ignore_ascii_case("DISTRIBUTED")
                });
                if !is_tx {
                    block_stack.push((tok, "BEGIN"));
                }
            } else if tok.text.eq_ignore_ascii_case("END") {
                let is_qualified = sig.get(i + 1).is_some_and(|n| {
                    n.text.eq_ignore_ascii_case("IF")
                        || n.text.eq_ignore_ascii_case("LOOP")
                        || n.text.eq_ignore_ascii_case("WHILE")
                        || n.text.eq_ignore_ascii_case("REPEAT")
                });
                if is_qualified {
                    i += 2;
                    continue;
                }
                if sig.get(i + 1).is_some_and(|n| n.text.eq_ignore_ascii_case("TRY") || n.text.eq_ignore_ascii_case("CATCH")) {
                    let _ = block_stack.pop();
                    i += 2;
                    continue;
                }
                let _ = block_stack.pop();
            }
        }
        i += 1;
    }

    let stmt_end = sig.last().map(|t| t.end).unwrap_or(sql.len());
    for open in paren_stack {
        out.push(SqlDiagnostic {
            start_byte: open.start,
            end_byte: open.end,
            severity: SqlDiagSeverity::Error,
            source: SqlDiagSource::StaticLint,
            message: "Unclosed parenthesis `(`".into(),
            recommendation: Some("Add a matching closing `)`.".into()),
            fixes: vec![SqlQuickFix {
                title: "Append `)` at end of statement".into(),
                start_byte: stmt_end,
                end_byte: stmt_end,
                replacement: ")".into(),
            }],
        });
    }

    for (open, label) in block_stack {
        out.push(SqlDiagnostic {
            start_byte: open.start,
            end_byte: open.end,
            severity: SqlDiagSeverity::Error,
            source: SqlDiagSource::StaticLint,
            message: format!("Unclosed `{label}` block (missing `END`)"),
            recommendation: Some(format!("Close the `{label}` block with `END`.")),
            fixes: vec![SqlQuickFix {
                title: "Append ` END`".into(),
                start_byte: stmt_end,
                end_byte: stmt_end,
                replacement: " END".into(),
            }],
        });
    }
}

fn check_syntax_and_clauses(
    sql: &str,
    sig: &[&Token<'_>],
    tables: &[SchemaTableHint],
    out: &mut Vec<SqlDiagnostic>,
) {
    let first = sig[0];
    if first.kind == TokenKind::Identifier && !matches_any_table_or_col(first.text, tables) {
        if let Some(kw) = suggest_keyword_typo(first.text, MAJOR_STATEMENT_KEYWORDS) {
            out.push(SqlDiagnostic {
                start_byte: first.start,
                end_byte: first.end,
                severity: SqlDiagSeverity::Error,
                source: SqlDiagSource::StaticLint,
                message: format!("Unknown SQL command `{}` — did you mean `{kw}`?", first.text),
                recommendation: Some(format!("Replace `{}` with `{kw}`.", first.text)),
                fixes: vec![SqlQuickFix {
                    title: format!("Replace `{}` with `{kw}`", first.text),
                    start_byte: first.start,
                    end_byte: first.end,
                    replacement: kw.to_string(),
                }],
            });
        }
    }

    let mut paren_depth = 0usize;
    let mut in_predicate_clause = false;
    let mut top_select_idx: Option<usize> = None;
    let mut top_from_idx: Option<usize> = None;
    let mut top_where_idx: Option<usize> = None;
    let mut has_top_where = false;
    let is_dml_write = first.text.eq_ignore_ascii_case("UPDATE") || first.text.eq_ignore_ascii_case("DELETE");

    let mut i = 0usize;
    while i < sig.len() {
        let tok = sig[i];
        if tok.text == "(" {
            paren_depth += 1;
            i += 1;
            continue;
        }
        if tok.text == ")" {
            paren_depth = paren_depth.saturating_sub(1);
            i += 1;
            continue;
        }

        if tok.text == "," {
            let next = sig.get(i + 1);
            let is_trailing_comma = match next {
                None => true,
                Some(n) => {
                    n.text == ")"
                        || n.text == ";"
                        || n.text == ","
                        || is_any_word_ci(
                            n,
                            &[
                                "FROM", "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION",
                                "INTERSECT", "EXCEPT", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL",
                                "CROSS", "SET", "VALUES", "RETURNING", "INTO", "ON", "WINDOW",
                            ],
                        )
                }
            };
            if is_trailing_comma {
                out.push(SqlDiagnostic {
                    start_byte: tok.start,
                    end_byte: tok.end,
                    severity: SqlDiagSeverity::Error,
                    source: SqlDiagSource::StaticLint,
                    message: "Unexpected trailing comma `,`".into(),
                    recommendation: Some("Remove the extra comma before the next clause or closing parenthesis.".into()),
                    fixes: vec![SqlQuickFix {
                        title: "Remove trailing comma".into(),
                        start_byte: tok.start,
                        end_byte: tok.end,
                        replacement: String::new(),
                    }],
                });
            }
        }

        if paren_depth == 0 {
            if is_word_ci(tok, "SELECT") && top_select_idx.is_none() {
                top_select_idx = Some(i);
                in_predicate_clause = false;
                if sig.get(i + 1).is_some_and(|n| is_word_ci(n, "FROM")) {
                    out.push(SqlDiagnostic {
                        start_byte: tok.start,
                        end_byte: sig[i + 1].end,
                        severity: SqlDiagSeverity::Error,
                        source: SqlDiagSource::StaticLint,
                        message: "Empty SELECT column list before FROM".into(),
                        recommendation: Some("Specify columns to select or use `*`.".into()),
                        fixes: vec![SqlQuickFix {
                            title: "Insert `*` after SELECT".into(),
                            start_byte: tok.end,
                            end_byte: tok.end,
                            replacement: " *".into(),
                        }],
                    });
                } else if sig.get(i + 1).is_some_and(|n| n.text == "*") {
                    if let Some(after_star) = sig.get(i + 2) {
                        if matches!(after_star.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier)
                            && detect_inline_clause_typo(sig, i + 2).is_none()
                        {
                            out.push(SqlDiagnostic {
                                start_byte: after_star.start,
                                end_byte: after_star.end,
                                severity: SqlDiagSeverity::Error,
                                source: SqlDiagSource::StaticLint,
                                message: format!("Missing `FROM` keyword before `{}`", after_star.text),
                                recommendation: Some(format!("Add `FROM` before `{}`.", after_star.text)),
                                fixes: vec![SqlQuickFix {
                                    title: format!("Insert `FROM` before `{}`", after_star.text),
                                    start_byte: after_star.start,
                                    end_byte: after_star.start,
                                    replacement: "FROM ".into(),
                                }],
                            });
                        }
                    }
                }
            } else if is_word_ci(tok, "FROM") && top_from_idx.is_none() {
                top_from_idx = Some(i);
                in_predicate_clause = false;
            } else if is_word_ci(tok, "WHERE") {
                has_top_where = true;
                if top_where_idx.is_none() {
                    top_where_idx = Some(i);
                }
                in_predicate_clause = true;
            } else if is_any_word_ci(tok, &["HAVING", "ON", "WHEN"]) {
                in_predicate_clause = true;
            } else if is_any_word_ci(tok, &["SET", "VALUES", "GROUP", "ORDER", "LIMIT", "OFFSET", "RETURNING"]) {
                in_predicate_clause = false;
            } else if is_any_word_ci(tok, &["UNION", "INTERSECT", "EXCEPT"]) {
                top_select_idx = None;
                top_from_idx = None;
                top_where_idx = None;
                in_predicate_clause = false;
            }
        } else if is_any_word_ci(tok, &["WHERE", "HAVING", "ON", "WHEN"]) {
            in_predicate_clause = true;
        } else if is_any_word_ci(tok, &["SELECT", "SET", "VALUES", "FROM", "GROUP", "ORDER", "LIMIT"]) {
            in_predicate_clause = false;
        }

        if is_word_ci(tok, "GROUP") || is_word_ci(tok, "ORDER") {
            let next_is_by = sig.get(i + 1).is_some_and(|n| is_word_ci(n, "BY"));
            let prev_is_dot = i > 0 && sig[i - 1].text == ".";
            let next_is_paren = sig.get(i + 1).is_some_and(|n| n.text == "(");
            if !next_is_by && !prev_is_dot && !next_is_paren {
                let kw = tok.text.to_ascii_uppercase();
                out.push(SqlDiagnostic {
                    start_byte: tok.start,
                    end_byte: tok.end,
                    severity: SqlDiagSeverity::Error,
                    source: SqlDiagSource::StaticLint,
                    message: format!("`{kw}` must be followed by `BY`"),
                    recommendation: Some(format!("Use `{kw} BY <column>`." )),
                    fixes: vec![SqlQuickFix {
                        title: format!("Insert `BY` after `{kw}`"),
                        start_byte: tok.end,
                        end_byte: tok.end,
                        replacement: " BY".into(),
                    }],
                });
            }
        }

        if in_predicate_clause && matches!(tok.text, "=" | "!=" | "<>") {
            if let Some(next) = sig.get(i + 1) {
                if is_word_ci(next, "NULL") {
                    let is_eq = tok.text == "=";
                    let replacement = if is_eq { "IS NULL" } else { "IS NOT NULL" };
                    out.push(SqlDiagnostic {
                        start_byte: tok.start,
                        end_byte: next.end,
                        severity: SqlDiagSeverity::Warning,
                        source: SqlDiagSource::StaticLint,
                        message: format!("Comparing with NULL using `{}` always evaluates to UNKNOWN", tok.text),
                        recommendation: Some(format!("Use `{replacement}` instead.")),
                        fixes: vec![SqlQuickFix {
                            title: format!("Change to `{replacement}`"),
                            start_byte: tok.start,
                            end_byte: next.end,
                            replacement: replacement.into(),
                        }],
                    });
                }
            }
        }

        if tok.kind == TokenKind::Identifier
            && i > 0
            && sig[i - 1].text != "."
            && !is_any_word_ci(sig[i - 1], &["FROM", "JOIN", "UPDATE", "INTO", "AS", "TABLE", "INDEX", "VIEW"])
            && !matches_any_table_or_col(tok.text, tables)
        {
            if let Some(kw) = detect_inline_clause_typo(sig, i) {
                out.push(SqlDiagnostic {
                    start_byte: tok.start,
                    end_byte: tok.end,
                    severity: SqlDiagSeverity::Error,
                    source: SqlDiagSource::StaticLint,
                    message: format!("Possible keyword typo `{}` — did you mean `{kw}`?", tok.text),
                    recommendation: Some(format!("Replace `{}` with `{kw}`.", tok.text)),
                    fixes: vec![SqlQuickFix {
                        title: format!("Replace `{}` with `{kw}`", tok.text),
                        start_byte: tok.start,
                        end_byte: tok.end,
                        replacement: kw.to_string(),
                    }],
                });
            }
        }

        i += 1;
    }

    if let (Some(where_i), Some(from_i)) = (top_where_idx, top_from_idx) {
        if where_i < from_i && top_select_idx.is_some_and(|sel_i| sel_i < where_i) {
            let wtok = sig[where_i];
            out.push(SqlDiagnostic {
                start_byte: wtok.start,
                end_byte: wtok.end,
                severity: SqlDiagSeverity::Error,
                source: SqlDiagSource::StaticLint,
                message: "`WHERE` clause appears before `FROM`".into(),
                recommendation: Some("Place the `FROM <table>` clause before `WHERE <condition>`.".into()),
                fixes: Vec::new(),
            });
        }
    }

    if is_dml_write && !has_top_where && sig.len() >= 2 {
        let verb = first.text.to_ascii_uppercase();
        let stmt_end = sig.last().map(|t| t.end).unwrap_or(sql.len());
        out.push(SqlDiagnostic {
            start_byte: first.start,
            end_byte: first.end,
            severity: SqlDiagSeverity::Warning,
            source: SqlDiagSource::StaticLint,
            message: format!("`{verb}` statement without a `WHERE` clause affects every row"),
            recommendation: Some("Add a `WHERE` predicate before running to avoid unintended bulk changes.".into()),
            fixes: vec![SqlQuickFix {
                title: "Append ` WHERE `".into(),
                start_byte: stmt_end,
                end_byte: stmt_end,
                replacement: " WHERE ".into(),
            }],
        });
    }
}

fn detect_inline_clause_typo(sig: &[&Token<'_>], idx: usize) -> Option<&'static str> {
    let tok = sig[idx];
    let prev = sig[idx - 1];
    let next = sig.get(idx + 1).copied();

    let prev_completes_expr = matches!(
        prev.kind,
        TokenKind::Identifier | TokenKind::QuotedIdentifier | TokenKind::Number | TokenKind::StringLiteral
    ) || prev.text == "*"
        || prev.text == ")";

    if !prev_completes_expr {
        return None;
    }

    let upper = tok.text.to_ascii_uppercase();
    match upper.as_str() {
        "FORM" | "FOMR" | "FRMO" | "FRON" => {
            if next.is_some_and(|n| matches!(n.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) || n.text == "(") {
                return Some("FROM");
            }
        }
        "WHER" | "WHRE" | "WEHRE" | "WERE" => {
            if next.is_some() {
                return Some("WHERE");
            }
        }
        "GROPU" | "GORUP" | "GRUOP" => {
            if next.is_some_and(|n| is_word_ci(n, "BY")) {
                return Some("GROUP");
            }
        }
        "ODER" | "ORDR" | "OREDR" | "OERDER" => {
            if next.is_some_and(|n| is_word_ci(n, "BY")) {
                return Some("ORDER");
            }
        }
        "LIMTI" | "LMIT" | "LIIMT" => {
            if next.is_some_and(|n| n.kind == TokenKind::Number) {
                return Some("LIMIT");
            }
        }
        "HAVNG" | "HAIVNG" => {
            if next.is_some() {
                return Some("HAVING");
            }
        }
        "JOINN" | "JION" => {
            if next.is_some_and(|n| matches!(n.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier)) {
                return Some("JOIN");
            }
        }
        "VALUSE" | "VAULES" => {
            if next.is_some_and(|n| n.text == "(") {
                return Some("VALUES");
            }
        }
        _ => {}
    }
    None
}

fn check_dialect_rules(sig: &[&Token<'_>], dialect: SqlDialect, out: &mut Vec<SqlDiagnostic>) {
    for (i, tok) in sig.iter().enumerate() {
        if tok.kind == TokenKind::QuotedIdentifier && tok.text.starts_with('`') && tok.text.ends_with('`') {
            match dialect {
                SqlDialect::PostgreSql | SqlDialect::CockroachDb => {
                    let inner = &tok.text[1..tok.text.len() - 1];
                    let replacement = format!("\"{}\"", inner.replace('"', "\"\""));
                    out.push(SqlDiagnostic {
                        start_byte: tok.start,
                        end_byte: tok.end,
                        severity: SqlDiagSeverity::Warning,
                        source: SqlDiagSource::StaticLint,
                        message: "PostgreSQL uses double quotes `\"...\"` instead of backticks".into(),
                        recommendation: Some(format!("Quote the identifier as {replacement}.")),
                        fixes: vec![SqlQuickFix {
                            title: format!("Change to {replacement}"),
                            start_byte: tok.start,
                            end_byte: tok.end,
                            replacement,
                        }],
                    });
                }
                SqlDialect::SqlServer => {
                    let inner = &tok.text[1..tok.text.len() - 1];
                    let replacement = format!("[{}]", inner.replace(']', "]]"));
                    out.push(SqlDiagnostic {
                        start_byte: tok.start,
                        end_byte: tok.end,
                        severity: SqlDiagSeverity::Warning,
                        source: SqlDiagSource::StaticLint,
                        message: "SQL Server uses `[...]` brackets instead of backticks".into(),
                        recommendation: Some(format!("Quote the identifier as {replacement}.")),
                        fixes: vec![SqlQuickFix {
                            title: format!("Change to {replacement}"),
                            start_byte: tok.start,
                            end_byte: tok.end,
                            replacement,
                        }],
                    });
                }
                SqlDialect::MySql | SqlDialect::Sqlite => {}
            }
        }

        if dialect == SqlDialect::SqlServer && is_word_ci(tok, "LIMIT") {
            let end = sig
                .get(i + 1)
                .filter(|n| n.kind == TokenKind::Number)
                .map(|n| n.end)
                .unwrap_or(tok.end);
            out.push(SqlDiagnostic {
                start_byte: tok.start,
                end_byte: end,
                severity: SqlDiagSeverity::Error,
                source: SqlDiagSource::StaticLint,
                message: "`LIMIT` is not valid in SQL Server".into(),
                recommendation: Some("Use `SELECT TOP (N)` or `ORDER BY ... OFFSET 0 ROWS FETCH NEXT N ROWS ONLY`.".into()),
                fixes: Vec::new(),
            });
        }

        if dialect.is_postgres_family()
            && tok.text.eq_ignore_ascii_case("IFNULL")
            && sig.get(i + 1).is_some_and(|n| n.text == "(")
        {
            out.push(SqlDiagnostic {
                start_byte: tok.start,
                end_byte: tok.end,
                severity: SqlDiagSeverity::Warning,
                source: SqlDiagSource::StaticLint,
                message: "`IFNULL()` is MySQL/SQLite syntax — PostgreSQL uses `COALESCE()`".into(),
                recommendation: Some("Replace `IFNULL` with standard `COALESCE`.".into()),
                fixes: vec![SqlQuickFix {
                    title: "Replace `IFNULL` with `COALESCE`".into(),
                    start_byte: tok.start,
                    end_byte: tok.end,
                    replacement: "COALESCE".into(),
                }],
            });
        }

        if !dialect.is_postgres_family() && is_word_ci(tok, "ILIKE") {
            out.push(SqlDiagnostic {
                start_byte: tok.start,
                end_byte: tok.end,
                severity: SqlDiagSeverity::Warning,
                source: SqlDiagSource::StaticLint,
                message: "`ILIKE` is PostgreSQL-specific".into(),
                recommendation: Some("Use `LIKE` (or `LOWER(col) LIKE LOWER(...)` for case-insensitive matching).".into()),
                fixes: vec![SqlQuickFix {
                    title: "Replace `ILIKE` with `LIKE`".into(),
                    start_byte: tok.start,
                    end_byte: tok.end,
                    replacement: "LIKE".into(),
                }],
            });
        }

        if dialect != SqlDialect::SqlServer
            && is_word_ci(tok, "TOP")
            && i > 0
            && is_word_ci(sig[i - 1], "SELECT")
            && sig.get(i + 1).is_some_and(|n| n.kind == TokenKind::Number || n.text == "(")
        {
            out.push(SqlDiagnostic {
                start_byte: tok.start,
                end_byte: tok.end,
                severity: SqlDiagSeverity::Warning,
                source: SqlDiagSource::StaticLint,
                message: "`SELECT TOP` is SQL Server syntax".into(),
                recommendation: Some("Use `LIMIT N` at the end of the query instead.".into()),
                fixes: Vec::new(),
            });
        }
    }
}

fn check_schema_references(sig: &[&Token<'_>], tables: &[SchemaTableHint], out: &mut Vec<SqlDiagnostic>) {
    let cte_names = extract_cte_names(sig);
    let table_map: HashMap<String, &SchemaTableHint> = tables
        .iter()
        .map(|t| (t.name.to_ascii_lowercase(), t))
        .collect();

    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut i = 0usize;
    while i < sig.len() {
        let tok = sig[i];
        if is_any_word_ci(tok, &["FROM", "JOIN", "UPDATE", "INTO"]) {
            if let Some(tbl_tok) = sig.get(i + 1) {
                if matches!(tbl_tok.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) {
                    let is_schema_qualified = sig.get(i + 2).is_some_and(|n| n.text == ".");
                    if is_schema_qualified {
                        if let Some(sub_tok) = sig.get(i + 3) {
                            let sub_clean = strip_ident_quotes(sub_tok.text);
                            aliases.insert(sub_clean.to_ascii_lowercase(), sub_clean.clone());
                            register_alias_after(sig, i + 4, &sub_clean, &mut aliases);
                        }
                    } else {
                        let clean = strip_ident_quotes(tbl_tok.text);
                        let lower = clean.to_ascii_lowercase();
                        aliases.insert(lower.clone(), clean.clone());
                        register_alias_after(sig, i + 2, &clean, &mut aliases);

                        let is_ddl = sig[0].text.eq_ignore_ascii_case("CREATE")
                            || sig[0].text.eq_ignore_ascii_case("DROP")
                            || sig[0].text.eq_ignore_ascii_case("ALTER");
                        let is_func_call = sig.get(i + 2).is_some_and(|n| n.text == "(");
                        if !is_ddl
                            && !is_func_call
                            && !cte_names.contains(&lower)
                            && !table_map.contains_key(&lower)
                        {
                            let mut fixes = Vec::new();
                            let recommendation = if let Some(best) =
                                best_fuzzy_match(&clean, tables.iter().map(|t| t.name.as_str()))
                            {
                                let replacement = preserve_ident_quotes(tbl_tok.text, best);
                                fixes.push(SqlQuickFix {
                                    title: format!("Replace `{clean}` with `{best}`"),
                                    start_byte: tbl_tok.start,
                                    end_byte: tbl_tok.end,
                                    replacement,
                                });
                                Some(format!("Did you mean table `{best}`?"))
                            } else {
                                Some("Check the table name or switch to the schema that owns this table.".into())
                            };
                            out.push(SqlDiagnostic {
                                start_byte: tbl_tok.start,
                                end_byte: tbl_tok.end,
                                severity: SqlDiagSeverity::Warning,
                                source: SqlDiagSource::StaticLint,
                                message: format!("Table `{clean}` not found in active schema"),
                                recommendation,
                                fixes,
                            });
                        }
                    }
                }
            }
        }
        i += 1;
    }

    for j in 0..sig.len().saturating_sub(2) {
        if sig[j + 1].text != "." {
            continue;
        }
        let qual_tok = sig[j];
        let col_tok = sig[j + 2];
        if !matches!(qual_tok.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) {
            continue;
        }
        if !matches!(col_tok.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) {
            continue;
        }
        if j > 0 && is_any_word_ci(sig[j - 1], &["FROM", "JOIN", "UPDATE", "INTO", "TABLE"]) {
            continue;
        }
        if sig.get(j + 3).is_some_and(|n| n.text == "(") {
            continue;
        }

        let qual = strip_ident_quotes(qual_tok.text).to_ascii_lowercase();
        let Some(table_name) = aliases.get(&qual) else {
            continue;
        };
        let Some(table) = table_map.get(&table_name.to_ascii_lowercase()) else {
            continue;
        };
        if table.columns.is_empty() {
            continue;
        }
        let col_clean = strip_ident_quotes(col_tok.text);
        let exists = table
            .columns
            .iter()
            .any(|(c, _)| c.eq_ignore_ascii_case(&col_clean));
        if !exists {
            let mut fixes = Vec::new();
            let recommendation = if let Some(best) =
                best_fuzzy_match(&col_clean, table.columns.iter().map(|(c, _)| c.as_str()))
            {
                let replacement = preserve_ident_quotes(col_tok.text, best);
                fixes.push(SqlQuickFix {
                    title: format!("Replace `{col_clean}` with `{best}`"),
                    start_byte: col_tok.start,
                    end_byte: col_tok.end,
                    replacement,
                });
                Some(format!("Did you mean column `{best}` on `{}`?", table.name))
            } else {
                Some(format!("Verify column names on table `{}`.", table.name))
            };
            out.push(SqlDiagnostic {
                start_byte: col_tok.start,
                end_byte: col_tok.end,
                severity: SqlDiagSeverity::Warning,
                source: SqlDiagSource::StaticLint,
                message: format!("Column `{col_clean}` not found on table `{}`", table.name),
                recommendation,
                fixes,
            });
        }
    }
}

fn extract_cte_names(sig: &[&Token<'_>]) -> HashSet<String> {
    let mut ctes = HashSet::new();
    if !sig.first().is_some_and(|t| is_word_ci(t, "WITH")) {
        return ctes;
    }
    let mut i = 1usize;
    if sig.get(i).is_some_and(|t| is_word_ci(t, "RECURSIVE")) {
        i += 1;
    }
    let mut paren_depth = 0usize;
    while i < sig.len() {
        let tok = sig[i];
        if tok.text == "(" {
            paren_depth += 1;
        } else if tok.text == ")" {
            paren_depth = paren_depth.saturating_sub(1);
        } else if paren_depth == 0 {
            if is_any_word_ci(tok, &["SELECT", "INSERT", "UPDATE", "DELETE", "MERGE"]) {
                break;
            }
            if matches!(tok.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) {
                let mut look = i + 1;
                if sig.get(look).is_some_and(|n| n.text == "(") {
                    let mut d = 1usize;
                    look += 1;
                    while look < sig.len() && d > 0 {
                        if sig[look].text == "(" {
                            d += 1;
                        } else if sig[look].text == ")" {
                            d -= 1;
                        }
                        look += 1;
                    }
                }
                if sig.get(look).is_some_and(|n| is_word_ci(n, "AS")) {
                    ctes.insert(strip_ident_quotes(tok.text).to_ascii_lowercase());
                }
            }
        }
        i += 1;
    }
    ctes
}

fn register_alias_after(
    sig: &[&Token<'_>],
    idx: usize,
    table_name: &str,
    aliases: &mut HashMap<String, String>,
) {
    let Some(cand) = sig.get(idx) else {
        return;
    };
    if is_word_ci(cand, "AS") {
        if let Some(alias) = sig.get(idx + 1) {
            if matches!(alias.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) {
                aliases.insert(strip_ident_quotes(alias.text).to_ascii_lowercase(), table_name.to_string());
            }
        }
    } else if matches!(cand.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) {
        aliases.insert(strip_ident_quotes(cand.text).to_ascii_lowercase(), table_name.to_string());
    }
}

fn extract_stmt_aliases(sql: &str) -> HashMap<String, String> {
    let tokens = tokenize(sql);
    let sig: Vec<&Token<'_>> = tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    let mut aliases = HashMap::new();
    for i in 0..sig.len() {
        if is_any_word_ci(sig[i], &["FROM", "JOIN", "UPDATE", "INTO"]) {
            if let Some(tbl) = sig.get(i + 1) {
                if matches!(tbl.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) {
                    let (name, next_i) = if sig.get(i + 2).is_some_and(|n| n.text == ".") {
                        if let Some(sub) = sig.get(i + 3) {
                            (strip_ident_quotes(sub.text), i + 4)
                        } else {
                            continue;
                        }
                    } else {
                        (strip_ident_quotes(tbl.text), i + 2)
                    };
                    aliases.insert(name.to_ascii_lowercase(), name.clone());
                    register_alias_after(&sig, next_i, &name, &mut aliases);
                }
            }
        }
    }
    aliases
}

fn collect_candidate_columns(
    qualifier: Option<&str>,
    aliases: &HashMap<String, String>,
    tables: &[SchemaTableHint],
) -> Vec<String> {
    if let Some(q) = qualifier {
        let resolved = aliases.get(&q.to_ascii_lowercase()).map(String::as_str).unwrap_or(q);
        if let Some(tbl) = tables.iter().find(|t| t.name.eq_ignore_ascii_case(resolved)) {
            return tbl.columns.iter().map(|(c, _)| c.clone()).collect();
        }
    }
    let mut cols = Vec::new();
    for tbl in tables {
        let is_ref = aliases.is_empty() || aliases.values().any(|n| n.eq_ignore_ascii_case(&tbl.name));
        if is_ref {
            for (c, _) in &tbl.columns {
                cols.push(c.clone());
            }
        }
    }
    cols
}

fn matches_any_table_or_col(word: &str, tables: &[SchemaTableHint]) -> bool {
    tables.iter().any(|t| {
        t.name.eq_ignore_ascii_case(word) || t.columns.iter().any(|(c, _)| c.eq_ignore_ascii_case(word))
    })
}

fn suggest_keyword_typo<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    if word.len() < 3 || word.len() > 16 || !word.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_') {
        return None;
    }
    let upper = word.to_ascii_uppercase();
    if candidates.iter().any(|&c| c == upper) {
        return None;
    }
    let max_dist = if upper.len() <= 4 { 1 } else { 2 };
    let mut best: Option<(&'a str, usize)> = None;
    for &cand in candidates {
        if cand.len().abs_diff(upper.len()) > max_dist {
            continue;
        }
        let dist = damerau_levenshtein(&upper, cand);
        if dist > 0 && dist <= max_dist && best.is_none_or(|(_, d)| dist < d) {
            best = Some((cand, dist));
        }
    }
    best.map(|(k, _)| k)
}

fn best_fuzzy_match<'a>(needle: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    if needle.len() < 2 {
        return None;
    }
    let lower = needle.to_ascii_lowercase();
    let max_dist = match lower.len() {
        0..=2 => 1,
        3..=5 => 2,
        _ => 3,
    };
    let mut best: Option<(&'a str, usize)> = None;
    for cand in candidates {
        let cand_lower = cand.to_ascii_lowercase();
        if cand_lower == lower {
            return None;
        }
        if cand_lower.len().abs_diff(lower.len()) > max_dist {
            continue;
        }
        let dist = damerau_levenshtein(&lower, &cand_lower);
        if dist > 0 && dist <= max_dist && best.is_none_or(|(_, d)| dist < d) {
            best = Some((cand, dist));
        }
    }
    best.map(|(c, _)| c)
}

fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = a.len();
    let m = b.len();
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate().take(n + 1) {
        row[0] = i;
    }
    for j in 0..=m {
        dp[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut val = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                val = val.min(dp[i - 2][j - 2] + 1);
            }
            dp[i][j] = val;
        }
    }
    dp[n][m]
}

fn strip_ident_quotes(text: &str) -> String {
    text.trim_matches(|c| matches!(c, '`' | '"' | '[' | ']')).to_string()
}

fn preserve_ident_quotes(orig: &str, replacement: &str) -> String {
    if orig.starts_with('`') && orig.ends_with('`') {
        format!("`{replacement}`")
    } else if orig.starts_with('"') && orig.ends_with('"') {
        format!("\"{replacement}\"")
    } else if orig.starts_with('[') && orig.ends_with(']') {
        format!("[{replacement}]")
    } else {
        replacement.to_string()
    }
}

fn format_ident_matching_style(orig_slice: &str, replacement: &str, dialect: SqlDialect) -> String {
    if orig_slice.starts_with('`') || orig_slice.starts_with('"') || orig_slice.starts_with('[') {
        dialect.quote_ident(replacement)
    } else {
        replacement.to_string()
    }
}

fn is_missing_from_before_offset(full_sql: &str, stmt_base: usize, abs_start: usize) -> bool {
    if abs_start <= stmt_base || abs_start > full_sql.len() {
        return false;
    }
    let before = &full_sql[stmt_base..abs_start];
    let sig: Vec<Token<'_>> = tokenize(before)
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    if sig.len() >= 2
        && is_word_ci(&sig[sig.len() - 2], "SELECT")
        && sig[sig.len() - 1].text == "*"
    {
        return true;
    }
    false
}

fn is_word_ci(tok: &Token<'_>, expected: &str) -> bool {
    matches!(tok.kind, TokenKind::Keyword | TokenKind::Identifier) && tok.text.eq_ignore_ascii_case(expected)
}

fn is_any_word_ci(tok: &Token<'_>, words: &[&str]) -> bool {
    matches!(tok.kind, TokenKind::Keyword | TokenKind::Identifier)
        && words.iter().any(|&w| tok.text.eq_ignore_ascii_case(w))
}

fn clamp_boundary(s: &str, mut byte: usize) -> usize {
    byte = byte.min(s.len());
    while byte > 0 && !s.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

fn char_offset_to_byte(s: &str, char_offset: usize) -> usize {
    s.char_indices()
        .nth(char_offset)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        for pos in 0..=h.len() - n.len() {
            if h[pos..pos + n.len()].eq_ignore_ascii_case(n) {
                return Some(pos);
            }
        }
        return None;
    }
    haystack.to_lowercase().find(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cottondata_core::SqlErrorLocation;

    fn sample_tables() -> Vec<SchemaTableHint> {
        vec![
            SchemaTableHint {
                name: "users".into(),
                columns: vec![
                    ("id".into(), "bigint".into()),
                    ("email".into(), "varchar".into()),
                    ("created_at".into(), "timestamp".into()),
                ],
            },
            SchemaTableHint {
                name: "orders".into(),
                columns: vec![
                    ("id".into(), "bigint".into()),
                    ("user_id".into(), "bigint".into()),
                    ("total_amount".into(), "decimal".into()),
                ],
            },
        ]
    }

    #[test]
    fn flags_trailing_comma_and_provides_quick_fix() {
        let sql = "SELECT id, email, FROM users;";
        let diags = lint_sql(sql, SqlDialect::MySql, &sample_tables(), true);
        let d = diags
            .iter()
            .find(|d| d.message.contains("trailing comma"))
            .expect("trailing comma diagnostic");
        let (fixed, _) = SqlDiagnostic::apply_fix(sql, &d.fixes[0]).expect("apply fix");
        assert_eq!(fixed, "SELECT id, email FROM users;");
    }

    #[test]
    fn flags_keyword_typos_and_null_equality_without_false_positives_in_set() {
        let sql = "SELCT * FORM users WHERE email = NULL;";
        let diags = lint_sql(sql, SqlDialect::MySql, &sample_tables(), true);
        assert!(diags.iter().any(|d| d.fixes.iter().any(|f| f.replacement == "SELECT")));
        assert!(diags.iter().any(|d| d.fixes.iter().any(|f| f.replacement == "FROM")));
        assert!(diags.iter().any(|d| d.fixes.iter().any(|f| f.replacement == "IS NULL")));

        let valid_update = "UPDATE users SET email = NULL WHERE id = 1;";
        let update_diags = lint_sql(valid_update, SqlDialect::MySql, &sample_tables(), true);
        assert!(update_diags.is_empty(), "unexpected diagnostics: {update_diags:?}");
    }

    #[test]
    fn flags_unknown_table_and_column_with_fuzzy_suggestions_and_skips_ctes() {
        let sql = "WITH active_users AS (SELECT id FROM users) SELECT a.id, u.emial FROM active_users a JOIN users u ON u.id = a.id; SELECT * FROM usres;";
        let diags = lint_sql(sql, SqlDialect::PostgreSql, &sample_tables(), true);
        assert!(!diags.iter().any(|d| d.message.contains("active_users")));
        assert!(diags
            .iter()
            .any(|d| d.message.contains("usres") && d.fixes.iter().any(|f| f.replacement == "users")));
        assert!(diags
            .iter()
            .any(|d| d.message.contains("emial") && d.fixes.iter().any(|f| f.replacement == "email")));
    }

    #[test]
    fn flags_dialect_mismatches_and_unclosed_blocks() {
        let pg_sql = "SELECT IFNULL(`email`, 'x') FROM users WHERE (id = 1";
        let diags = lint_sql(pg_sql, SqlDialect::PostgreSql, &sample_tables(), true);
        assert!(diags.iter().any(|d| d.fixes.iter().any(|f| f.replacement == "COALESCE")));
        assert!(diags.iter().any(|d| d.fixes.iter().any(|f| f.replacement == "\"email\"")));
        assert!(diags.iter().any(|d| d.message.contains("Unclosed parenthesis")));
    }

    #[test]
    fn resolves_server_error_offset_and_suggests_schema_fix() {
        let full_sql = "SELECT 1;\nSELECT u.emial FROM users u;";
        let stmt_start = full_sql.find("SELECT u.emial").unwrap();
        let db_diag = DbDiagnostic {
            code: Some(1054),
            sqlstate: None,
            severity: cottondata_core::DiagnosticSeverity::Error,
            summary: "Unknown column 'u.emial' in 'field list'".into(),
            hint: None,
            raw_message: "Unknown column 'u.emial' in 'field list'".into(),
            location: Some(Box::new(SqlErrorLocation {
                object_name: Some("u.emial".into()),
                object_kind: Some(SqlErrorObjectKind::Column),
                ..Default::default()
            })),
        };
        let resolved = resolve_server_error_diagnostic(
            full_sql,
            (stmt_start, full_sql.len()),
            &db_diag,
            SqlDialect::MySql,
            &sample_tables(),
        )
        .expect("resolved diagnostic");
        assert_eq!(&full_sql[resolved.start_byte..resolved.end_byte], "u.emial");
        assert_eq!(resolved.fixes[0].replacement, "u.email");
    }

    #[test]
    fn flags_missing_from_after_select_star_and_offers_insert_from_fix() {
        let sql = "select * psw_resend_audit";
        let diags = lint_sql(sql, SqlDialect::MySql, &sample_tables(), false);
        let d = diags
            .iter()
            .find(|d| d.message.contains("Missing `FROM`"))
            .expect("missing FROM lint");
        let (fixed, _) = SqlDiagnostic::apply_fix(sql, &d.fixes[0]).expect("apply fix");
        assert_eq!(fixed, "select * FROM psw_resend_audit");

        let db_diag = DbDiagnostic::classify(
            "MySqlError { ERROR 1064 (42000): You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near 'psw_resend_audit' at line 1 }",
            Some(1064),
        );
        assert_eq!(db_diag.summary, "SQL syntax error near 'psw_resend_audit' (ERROR 1064)");
        let resolved = resolve_server_error_diagnostic(
            sql,
            (0, sql.len()),
            &db_diag,
            SqlDialect::MySql,
            &sample_tables(),
        )
        .expect("resolved");
        assert_eq!(&sql[resolved.start_byte..resolved.end_byte], "psw_resend_audit");
        assert_eq!(resolved.fixes[0].replacement, "FROM ");
    }
}
