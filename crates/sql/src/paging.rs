use crate::lexer::{tokenize, Token, TokenKind};
use cottondata_core::{SqlDialect, UNLIMITED_ROWS};

pub fn strip_trailing_terminator(sql: &str) -> &str {
    sql.trim().trim_end_matches(|c: char| c == ';' || c.is_whitespace())
}

fn significant_tokens(sql: &str) -> Vec<Token<'_>> {
    tokenize(sql)
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect()
}

fn token_eq_ci(token: &Token<'_>, expected: &str) -> bool {
    matches!(token.kind, TokenKind::Keyword | TokenKind::Identifier)
        && token.text.eq_ignore_ascii_case(expected)
}

pub fn is_pageable_read_query(sql: &str) -> bool {
    let tokens = significant_tokens(sql);
    let Some(first) = tokens.first() else {
        return false;
    };
    let starts_read = first.text == "("
        || token_eq_ci(first, "SELECT")
        || token_eq_ci(first, "WITH")
        || token_eq_ci(first, "VALUES")
        || token_eq_ci(first, "TABLE");
    if !starts_read {
        return false;
    }

    let has_side_effect = tokens.iter().any(|token| {
        token_eq_ci(token, "INSERT")
            || token_eq_ci(token, "UPDATE")
            || token_eq_ci(token, "DELETE")
            || token_eq_ci(token, "MERGE")
            || token_eq_ci(token, "INTO")
    });
    if has_side_effect {
        return false;
    }

    let semicolon_count = tokens
        .iter()
        .filter(|token| token.kind == TokenKind::Punctuation && token.text == ";")
        .count();
    semicolon_count == 0
        || (semicolon_count == 1 && tokens.last().is_some_and(|token| token.text == ";"))
}

pub fn has_explicit_row_limit(sql: &str) -> bool {
    let tokens = significant_tokens(sql);
    let mut depth: usize = 0;

    for (idx, token) in tokens.iter().enumerate() {
        if token.kind == TokenKind::Punctuation {
            if token.text == "(" {
                depth += 1;
                continue;
            }
            if token.text == ")" {
                depth = depth.saturating_sub(1);
                continue;
            }
        }
        if depth > 0 {
            continue;
        }
        if token_eq_ci(token, "LIMIT") {
            return true;
        }
        if token_eq_ci(token, "FETCH")
            && tokens
                .get(idx + 1)
                .is_some_and(|next| token_eq_ci(next, "FIRST") || token_eq_ci(next, "NEXT"))
        {
            return true;
        }
        if token_eq_ci(token, "SELECT") {
            let mut look = idx + 1;
            if tokens
                .get(look)
                .is_some_and(|next| token_eq_ci(next, "DISTINCT") || token_eq_ci(next, "ALL"))
            {
                look += 1;
            }
            if tokens.get(look).is_some_and(|next| token_eq_ci(next, "TOP")) {
                return true;
            }
        }
    }
    false
}

fn has_top_level_locking_or_tail_clause(sql: &str, dialect: SqlDialect) -> bool {
    let tokens = significant_tokens(sql);
    let mut depth: usize = 0;

    for (idx, token) in tokens.iter().enumerate() {
        if token.kind == TokenKind::Punctuation {
            if token.text == "(" {
                depth += 1;
                continue;
            }
            if token.text == ")" {
                depth = depth.saturating_sub(1);
                continue;
            }
        }
        if depth > 0 {
            continue;
        }
        if token_eq_ci(token, "FOR")
            && tokens.get(idx + 1).is_some_and(|next| {
                token_eq_ci(next, "UPDATE")
                    || token_eq_ci(next, "SHARE")
                    || token_eq_ci(next, "NO")
                    || token_eq_ci(next, "KEY")
                    || (dialect == SqlDialect::SqlServer
                        && (token_eq_ci(next, "JSON")
                            || token_eq_ci(next, "XML")
                            || token_eq_ci(next, "BROWSE")))
            })
        {
            return true;
        }
        if token_eq_ci(token, "LOCK")
            && tokens.get(idx + 1).is_some_and(|next| token_eq_ci(next, "IN"))
        {
            return true;
        }
        if dialect == SqlDialect::SqlServer && token_eq_ci(token, "OPTION") {
            return true;
        }
    }
    false
}

