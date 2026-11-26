#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Identifier,
    QuotedIdentifier,
    StringLiteral,
    Number,
    Comment,
    Operator,
    Punctuation,
    Whitespace,
    SystemVar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind,
    pub text: &'a str,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlStatement<'a> {
    pub sql: &'a str,
    pub start: usize,
    pub end: usize,
}

pub const MYSQL_KEYWORDS: &[&str] = &[
    "ACCESSIBLE", "ADD", "ALL", "ALTER", "ANALYZE", "AND", "AS", "ASC", "AUTO_INCREMENT", "BEFORE",
    "BEGIN", "BETWEEN", "BIGINT", "BIGSERIAL", "BINARY", "BIT", "BLOB", "BOOL", "BOOLEAN", "BY",
    "CALL", "CASCADE", "CASE", "CHANGE", "CHAR", "CHARACTER", "CHECK", "COLLATE", "COLUMN",
    "COMMIT", "CONFLICT", "CONSTRAINT", "CREATE", "CROSS", "CURRENT_DATE", "CURRENT_TIME",
    "CURRENT_TIMESTAMP", "CURRENT_USER", "DATABASE", "DATABASES", "DATE", "DATETIME", "DECIMAL",
    "DECLARE", "DEFAULT", "DEFINER", "DELETE", "DESC", "DESCRIBE", "DISTINCT", "DIV", "DO",
    "DOUBLE", "DROP", "DUPLICATE", "ELSE", "ELSEIF", "END", "ENGINE", "ENUM", "EVENT", "EXISTS",
    "EXPLAIN", "EXTENSION", "FALSE", "FLOAT", "FOR", "FORCE", "FOREIGN", "FORMAT", "FROM", "FULL",
    "FULLTEXT", "FUNCTION", "GENERATED", "GRANT", "GROUP", "HAVING", "IF", "IGNORE", "ILIKE", "IN",
    "INDEX", "INFILE", "INNER", "INSERT", "INT", "INTEGER", "INTERVAL", "INTO", "IS", "JOIN",
    "JSON", "JSONB", "KEY", "KEYS", "KILL", "LATERAL", "LEFT", "LIKE", "LIMIT", "LOAD", "LOCK",
    "LONG", "LONGBLOB", "LONGTEXT", "LOW_PRIORITY", "MATCH", "MATERIALIZED", "MEDIUMBLOB",
    "MEDIUMINT", "MEDIUMTEXT", "MOD", "MODIFY", "NATURAL", "NOT", "NOTHING", "NULL", "NUMERIC",
    "OFFSET", "ON", "OPTIMIZE", "OPTION", "OR", "ORDER", "OUTER", "OVER", "PARTITION", "PRIMARY",
    "PRIVILEGES", "PROCEDURE", "PROCESSLIST", "PURGE", "QUERY", "RANGE", "READ", "REAL",
    "RECURSIVE", "REFERENCES", "REGEXP", "RELEASE", "RENAME", "REPEAT", "REPLACE", "REQUIRE",
    "RESTRICT", "RETURN", "RETURNING", "RETURNS", "REVOKE", "RIGHT", "RLIKE", "ROLLBACK", "ROW",
    "ROWS", "SAVEPOINT", "SCHEMA", "SCHEMAS", "SELECT", "SEPARATOR", "SEQUENCE", "SERIAL",
    "SESSION", "SET", "SHOW", "SMALLINT", "SPATIAL", "START", "STATUS", "STORED", "STRAIGHT_JOIN",
    "TABLE", "TABLES", "TEMPORARY", "TERMINATED", "TEXT", "THEN", "TIME", "TIMESTAMP", "TINYBLOB",
    "TINYINT", "TINYTEXT", "TO", "TRANSACTION", "TRIGGER", "TRIGGERS", "TRUE", "TRUNCATE", "UNION",
    "UNIQUE", "UNLOCK", "UNSIGNED", "UPDATE", "USAGE", "USE", "USER", "USING", "UTC_DATE",
    "UTC_TIME", "UTC_TIMESTAMP", "UUID", "VACUUM", "VALUES", "VARBINARY", "VARCHAR", "VARIABLES",
    "VIEW", "VIRTUAL", "WARNINGS", "WHEN", "WHERE", "WHILE", "WINDOW", "WITH", "WRITE", "XOR",
    "YEAR", "ZEROFILL",
];

