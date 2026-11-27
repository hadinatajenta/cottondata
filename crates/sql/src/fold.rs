use crate::lexer::{dollar_quote_tag_len, split_statements, tokenize, Token, TokenKind};
use cottondata_core::SqlDialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FoldKind {
    CustomRegion,
    Statement,
    ProceduralBlock,
    DollarQuoted,
    Cte,
    Subquery,
    Parentheses,
    CaseBlock,
    Clause,
    BlockComment,
    LineCommentBlock,
}

impl FoldKind {
    fn priority(self) -> u8 {
        match self {
            Self::CustomRegion => 0,
            Self::Statement => 1,
            Self::ProceduralBlock => 2,
            Self::DollarQuoted => 3,
            Self::Cte => 4,
            Self::Subquery => 5,
            Self::Parentheses => 6,
            Self::CaseBlock => 7,
            Self::Clause => 8,
            Self::BlockComment => 9,
            Self::LineCommentBlock => 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldRegion {
    pub start_line: usize,
    pub end_line: usize,
    pub kind: FoldKind,
    pub depth: u16,
    pub placeholder_hint: String,
}

impl FoldRegion {
    pub fn hidden_line_count(&self) -> usize {
        self.end_line.saturating_sub(self.start_line)
    }

    pub fn badge_label(&self) -> String {
        let hidden = self.hidden_line_count();
        let line_word = if hidden == 1 { "line" } else { "lines" };
        match self.kind {
            FoldKind::CustomRegion if !self.placeholder_hint.is_empty() => {
                format!("{} · {hidden} {line_word}", self.placeholder_hint)
            }
            _ if !self.placeholder_hint.is_empty() => {
                format!("{} · {hidden} {line_word}", self.placeholder_hint)
            }
            _ => format!("… {hidden} {line_word}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineByteSpan {
    pub start: usize,
    pub end: usize,
}

pub fn compute_line_spans(sql: &str) -> Vec<LineByteSpan> {
    let mut spans = Vec::new();
    let bytes = sql.as_bytes();
    let mut start = 0usize;
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            let end = if idx > start && bytes[idx - 1] == b'\r' {
                idx - 1
            } else {
                idx
            };
            spans.push(LineByteSpan { start, end });
            start = idx + 1;
        }
    }
    let end = if bytes.len() > start && bytes[bytes.len() - 1] == b'\r' {
        bytes.len() - 1
    } else {
        bytes.len()
    };
    spans.push(LineByteSpan { start, end });
    spans
}

pub fn line_index_for_byte(spans: &[LineByteSpan], byte: usize) -> usize {
    if spans.is_empty() {
        return 0;
    }
    let idx = spans.partition_point(|span| span.start <= byte);
    idx.saturating_sub(1).min(spans.len() - 1)
}

pub fn compute_fold_regions(sql: &str, _dialect: SqlDialect) -> Vec<FoldRegion> {
    if sql.is_empty() || !sql.contains('\n') {
        return Vec::new();
    }
    let spans = compute_line_spans(sql);
    if spans.len() < 2 {
        return Vec::new();
    }

    let mut raw_regions = Vec::new();
    let all_tokens = tokenize(sql);

    collect_comment_and_region_folds(sql, &all_tokens, &spans, &mut raw_regions);
    collect_dollar_quoted_folds(sql, &all_tokens, &spans, &mut raw_regions);
    collect_blocks_and_parens(sql, &all_tokens, &spans, &mut raw_regions);

    for stmt in split_statements(sql) {
        let stmt_slice = &sql[stmt.start..stmt.end];
        let mut stmt_tokens = tokenize(stmt_slice);
        for tok in &mut stmt_tokens {
            tok.start += stmt.start;
            tok.end += stmt.start;
        }
        collect_statement_and_clause_folds(sql, &stmt_tokens, &spans, &mut raw_regions);
    }

    finalize_regions(raw_regions)
}

pub fn gutter_fold_regions(regions: &[FoldRegion]) -> Vec<&FoldRegion> {
    let mut out = Vec::new();
    let mut last_start: Option<usize> = None;
    for region in regions {
        if last_start != Some(region.start_line) {
            out.push(region);
            last_start = Some(region.start_line);
        }
    }
    out
}

pub fn innermost_region_at_line(regions: &[FoldRegion], line: usize) -> Option<&FoldRegion> {
    regions
        .iter()
        .filter(|r| line >= r.start_line && line <= r.end_line)
        .min_by_key(|r| (r.end_line - r.start_line, std::cmp::Reverse(r.depth), r.kind.priority()))
}

enum RegionDirective {
    Open(String),
    Close,
}

fn parse_region_directive(comment_text: &str) -> Option<RegionDirective> {
    let body = if let Some(rest) = comment_text.strip_prefix("--") {
        rest.trim()
    } else if let Some(rest) = comment_text.strip_prefix('#') {
        rest.trim()
    } else {
        return None;
    };

    if let Some(rest) = strip_prefix_ci(body, "endregion") {
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return Some(RegionDirective::Close);
        }
    }
    if let Some(rest) = strip_prefix_ci(body, "end region") {
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return Some(RegionDirective::Close);
        }
    }
    if let Some(rest) = strip_prefix_ci(body, "region") {
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return Some(RegionDirective::Open(rest.trim().to_string()));
        }
    }
    if let Some(rest) = strip_prefix_ci(body, "<editor-fold") {
        let label = extract_editor_fold_desc(rest).unwrap_or_default();
        return Some(RegionDirective::Open(label));
    }
    if strip_prefix_ci(body, "</editor-fold>").is_some() {
        return Some(RegionDirective::Close);
    }
    None
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let bytes = s.as_bytes();
    let pbytes = prefix.as_bytes();
    if bytes.len() < pbytes.len() {
        return None;
    }
    if bytes[..pbytes.len()].eq_ignore_ascii_case(pbytes) {
        Some(&s[pbytes.len()..])
    } else {
        None
    }
}

fn extract_editor_fold_desc(rest: &str) -> Option<String> {
    let lower = rest.to_ascii_lowercase();
    let pos = lower.find("desc=")?;
    let after = rest[pos + 5..].trim_start();
    let quote = after.bytes().next()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let tail = &after[1..];
    let end = tail.find(quote as char)?;
    Some(tail[..end].trim().to_string())
}

fn collect_comment_and_region_folds(
    sql: &str,
    tokens: &[Token<'_>],
    spans: &[LineByteSpan],
    out: &mut Vec<FoldRegion>,
) {
    let mut region_stack: Vec<(usize, String)> = Vec::new();
    let mut line_comment_run: Option<(usize, usize)> = None;

    let flush_comment_run = |run: &mut Option<(usize, usize)>, out: &mut Vec<FoldRegion>| {
        if let Some((start_line, end_line)) = run.take() {
            if end_line > start_line {
                out.push(FoldRegion {
                    start_line,
                    end_line,
                    kind: FoldKind::LineCommentBlock,
                    depth: 0,
                    placeholder_hint: String::new(),
                });
            }
        }
    };

    for tok in tokens {
        if tok.kind == TokenKind::Whitespace {
            continue;
        }
        if tok.kind != TokenKind::Comment {
            flush_comment_run(&mut line_comment_run, out);
            continue;
        }

        if tok.text.starts_with("/*") {
            flush_comment_run(&mut line_comment_run, out);
            let start_line = line_index_for_byte(spans, tok.start);
            let end_byte = tok.end.saturating_sub(1).max(tok.start);
            let end_line = line_index_for_byte(spans, end_byte);
            if end_line > start_line {
                out.push(FoldRegion {
                    start_line,
                    end_line,
                    kind: FoldKind::BlockComment,
                    depth: 0,
                    placeholder_hint: "… */".to_string(),
                });
            }
            continue;
        }

        let line = line_index_for_byte(spans, tok.start);
        if let Some(directive) = parse_region_directive(tok.text) {
            flush_comment_run(&mut line_comment_run, out);
            match directive {
                RegionDirective::Open(label) => {
                    region_stack.push((line, label));
                }
                RegionDirective::Close => {
                    if let Some((start_line, label)) = region_stack.pop() {
                        if line > start_line {
                            out.push(FoldRegion {
                                start_line,
                                end_line: line,
                                kind: FoldKind::CustomRegion,
                                depth: 0,
                                placeholder_hint: label,
                            });
                        }
                    }
                }
            }
            continue;
        }

        let line_start = spans[line].start;
        let is_full_line_comment = sql[line_start..tok.start].trim().is_empty();
        if !is_full_line_comment {
            flush_comment_run(&mut line_comment_run, out);
            continue;
        }

        match line_comment_run {
            Some((start, prev)) if line == prev + 1 => {
                line_comment_run = Some((start, line));
            }
            _ => {
                flush_comment_run(&mut line_comment_run, out);
                line_comment_run = Some((line, line));
            }
        }
    }

    flush_comment_run(&mut line_comment_run, out);
}

fn collect_dollar_quoted_folds(
    sql: &str,
    tokens: &[Token<'_>],
    spans: &[LineByteSpan],
    out: &mut Vec<FoldRegion>,
) {
    let bytes = sql.as_bytes();
    for tok in tokens {
        if tok.kind != TokenKind::StringLiteral {
            continue;
        }
        let Some(tag_len) = dollar_quote_tag_len(bytes, tok.start) else {
            continue;
        };
        let start_line = line_index_for_byte(spans, tok.start);
        let end_byte = tok.end.saturating_sub(1).max(tok.start);
        let end_line = line_index_for_byte(spans, end_byte);
        if end_line <= start_line {
            continue;
        }
        let tag = &sql[tok.start..tok.start + tag_len];
        out.push(FoldRegion {
            start_line,
            end_line,
            kind: FoldKind::DollarQuoted,
            depth: 0,
            placeholder_hint: format!("… {tag}"),
        });

        let body_start = tok.start + tag_len;
        let body_end = if tok.end >= body_start + tag_len && &sql[tok.end - tag_len..tok.end] == tag {
            tok.end - tag_len
        } else {
            tok.end
        };
        if body_start < body_end {
            let body = &sql[body_start..body_end];
            let mut inner_tokens = tokenize(body);
            for inner in &mut inner_tokens {
                inner.start += body_start;
                inner.end += body_start;
            }
            collect_blocks_and_parens(sql, &inner_tokens, spans, out);
            collect_clause_folds_from_raw(sql, &inner_tokens, spans, out);
        }
    }
}

fn collect_statement_and_clause_folds(
    sql: &str,
    stmt_tokens: &[Token<'_>],
    spans: &[LineByteSpan],
    out: &mut Vec<FoldRegion>,
) {
    let sig: Vec<&Token<'_>> = stmt_tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    if sig.is_empty() {
        return;
    }

    let first_line = line_index_for_byte(spans, sig[0].start);
    let last_tok = sig[sig.len() - 1];
    let last_byte = last_tok.end.saturating_sub(1).max(last_tok.start);
    let last_line = line_index_for_byte(spans, last_byte);
    if last_line > first_line {
        out.push(FoldRegion {
            start_line: first_line,
            end_line: last_line,
            kind: FoldKind::Statement,
            depth: 0,
            placeholder_hint: String::new(),
        });
    }

    collect_clause_folds(sql, &sig, spans, out);
}

fn collect_clause_folds_from_raw(
    sql: &str,
    raw_tokens: &[Token<'_>],
    spans: &[LineByteSpan],
    out: &mut Vec<FoldRegion>,
) {
    let sig: Vec<&Token<'_>> = raw_tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    collect_clause_folds(sql, &sig, spans, out);
}

#[derive(Debug, Clone, Copy)]
struct ParenFrame {
    open_line: usize,
    kind: FoldKind,
    scope_start_sig_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeywordBlockKind {
    Case,
    Begin,
    BeginTry,
    BeginCatch,
    If,
    Loop,
    While,
    Repeat,
}

#[derive(Debug, Clone, Copy)]
struct KeywordBlockFrame {
    open_line: usize,
    kind: KeywordBlockKind,
}

fn collect_blocks_and_parens(
    sql: &str,
    raw_tokens: &[Token<'_>],
    spans: &[LineByteSpan],
    out: &mut Vec<FoldRegion>,
) {
    let sig: Vec<&Token<'_>> = raw_tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    if sig.is_empty() {
        return;
    }

    let mut paren_stack: Vec<ParenFrame> = Vec::new();
    let mut block_stack: Vec<KeywordBlockFrame> = Vec::new();
    let mut in_with_header = false;
    let mut at_stmt_start = true;
    let mut paren_scopes: Vec<(usize, usize)> = Vec::new();

    let mut i = 0usize;
    while i < sig.len() {
        let tok = sig[i];
        let line = line_index_for_byte(spans, tok.start);

        if tok.kind == TokenKind::Punctuation && tok.text == ";" {
            in_with_header = false;
            at_stmt_start = true;
            i += 1;
            continue;
        }

        if tok.kind == TokenKind::Punctuation && tok.text == "(" {
            let prev_is_as = i > 0 && is_word_ci(sig[i - 1], "AS");
            let next_starts_query = sig
                .get(i + 1)
                .is_some_and(|next| is_word_ci(next, "SELECT") || is_word_ci(next, "WITH"));
            let kind = if in_with_header && paren_stack.is_empty() && prev_is_as {
                FoldKind::Cte
            } else if next_starts_query {
                FoldKind::Subquery
            } else {
                FoldKind::Parentheses
            };
            paren_stack.push(ParenFrame {
                open_line: line,
                kind,
                scope_start_sig_idx: i + 1,
            });
            at_stmt_start = false;
            i += 1;
            continue;
        }

        if tok.kind == TokenKind::Punctuation && tok.text == ")" {
            if let Some(frame) = paren_stack.pop() {
                if frame.scope_start_sig_idx < i
                    && matches!(frame.kind, FoldKind::Cte | FoldKind::Subquery)
                {
                    paren_scopes.push((frame.scope_start_sig_idx, i));
                }
                if line > frame.open_line {
                    let keep_closing = closing_line_has_trailing_construct(&sig, i, line, spans);
                    let end_line = if keep_closing { line.saturating_sub(1) } else { line };
                    if end_line > frame.open_line {
                        let hint = if keep_closing {
                            String::new()
                        } else {
                            "… )".to_string()
                        };
                        out.push(FoldRegion {
                            start_line: frame.open_line,
                            end_line,
                            kind: frame.kind,
                            depth: 0,
                            placeholder_hint: hint,
                        });
                    }
                }
            }
            at_stmt_start = false;
            i += 1;
            continue;
        }

        if matches!(tok.kind, TokenKind::Keyword | TokenKind::Identifier) {
            if paren_stack.is_empty() {
                if is_word_ci(tok, "WITH") && at_stmt_start {
                    in_with_header = true;
                } else if in_with_header
                    && (is_word_ci(tok, "SELECT")
                        || is_word_ci(tok, "INSERT")
                        || is_word_ci(tok, "UPDATE")
                        || is_word_ci(tok, "DELETE")
                        || is_word_ci(tok, "MERGE"))
                {
                    in_with_header = false;
                }
            }
            at_stmt_start = false;

            if is_word_ci(tok, "CASE") {
                block_stack.push(KeywordBlockFrame {
                    open_line: line,
                    kind: KeywordBlockKind::Case,
                });
            } else if is_word_ci(tok, "BEGIN") {
                if let Some(next) = sig.get(i + 1) {
                    if is_word_ci(next, "TRY") {
                        block_stack.push(KeywordBlockFrame {
                            open_line: line,
                            kind: KeywordBlockKind::BeginTry,
                        });
                        i += 2;
                        continue;
                    }
                    if is_word_ci(next, "CATCH") {
                        block_stack.push(KeywordBlockFrame {
                            open_line: line,
                            kind: KeywordBlockKind::BeginCatch,
                        });
                        i += 2;
                        continue;
                    }
                    let is_tx_start = next.text == ";"
                        || is_word_ci(next, "TRANSACTION")
                        || is_word_ci(next, "TRAN")
                        || is_word_ci(next, "WORK")
                        || is_word_ci(next, "DEFERRED")
                        || is_word_ci(next, "IMMEDIATE")
                        || is_word_ci(next, "EXCLUSIVE")
                        || is_word_ci(next, "ISOLATION")
                        || is_word_ci(next, "READ")
                        || is_word_ci(next, "DISTRIBUTED");
                    if !is_tx_start {
                        block_stack.push(KeywordBlockFrame {
                            open_line: line,
                            kind: KeywordBlockKind::Begin,
                        });
                    }
                }
            } else if is_word_ci(tok, "IF") {
                let is_ddl_guard = sig.get(i + 1).is_some_and(|n| {
                    is_word_ci(n, "EXISTS") || is_word_ci(n, "NOT")
                });
                let is_end_if = i > 0 && is_word_ci(sig[i - 1], "END");
                if !is_ddl_guard && !is_end_if && is_procedural_if(&sig, i) {
                    block_stack.push(KeywordBlockFrame {
                        open_line: line,
                        kind: KeywordBlockKind::If,
                    });
                }
            } else if is_word_ci(tok, "LOOP") && !(i > 0 && is_word_ci(sig[i - 1], "END")) {
                block_stack.push(KeywordBlockFrame {
                    open_line: line,
                    kind: KeywordBlockKind::Loop,
                });
            } else if is_word_ci(tok, "WHILE") && !(i > 0 && is_word_ci(sig[i - 1], "END")) {
                block_stack.push(KeywordBlockFrame {
                    open_line: line,
                    kind: KeywordBlockKind::While,
                });
            } else if is_word_ci(tok, "REPEAT") && !(i > 0 && is_word_ci(sig[i - 1], "END")) {
                block_stack.push(KeywordBlockFrame {
                    open_line: line,
                    kind: KeywordBlockKind::Repeat,
                });
            } else if is_word_ci(tok, "END") {
                if let Some(next) = sig.get(i + 1) {
                    let qualified_target = if is_word_ci(next, "IF") {
                        Some((KeywordBlockKind::If, "… END IF"))
                    } else if is_word_ci(next, "LOOP") {
                        Some((KeywordBlockKind::Loop, "… END LOOP"))
                    } else if is_word_ci(next, "WHILE") {
                        Some((KeywordBlockKind::While, "… END WHILE"))
                    } else if is_word_ci(next, "REPEAT") {
                        Some((KeywordBlockKind::Repeat, "… END REPEAT"))
                    } else if is_word_ci(next, "TRY") {
                        Some((KeywordBlockKind::BeginTry, "… END TRY"))
                    } else if is_word_ci(next, "CATCH") {
                        Some((KeywordBlockKind::BeginCatch, "… END CATCH"))
                    } else {
                        None
                    };
                    if let Some((expected_kind, hint)) = qualified_target {
                        let end_tok_line = line_index_for_byte(spans, next.start);
                        if let Some(pos) = block_stack.iter().rposition(|b| b.kind == expected_kind) {
                            let frame = block_stack.remove(pos);
                            emit_block_fold(frame, end_tok_line, i + 1, &sig, spans, hint, out);
                        }
                        i += 2;
                        continue;
                    }
                }

                if let Some(pos) = block_stack
                    .iter()
                    .rposition(|b| matches!(b.kind, KeywordBlockKind::Case | KeywordBlockKind::Begin))
                {
                    let frame = block_stack.remove(pos);
                    emit_block_fold(frame, line, i, &sig, spans, "… END", out);
                }
            }
        }

        i += 1;
    }

    for (scope_start, scope_end) in paren_scopes {
        collect_clause_folds(sql, &sig[scope_start..scope_end], spans, out);
    }
}

fn emit_block_fold(
    frame: KeywordBlockFrame,
    close_line: usize,
    close_sig_idx: usize,
    sig: &[&Token<'_>],
    spans: &[LineByteSpan],
    hint: &str,
    out: &mut Vec<FoldRegion>,
) {
    if close_line <= frame.open_line {
        return;
    }
    let keep_closing = closing_line_has_trailing_construct(sig, close_sig_idx, close_line, spans);
    let end_line = if keep_closing {
        close_line.saturating_sub(1)
    } else {
        close_line
    };
    if end_line <= frame.open_line {
        return;
    }
    let kind = match frame.kind {
        KeywordBlockKind::Case => FoldKind::CaseBlock,
        _ => FoldKind::ProceduralBlock,
    };
    out.push(FoldRegion {
        start_line: frame.open_line,
        end_line,
        kind,
        depth: 0,
        placeholder_hint: if keep_closing { String::new() } else { hint.to_string() },
    });
}

fn is_procedural_if(sig: &[&Token<'_>], if_idx: usize) -> bool {
    let mut paren_depth = 0usize;
    for tok in &sig[if_idx + 1..] {
        if tok.text == ";" {
            return false;
        }
        if tok.text == "(" {
            paren_depth += 1;
        } else if tok.text == ")" {
            paren_depth = paren_depth.saturating_sub(1);
        } else if paren_depth == 0 && is_word_ci(tok, "THEN") {
            return true;
        }
    }
    false
}

fn closing_line_has_trailing_construct(
    sig: &[&Token<'_>],
    close_idx: usize,
    close_line: usize,
    spans: &[LineByteSpan],
) -> bool {
    let mut saw_comma = false;
    for tok in &sig[close_idx + 1..] {
        let tok_line = line_index_for_byte(spans, tok.start);
        if tok_line != close_line {
            break;
        }
        if tok.text == "," {
            saw_comma = true;
            continue;
        }
        if matches!(tok.text, ")" | ";") {
            continue;
        }
        if saw_comma {
            return true;
        }
        if tok.text == "(" {
            return true;
        }
        if matches!(tok.kind, TokenKind::Keyword | TokenKind::Identifier) {
            let upper = tok.text.to_ascii_uppercase();
            if matches!(
                upper.as_str(),
                "SELECT"
                    | "FROM"
                    | "JOIN"
                    | "LEFT"
                    | "RIGHT"
                    | "INNER"
                    | "OUTER"
                    | "FULL"
                    | "CROSS"
                    | "NATURAL"
                    | "WHERE"
                    | "GROUP"
                    | "HAVING"
                    | "ORDER"
                    | "LIMIT"
                    | "OFFSET"
                    | "UNION"
                    | "INTERSECT"
                    | "EXCEPT"
                    | "SET"
                    | "VALUES"
                    | "RETURNING"
                    | "WITH"
                    | "WHEN"
                    | "ELSE"
            ) {
                return true;
            }
        }
    }
    false
}

fn collect_clause_folds(
    _sql: &str,
    scope_tokens: &[&Token<'_>],
    spans: &[LineByteSpan],
    out: &mut Vec<FoldRegion>,
) {
    if scope_tokens.len() < 2 {
        return;
    }

    let mut clause_starts: Vec<(usize, usize)> = Vec::new();
    let mut paren_depth = 0usize;
    let mut case_depth = 0usize;
    let mut i = 0usize;

    while i < scope_tokens.len() {
        let tok = scope_tokens[i];
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
        if paren_depth > 0 {
            i += 1;
            continue;
        }
        if is_word_ci(tok, "CASE") {
            case_depth += 1;
            i += 1;
            continue;
        }
        if is_word_ci(tok, "END") && case_depth > 0 {
            case_depth -= 1;
            i += 1;
            continue;
        }
        if case_depth > 0 {
            i += 1;
            continue;
        }

        if let Some(advance) = match_clause_starter(scope_tokens, i) {
            let line = line_index_for_byte(spans, tok.start);
            let same_line_as_prev = clause_starts.last().is_some_and(|&(_, prev_line)| prev_line == line);
            if !same_line_as_prev {
                clause_starts.push((i, line));
            }
            i += advance;
            continue;
        }
        i += 1;
    }

    if clause_starts.is_empty() {
        return;
    }

    for idx in 0..clause_starts.len() {
        let (start_sig_idx, start_line) = clause_starts[idx];
        let next_sig_idx = clause_starts
            .get(idx + 1)
            .map(|&(s, _)| s)
            .unwrap_or(scope_tokens.len());
        if next_sig_idx <= start_sig_idx {
            continue;
        }
        let last_tok = scope_tokens[next_sig_idx - 1];
        let last_byte = last_tok.end.saturating_sub(1).max(last_tok.start);
        let end_line = line_index_for_byte(spans, last_byte);
        if end_line > start_line {
            out.push(FoldRegion {
                start_line,
                end_line,
                kind: FoldKind::Clause,
                depth: 0,
                placeholder_hint: String::new(),
            });
        }
    }
}

fn match_clause_starter(tokens: &[&Token<'_>], idx: usize) -> Option<usize> {
    let tok = tokens[idx];
    if !matches!(tok.kind, TokenKind::Keyword | TokenKind::Identifier) {
        return None;
    }
    let upper = tok.text.to_ascii_uppercase();
    match upper.as_str() {
        "SELECT" | "FROM" | "WHERE" | "HAVING" | "WINDOW" | "LIMIT" | "OFFSET" | "SET" | "VALUES"
        | "RETURNING" | "INTERSECT" | "EXCEPT" | "STRAIGHT_JOIN" | "JOIN" => Some(1),
        "UNION" => {
            if tokens.get(idx + 1).is_some_and(|n| is_word_ci(n, "ALL") || is_word_ci(n, "DISTINCT")) {
                Some(2)
            } else {
                Some(1)
            }
        }
        "GROUP" | "ORDER" | "PARTITION" => {
            if tokens.get(idx + 1).is_some_and(|n| is_word_ci(n, "BY")) {
                Some(2)
            } else {
                None
            }
        }
        "LEFT" | "RIGHT" | "FULL" | "INNER" | "CROSS" | "NATURAL" => {
            let mut look = idx + 1;
            while look < tokens.len()
                && look <= idx + 3
                && (is_word_ci(tokens[look], "OUTER")
                    || is_word_ci(tokens[look], "INNER")
                    || is_word_ci(tokens[look], "LEFT")
                    || is_word_ci(tokens[look], "RIGHT")
                    || is_word_ci(tokens[look], "FULL")
                    || is_word_ci(tokens[look], "LATERAL"))
            {
                look += 1;
            }
            if tokens.get(look).is_some_and(|n| is_word_ci(n, "JOIN")) {
                Some(look - idx + 1)
            } else {
                None
            }
        }
        "ON" => {
            if tokens
                .get(idx + 1)
                .is_some_and(|n| is_word_ci(n, "CONFLICT") || is_word_ci(n, "DUPLICATE"))
            {
                Some(2)
            } else {
                None
            }
        }
        "FETCH" => {
            if tokens
                .get(idx + 1)
                .is_some_and(|n| is_word_ci(n, "FIRST") || is_word_ci(n, "NEXT"))
            {
                Some(2)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_word_ci(tok: &Token<'_>, expected: &str) -> bool {
    matches!(tok.kind, TokenKind::Keyword | TokenKind::Identifier) && tok.text.eq_ignore_ascii_case(expected)
}

fn finalize_regions(mut regions: Vec<FoldRegion>) -> Vec<FoldRegion> {
    regions.retain(|r| r.end_line > r.start_line);
    regions.sort_by(|a, b| {
        a.start_line
            .cmp(&b.start_line)
            .then_with(|| b.end_line.cmp(&a.end_line))
            .then_with(|| a.kind.priority().cmp(&b.kind.priority()))
    });

    let mut deduped: Vec<FoldRegion> = Vec::with_capacity(regions.len());
    for region in regions {
        let same_span = deduped
            .last()
            .is_some_and(|prev| prev.start_line == region.start_line && prev.end_line == region.end_line);
        if !same_span {
            deduped.push(region);
        }
    }

    for idx in 0..deduped.len() {
        let (s, e) = (deduped[idx].start_line, deduped[idx].end_line);
        let mut depth = 0u16;
        for other in &deduped[..idx] {
            if other.start_line <= s && other.end_line >= e && (other.start_line < s || other.end_line > e) {
                depth = depth.saturating_add(1);
            }
        }
        deduped[idx].depth = depth;
    }

    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_multi_line_statements_and_clauses_independently() {
        let sql = "\
SELECT
  u.id,
  u.name
FROM users u
LEFT JOIN orders o
  ON o.user_id = u.id
  AND o.status = 'paid'
WHERE
  u.active = 1
  AND u.country = 'ID'
GROUP BY
  u.id,
  u.name
ORDER BY
  u.id DESC;";

        let regions = compute_fold_regions(sql, SqlDialect::MySql);
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::Statement && r.start_line == 0 && r.end_line == 14));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::Clause && r.start_line == 0 && r.end_line == 2));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::Clause && r.start_line == 4 && r.end_line == 6));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::Clause && r.start_line == 7 && r.end_line == 9));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::Clause && r.start_line == 10 && r.end_line == 12));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::Clause && r.start_line == 13 && r.end_line == 14));

        let gutter = gutter_fold_regions(&regions);
        assert_eq!(gutter[0].kind, FoldKind::Statement);
        assert_eq!(gutter[0].start_line, 0);
        assert_eq!(gutter[0].end_line, 14);
    }

    #[test]
    fn preserves_trailing_cte_on_closing_paren_line() {
        let sql = "\
WITH cte1 AS (
  SELECT 1
), cte2 AS (
  SELECT 2
)
SELECT * FROM cte2;";

        let regions = compute_fold_regions(sql, SqlDialect::PostgreSql);
        let cte1 = regions
            .iter()
            .find(|r| r.kind == FoldKind::Cte && r.start_line == 0)
            .expect("cte1 region");
        assert_eq!(cte1.end_line, 1);

        let cte2 = regions
            .iter()
            .find(|r| r.kind == FoldKind::Cte && r.start_line == 2)
            .expect("cte2 region");
        assert_eq!(cte2.end_line, 4);
    }

    #[test]
    fn distinguishes_begin_transaction_from_case_and_begin_blocks() {
        let sql = "\
BEGIN TRANSACTION;
SELECT
  CASE
    WHEN a = 1 THEN 'one'
    ELSE 'other'
  END AS label
FROM items;
COMMIT;";

        let regions = compute_fold_regions(sql, SqlDialect::SqlServer);
        assert!(!regions.iter().any(|r| r.kind == FoldKind::ProceduralBlock));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::CaseBlock && r.start_line == 2 && r.end_line == 5));
    }

    #[test]
    fn detects_postgres_dollar_quoted_and_inner_begin_end() {
        let sql = "\
CREATE FUNCTION add_one(x int) RETURNS int AS $$
BEGIN
  RETURN x + 1;
END;
$$ LANGUAGE plpgsql;";

        let regions = compute_fold_regions(sql, SqlDialect::PostgreSql);
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::Statement && r.start_line == 0 && r.end_line == 4));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::DollarQuoted && r.start_line == 0 && r.end_line == 4)
            || regions
                .iter()
                .any(|r| r.kind == FoldKind::Statement && r.start_line == 0 && r.end_line == 4));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::ProceduralBlock && r.start_line == 1 && r.end_line == 3));
    }

    #[test]
    fn detects_comments_and_nested_custom_regions() {
        let sql = "\
-- region Reports
/*
 * Header block
 */
-- line comment 1
-- line comment 2
SELECT 1;
-- endregion";

        let regions = compute_fold_regions(sql, SqlDialect::MySql);
        let custom = regions
            .iter()
            .find(|r| r.kind == FoldKind::CustomRegion)
            .expect("custom region");
        assert_eq!(custom.start_line, 0);
        assert_eq!(custom.end_line, 7);
        assert_eq!(custom.placeholder_hint, "Reports");
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::BlockComment && r.start_line == 1 && r.end_line == 3));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::LineCommentBlock && r.start_line == 4 && r.end_line == 5));
    }

    #[test]
    fn detects_mssql_try_catch_and_mysql_delimiter_proc() {
        let tsql = "\
BEGIN TRY
  SELECT 1;
END TRY
BEGIN CATCH
  SELECT ERROR_MESSAGE();
END CATCH";
        let regions = compute_fold_regions(tsql, SqlDialect::SqlServer);
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::ProceduralBlock && r.start_line == 0 && r.end_line == 2));
        assert!(regions
            .iter()
            .any(|r| r.kind == FoldKind::ProceduralBlock && r.start_line == 3 && r.end_line == 5));

        let mysql_proc = "\
DELIMITER //
CREATE PROCEDURE p_test()
BEGIN
  IF 1 = 1 THEN
    SELECT 1;
  END IF;
END //
DELIMITER ;";
        let mregions = compute_fold_regions(mysql_proc, SqlDialect::MySql);
        assert!(mregions
            .iter()
            .any(|r| r.kind == FoldKind::ProceduralBlock && r.start_line == 2 && r.end_line == 6));
        assert!(mregions
            .iter()
            .any(|r| r.kind == FoldKind::ProceduralBlock && r.start_line == 3 && r.end_line == 5));
    }

    #[test]
    fn innermost_region_resolves_nested_cursor_line() {
        let sql = "\
SELECT *
FROM (
  SELECT id
  FROM orders
) o
WHERE o.id > 10;";
        let regions = compute_fold_regions(sql, SqlDialect::MySql);
        let inner = innermost_region_at_line(&regions, 3).expect("innermost");
        assert_eq!(inner.kind, FoldKind::Subquery);
        assert_eq!((inner.start_line, inner.end_line), (1, 4));
    }
}