fn has_top_level_order_by(sql: &str) -> bool {
    let tokens = significant_tokens(sql);
    let mut depth: usize = 0;
    for (idx, token) in tokens.iter().enumerate() {
        if token.kind == TokenKind::Punctuation {
            if token.text == "(" {
                depth += 1;
                continue;
            }
            if token.text == ")" {
                depth = depth.saturating_sub(1);
                continue;
            }
        }
        if depth == 0
            && token_eq_ci(token, "ORDER")
            && tokens.get(idx + 1).is_some_and(|next| token_eq_ci(next, "BY"))
        {
            return true;
        }
    }
    false
}

fn is_top_level_distinct_select(sql: &str) -> bool {
    let tokens = significant_tokens(sql);
    let mut depth: usize = 0;
    for (idx, token) in tokens.iter().enumerate() {
        if token.kind == TokenKind::Punctuation {
            if token.text == "(" {
                depth += 1;
                continue;
            }
            if token.text == ")" {
                depth = depth.saturating_sub(1);
                continue;
            }
        }
        if depth == 0
            && token_eq_ci(token, "SELECT")
            && tokens.get(idx + 1).is_some_and(|next| token_eq_ci(next, "DISTINCT"))
        {
            return true;
        }
    }
    false
}

pub fn can_auto_paginate(dialect: SqlDialect, sql: &str) -> bool {
    is_pageable_read_query(sql)
        && !has_explicit_row_limit(sql)
        && !has_top_level_locking_or_tail_clause(sql, dialect)
}

pub fn build_paginated_query(dialect: SqlDialect, sql: &str, limit: u32, offset: u64) -> Option<String> {
    if !can_auto_paginate(dialect, sql) {
        return None;
    }
    let body = strip_trailing_terminator(sql);
    if body.is_empty() {
        return None;
    }
    let capped_limit = limit.max(1);
    match dialect {
        SqlDialect::MySql | SqlDialect::PostgreSql | SqlDialect::CockroachDb | SqlDialect::Sqlite => {
            if offset == 0 {
                Some(format!("{body}\nLIMIT {capped_limit}"))
            } else {
                Some(format!("{body}\nLIMIT {capped_limit} OFFSET {offset}"))
            }
        }
        SqlDialect::SqlServer => {
            if has_top_level_order_by(body) {
                Some(format!(
                    "{body}\nOFFSET {offset} ROWS FETCH NEXT {capped_limit} ROWS ONLY"
                ))
            } else if is_top_level_distinct_select(body) {
                Some(format!(
                    "SELECT * FROM (\n{body}\n) AS _cd_page ORDER BY (SELECT NULL) OFFSET {offset} ROWS FETCH NEXT {capped_limit} ROWS ONLY"
                ))
            } else {
                Some(format!(
                    "{body}\nORDER BY (SELECT NULL) OFFSET {offset} ROWS FETCH NEXT {capped_limit} ROWS ONLY"
                ))
            }
        }
    }
}

fn starts_with_cte(sql: &str) -> bool {
    significant_tokens(sql)
        .first()
        .is_some_and(|token| token_eq_ci(token, "WITH"))
}