pub fn dollar_quote_tag_len(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'$') {
        return None;
    }
    let is_preceded_by_word = start > 0 && {
        let previous = bytes[start - 1];
        previous.is_ascii_alphanumeric() || previous == b'_' || previous == b'$'
    };
    if is_preceded_by_word {
        return None;
    }
    let mut cursor = start + 1;
    if let Some(first) = bytes.get(cursor) {
        if first.is_ascii_digit() {
            return None;
        }
    }
    while cursor < bytes.len() && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_') {
        cursor += 1;
    }
    (bytes.get(cursor) == Some(&b'$')).then_some(cursor + 1 - start)
}

fn dollar_quoted_end(input: &str, start: usize, tag_len: usize) -> usize {
    let tag = &input[start..start + tag_len];
    let body_start = start + tag_len;
    input[body_start..]
        .find(tag)
        .map(|offset| body_start + offset + tag_len)
        .unwrap_or(input.len())
}

fn starts_hash_comment(bytes: &[u8], position: usize) -> bool {
    bytes.get(position) == Some(&b'#') && bytes.get(position + 1) != Some(&b'>')
}

pub fn is_keyword(word: &str) -> bool {
    MYSQL_KEYWORDS
        .binary_search_by(|kw| kw.cmp(&word.to_ascii_uppercase().as_str()))
        .is_ok()
}