fn strip_top_level_order_by(body: &str) -> &str {
    if has_explicit_row_limit(body) {
        return body;
    }
    let tokens = significant_tokens(body);
    let mut depth: usize = 0;
    let mut last_order_idx: Option<usize> = None;
    for (idx, token) in tokens.iter().enumerate() {
        if token.kind == TokenKind::Punctuation {
            if token.text == "(" {
                depth += 1;
                continue;
            }
            if token.text == ")" {
                depth = depth.saturating_sub(1);
                continue;
            }
        }
        if depth == 0
            && token_eq_ci(token, "ORDER")
            && tokens.get(idx + 1).is_some_and(|next| token_eq_ci(next, "BY"))
        {
            last_order_idx = Some(idx);
        }
    }
    let Some(order_idx) = last_order_idx else {
        return body;
    };
    let mut tail_depth: usize = 0;
    for token in &tokens[order_idx + 2..] {
        if token.kind == TokenKind::Punctuation {
            if token.text == "(" {
                tail_depth += 1;
                continue;
            }
            if token.text == ")" {
                tail_depth = tail_depth.saturating_sub(1);
                continue;
            }
        }
        if tail_depth == 0 && token_eq_ci(token, "OFFSET") {
            return body;
        }
    }
    body[..tokens[order_idx].start].trim_end()
}