pub fn tokenize(input: &str) -> Vec<Token<'_>> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut out = Vec::with_capacity(input.len() / 5 + 8);

    while i < len {
        let start = i;
        let b = bytes[i];

        if b.is_ascii_whitespace() {
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            out.push(Token { kind: TokenKind::Whitespace, text: &input[start..i], start, end: i });
            continue;
        }

        if starts_hash_comment(bytes, i) {
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            out.push(Token { kind: TokenKind::Comment, text: &input[start..i], start, end: i });
            continue;
        }

        if b == b'-' && i + 1 < len && bytes[i + 1] == b'-' {
            let next_is_ws = i + 2 == len || bytes[i + 2].is_ascii_whitespace() || bytes[i + 2].is_ascii_control();
            if next_is_ws {
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                out.push(Token { kind: TokenKind::Comment, text: &input[start..i], start, end: i });
                continue;
            }
        }

        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(len);
            out.push(Token { kind: TokenKind::Comment, text: &input[start..i], start, end: i });
            continue;
        }

        if b == b'\'' || b == b'"' {
            let quote = b;
            i += 1;
            while i < len {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2;
                } else if bytes[i] == quote {
                    if i + 1 < len && bytes[i + 1] == quote {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            out.push(Token { kind: TokenKind::StringLiteral, text: &input[start..i], start, end: i });
            continue;
        }

        if b == b'`' {
            i += 1;
            while i < len {
                if bytes[i] == b'`' {
                    if i + 1 < len && bytes[i + 1] == b'`' {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            out.push(Token { kind: TokenKind::QuotedIdentifier, text: &input[start..i], start, end: i });
            continue;
        }

        if b == b'@' && i + 1 < len && (bytes[i + 1] == b'@' || bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_') {
            i += 1;
            if i < len && bytes[i] == b'@' {
                i += 1;
            }
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.') {
                i += 1;
            }
            out.push(Token { kind: TokenKind::SystemVar, text: &input[start..i], start, end: i });
            continue;
        }

        if let Some(tag_len) = dollar_quote_tag_len(bytes, i) {
            i = dollar_quoted_end(input, i, tag_len);
            out.push(Token { kind: TokenKind::StringLiteral, text: &input[start..i], start, end: i });
            continue;
        }

        if b.is_ascii_digit() {
            i += 1;
            if b == b'0' && i < len && (bytes[i] == b'x' || bytes[i] == b'X') {
                i += 1;
                while i < len && bytes[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < len && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
            }
            out.push(Token { kind: TokenKind::Number, text: &input[start..i], start, end: i });
            continue;
        }

        if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
            i += 1;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$') {
                i += 1;
            }
            let text = &input[start..i];
            let kind = if is_keyword(text) {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            out.push(Token { kind, text, start, end: i });
            continue;
        }

        if matches!(b, b'(' | b')' | b',' | b';' | b'.') {
            i += 1;
            out.push(Token { kind: TokenKind::Punctuation, text: &input[start..i], start, end: i });
            continue;
        }

        let ch_len = input[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        i += ch_len;
        out.push(Token { kind: TokenKind::Operator, text: &input[start..i], start, end: i });
    }

    out
}

pub fn split_statements(input: &str) -> Vec<SqlStatement<'_>> {
    let mut statements = Vec::new();
    let mut delimiter = ";".to_string();
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut stmt_start = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut at_line_start = true;

    while i < len {
        if !in_single && !in_double && !in_backtick && !in_line_comment && !in_block_comment && at_line_start {
            let line_end = input[i..].find('\n').map(|o| i + o).unwrap_or(len);
            let trimmed = input[i..line_end].trim();
            if trimmed.len() > 10 && trimmed[..9].eq_ignore_ascii_case("DELIMITER") && trimmed.as_bytes()[9].is_ascii_whitespace() {
                let new_delim = trimmed[9..].trim();
                if !new_delim.is_empty() {
                    let prefix = input[stmt_start..i].trim();
                    if !prefix.is_empty() {
                        statements.push(SqlStatement { sql: prefix, start: stmt_start, end: i });
                    }
                    delimiter = new_delim.to_string();
                    i = (line_end + 1).min(len);
                    stmt_start = i;
                    at_line_start = true;
                    continue;
                }
            }
        }

        let b = bytes[i];
        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
                at_line_start = true;
            }
            i += 1;
            continue;
        }

        if in_block_comment {
            if b == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
            } else {
                if b == b'\n' {
                    at_line_start = true;
                }
                i += 1;
            }
            continue;
        }

        if in_single {
            if b == b'\\' && i + 1 < len {
                i += 2;
                continue;
            }
            if b == b'\'' {
                if i + 1 < len && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_single = false;
            }
            i += 1;
            continue;
        }

        if in_double {
            if b == b'\\' && i + 1 < len {
                i += 2;
                continue;
            }
            if b == b'"' {
                if i + 1 < len && bytes[i + 1] == b'"' {
                    i += 2;
                    continue;
                }
                in_double = false;
            }
            i += 1;
            continue;
        }

        if in_backtick {
            if b == b'`' {
                if i + 1 < len && bytes[i + 1] == b'`' {
                    i += 2;
                    continue;
                }
                in_backtick = false;
            }
            i += 1;
            continue;
        }

        if starts_hash_comment(bytes, i) {
            in_line_comment = true;
            at_line_start = false;
            i += 1;
            continue;
        }
        if let Some(tag_len) = dollar_quote_tag_len(bytes, i) {
            i = dollar_quoted_end(input, i, tag_len);
            at_line_start = false;
            continue;
        }
        if b == b'-' && i + 1 < len && bytes[i + 1] == b'-' && (i + 2 == len || bytes[i + 2].is_ascii_whitespace()) {
            in_line_comment = true;
            at_line_start = false;
            i += 2;
            continue;
        }
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            in_block_comment = true;
            at_line_start = false;
            i += 2;
            continue;
        }
        if b == b'\'' {
            in_single = true;
            at_line_start = false;
            i += 1;
            continue;
        }
        if b == b'"' {
            in_double = true;
            at_line_start = false;
            i += 1;
            continue;
        }
        if b == b'`' {
            in_backtick = true;
            at_line_start = false;
            i += 1;
            continue;
        }

        if input[i..].starts_with(&delimiter) {
            let segment = input[stmt_start..i].trim();
            if !segment.is_empty() {
                statements.push(SqlStatement { sql: segment, start: stmt_start, end: i });
            }
            i += delimiter.len();
            stmt_start = i;
            at_line_start = false;
            continue;
        }

        if b == b'\n' {
            at_line_start = true;
        } else if !b.is_ascii_whitespace() {
            at_line_start = false;
        }
        i += 1;
    }

    let tail = input[stmt_start..].trim();
    if !tail.is_empty() {
        statements.push(SqlStatement { sql: tail, start: stmt_start, end: len });
    }

    statements
}

pub fn statement_at_offset(input: &str, byte_offset: usize) -> Option<SqlStatement<'_>> {
    let stmts = split_statements(input);
    if stmts.is_empty() {
        return None;
    }
    let clamped = byte_offset.min(input.len());
    stmts
        .iter()
        .find(|s| clamped >= s.start && clamped <= s.end)
        .or_else(|| stmts.iter().rev().find(|s| s.start <= clamped))
        .or_else(|| stmts.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_delimiter_procedures_and_semicolons() {
        let script = r#"
            SELECT 'a;b' AS literal;
            DELIMITER //
            CREATE PROCEDURE p_test()
            BEGIN
                SELECT 1;
                SELECT 2;
            END //
            DELIMITER ;
            SELECT @@version;
        "#;
        let stmts = split_statements(script);
        assert_eq!(stmts.len(), 3);
        assert!(stmts[0].sql.starts_with("SELECT 'a;b'"));
        assert!(stmts[1].sql.contains("CREATE PROCEDURE p_test()"));
        assert!(stmts[1].sql.contains("SELECT 2;"));
        assert_eq!(stmts[2].sql, "SELECT @@version");
    }

    #[test]
    fn keeps_postgres_dollar_quoted_function_bodies_in_one_statement() {
        let script = "CREATE FUNCTION add_one(x int) RETURNS int AS $$\nBEGIN\n  RETURN x + 1;\nEND;\n$$ LANGUAGE plpgsql;\nSELECT add_one(1);\nDO $body$ BEGIN PERFORM 1; END $body$;";
        let stmts = split_statements(script);
        assert_eq!(stmts.len(), 3);
        assert!(stmts[0].sql.ends_with("LANGUAGE plpgsql"));
        assert!(stmts[0].sql.contains("RETURN x + 1;"));
        assert_eq!(stmts[1].sql, "SELECT add_one(1)");
        assert!(stmts[2].sql.starts_with("DO $body$"));
    }

    #[test]
    fn positional_parameters_and_identifiers_with_dollar_are_not_dollar_quotes() {
        let stmts = split_statements("SELECT $1, price$usd FROM t WHERE id = $2; SELECT 2");
        assert_eq!(stmts.len(), 2);
        assert_eq!(dollar_quote_tag_len(b"$1", 0), None);
        assert_eq!(dollar_quote_tag_len(b"a$b$", 1), None);
        assert_eq!(dollar_quote_tag_len(b"$tag$", 0), Some(5));
    }

    #[test]
    fn json_path_operators_are_not_mysql_hash_comments() {
        let stmts = split_statements("SELECT data #>> '{a,b}' FROM t; SELECT 2; # trailing comment ; still comment");
        assert_eq!(stmts.len(), 3);
        assert_eq!(stmts[0].sql, "SELECT data #>> '{a,b}' FROM t");
        assert_eq!(stmts[1].sql, "SELECT 2");
        let tokens = tokenize("SELECT data #> '{a}'");
        assert!(tokens.iter().all(|token| token.kind != TokenKind::Comment));
        let dollar_tokens = tokenize("SELECT $$it's; fine$$");
        assert!(dollar_tokens
            .iter()
            .any(|token| token.kind == TokenKind::StringLiteral && token.text == "$$it's; fine$$"));
    }

    #[test]
    fn keyword_list_stays_sorted_for_binary_search() {
        assert!(MYSQL_KEYWORDS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(is_keyword("returning"));
        assert!(is_keyword("ilike"));
    }

    #[test]
    fn resolves_statement_at_cursor_offset() {
        let sql = "SELECT 1;\nSELECT 2;\nSELECT 3;";
        let pos = sql.find("SELECT 2").unwrap() + 4;
        let current = statement_at_offset(sql, pos).unwrap();
        assert_eq!(current.sql, "SELECT 2");
    }
}