pub fn build_sorted_query(
    dialect: SqlDialect,
    base_sql: &str,
    col_name: &str,
    descending: bool,
    limit: u32,
    offset: u64,
) -> Option<String> {
    if col_name.trim().is_empty() {
        return None;
    }
    let body = strip_trailing_terminator(base_sql);
    if body.is_empty()
        || !is_pageable_read_query(body)
        || has_top_level_locking_or_tail_clause(body, dialect)
    {
        return None;
    }
    if dialect == SqlDialect::SqlServer && starts_with_cte(body) {
        return None;
    }

    let inner = if dialect == SqlDialect::SqlServer {
        strip_top_level_order_by(body)
    } else {
        body
    };
    if inner.is_empty() {
        return None;
    }

    let quoted_col = dialect.quote_ident(col_name);
    let dir_sql = if descending { "DESC" } else { "ASC" };

    match dialect {
        SqlDialect::MySql | SqlDialect::PostgreSql | SqlDialect::CockroachDb | SqlDialect::Sqlite => {
            if limit == UNLIMITED_ROWS {
                if offset == 0 {
                    Some(format!(
                        "SELECT * FROM (\n{inner}\n) AS _cd_sort ORDER BY {quoted_col} {dir_sql}"
                    ))
                } else {
                    let tail = match dialect {
                        SqlDialect::MySql => format!("LIMIT 18446744073709551615 OFFSET {offset}"),
                        SqlDialect::Sqlite => format!("LIMIT -1 OFFSET {offset}"),
                        SqlDialect::PostgreSql | SqlDialect::CockroachDb => format!("OFFSET {offset}"),
                        SqlDialect::SqlServer => unreachable!(),
                    };
                    Some(format!(
                        "SELECT * FROM (\n{inner}\n) AS _cd_sort ORDER BY {quoted_col} {dir_sql} {tail}"
                    ))
                }
            } else {
                let capped_limit = limit.max(1);
                Some(format!(
                    "SELECT * FROM (\n{inner}\n) AS _cd_sort ORDER BY {quoted_col} {dir_sql} LIMIT {capped_limit} OFFSET {offset}"
                ))
            }
        }
        SqlDialect::SqlServer => {
            if limit == UNLIMITED_ROWS {
                if offset == 0 {
                    Some(format!(
                        "SELECT * FROM (\n{inner}\n) AS _cd_sort ORDER BY {quoted_col} {dir_sql}"
                    ))
                } else {
                    Some(format!(
                        "SELECT * FROM (\n{inner}\n) AS _cd_sort ORDER BY {quoted_col} {dir_sql} OFFSET {offset} ROWS"
                    ))
                }
            } else {
                let capped_limit = limit.max(1);
                Some(format!(
                    "SELECT * FROM (\n{inner}\n) AS _cd_sort ORDER BY {quoted_col} {dir_sql} OFFSET {offset} ROWS FETCH NEXT {capped_limit} ROWS ONLY"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_pageable_reads_and_rejects_writes_or_multi_statement() {
        assert!(is_pageable_read_query("SELECT * FROM users"));
        assert!(is_pageable_read_query("  WITH active AS (SELECT id FROM users) SELECT * FROM active; "));
        assert!(is_pageable_read_query("(SELECT 1) UNION ALL (SELECT 2)"));
        assert!(is_pageable_read_query("TABLE users;"));
        assert!(!is_pageable_read_query("SHOW TABLES"));
        assert!(!is_pageable_read_query("EXPLAIN SELECT * FROM users"));
        assert!(!is_pageable_read_query("SELECT * INTO backup FROM users"));
        assert!(!is_pageable_read_query("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d"));
        assert!(!is_pageable_read_query("SELECT 1; SELECT 2"));
    }

    #[test]
    fn detects_top_level_limits_but_ignores_subqueries_and_literals() {
        assert!(!has_explicit_row_limit("SELECT * FROM users"));
        assert!(!has_explicit_row_limit(
            "SELECT * FROM users WHERE id IN (SELECT user_id FROM logs LIMIT 10)"
        ));
        assert!(!has_explicit_row_limit(
            "SELECT 'LIMIT 10' AS label FROM users -- LIMIT 5"
        ));
        assert!(has_explicit_row_limit("SELECT * FROM users LIMIT 20"));
        assert!(has_explicit_row_limit("SELECT * FROM users FETCH FIRST 10 ROWS ONLY"));
        assert!(has_explicit_row_limit("SELECT * FROM users ORDER BY id OFFSET 0 ROWS FETCH NEXT 50 ROWS ONLY"));
        assert!(has_explicit_row_limit("SELECT TOP 25 * FROM users"));
        assert!(has_explicit_row_limit("SELECT DISTINCT TOP (10) id FROM users"));
    }

    #[test]
    fn builds_paginated_continuations_safely_across_dialects() {
        assert_eq!(
            build_paginated_query(SqlDialect::MySql, "SELECT * FROM users -- trailing comment", 101, 100),
            Some("SELECT * FROM users -- trailing comment\nLIMIT 101 OFFSET 100".into())
        );
        assert_eq!(
            build_paginated_query(SqlDialect::CockroachDb, "SELECT * FROM users;", 101, 0),
            Some("SELECT * FROM users\nLIMIT 101".into())
        );
        assert_eq!(
            build_paginated_query(SqlDialect::SqlServer, "SELECT * FROM users ORDER BY id DESC;", 101, 100),
            Some("SELECT * FROM users ORDER BY id DESC\nOFFSET 100 ROWS FETCH NEXT 101 ROWS ONLY".into())
        );
        assert_eq!(
            build_paginated_query(SqlDialect::SqlServer, "SELECT * FROM users;", 101, 100),
            Some("SELECT * FROM users\nORDER BY (SELECT NULL) OFFSET 100 ROWS FETCH NEXT 101 ROWS ONLY".into())
        );
        assert_eq!(
            build_paginated_query(SqlDialect::SqlServer, "SELECT DISTINCT email FROM users;", 101, 100),
            Some(
                "SELECT * FROM (\nSELECT DISTINCT email FROM users\n) AS _cd_page ORDER BY (SELECT NULL) OFFSET 100 ROWS FETCH NEXT 101 ROWS ONLY"
                    .into()
            )
        );
        assert_eq!(
            build_paginated_query(SqlDialect::MySql, "SELECT * FROM users LIMIT 50", 101, 100),
            None
        );
        assert_eq!(
            build_paginated_query(SqlDialect::MySql, "SELECT * FROM users FOR UPDATE", 101, 100),
            None
        );
        assert_eq!(
            build_paginated_query(SqlDialect::SqlServer, "SELECT * FROM users FOR JSON AUTO", 101, 100),
            None
        );
    }

    #[test]
    fn builds_sorted_queries_across_all_dialects_and_handles_edge_cases() {
        assert_eq!(
            build_sorted_query(
                SqlDialect::MySql,
                "SELECT * FROM users WHERE active = 1 ORDER BY created_at -- keep comment;",
                "tot`al",
                true,
                101,
                0,
            ),
            Some(
                "SELECT * FROM (\nSELECT * FROM users WHERE active = 1 ORDER BY created_at -- keep comment\n) AS _cd_sort ORDER BY `tot``al` DESC LIMIT 101 OFFSET 0"
                    .into()
            )
        );
        assert_eq!(
            build_sorted_query(
                SqlDialect::PostgreSql,
                "WITH active AS (SELECT id, \"name\" FROM users) SELECT * FROM active;",
                "na\"me",
                false,
                50,
                100,
            ),
            Some(
                "SELECT * FROM (\nWITH active AS (SELECT id, \"name\" FROM users) SELECT * FROM active\n) AS _cd_sort ORDER BY \"na\"\"me\" ASC LIMIT 50 OFFSET 100"
                    .into()
            )
        );
        assert_eq!(
            build_sorted_query(
                SqlDialect::CockroachDb,
                "SELECT id FROM orders",
                "id",
                true,
                UNLIMITED_ROWS,
                0,
            ),
            Some("SELECT * FROM (\nSELECT id FROM orders\n) AS _cd_sort ORDER BY \"id\" DESC".into())
        );
        assert_eq!(
            build_sorted_query(
                SqlDialect::Sqlite,
                "SELECT id, score FROM scores;",
                "score",
                true,
                25,
                50,
            ),
            Some(
                "SELECT * FROM (\nSELECT id, score FROM scores\n) AS _cd_sort ORDER BY \"score\" DESC LIMIT 25 OFFSET 50"
                    .into()
            )
        );
        assert_eq!(
            build_sorted_query(
                SqlDialect::SqlServer,
                "SELECT id, total FROM orders WHERE status = 'paid' ORDER BY created_at DESC;",
                "to]tal",
                true,
                101,
                200,
            ),
            Some(
                "SELECT * FROM (\nSELECT id, total FROM orders WHERE status = 'paid'\n) AS _cd_sort ORDER BY [to]]tal] DESC OFFSET 200 ROWS FETCH NEXT 101 ROWS ONLY"
                    .into()
            )
        );
        assert_eq!(
            build_sorted_query(
                SqlDialect::SqlServer,
                "SELECT TOP 50 id, total FROM orders ORDER BY created_at DESC",
                "total",
                false,
                101,
                0,
            ),
            Some(
                "SELECT * FROM (\nSELECT TOP 50 id, total FROM orders ORDER BY created_at DESC\n) AS _cd_sort ORDER BY [total] ASC OFFSET 0 ROWS FETCH NEXT 101 ROWS ONLY"
                    .into()
            )
        );
        assert_eq!(
            build_sorted_query(
                SqlDialect::SqlServer,
                "SELECT id, ROW_NUMBER() OVER (ORDER BY id) AS rn FROM orders",
                "rn",
                true,
                UNLIMITED_ROWS,
                0,
            ),
            Some(
                "SELECT * FROM (\nSELECT id, ROW_NUMBER() OVER (ORDER BY id) AS rn FROM orders\n) AS _cd_sort ORDER BY [rn] DESC"
                    .into()
            )
        );
        assert_eq!(
            build_sorted_query(
                SqlDialect::SqlServer,
                "WITH cte AS (SELECT 1 AS a) SELECT * FROM cte",
                "a",
                true,
                100,
                0,
            ),
            None
        );
        assert_eq!(
            build_sorted_query(SqlDialect::MySql, "SELECT * FROM users", "   ", true, 100, 0),
            None
        );
        assert_eq!(
            build_sorted_query(SqlDialect::PostgreSql, "DELETE FROM users", "id", true, 100, 0),
            None
        );
        assert_eq!(
            build_sorted_query(SqlDialect::MySql, "SHOW TABLES", "Tables", false, 100, 0),
            None
        );
        assert_eq!(
            build_sorted_query(SqlDialect::MySql, "SELECT * FROM users FOR UPDATE", "id", true, 100, 0),
            None
        );
    }
}
