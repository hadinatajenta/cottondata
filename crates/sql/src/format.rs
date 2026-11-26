use crate::lexer::{tokenize, TokenKind};

const NEWLINE_CLAUSES: &[&str] = &[
    "SELECT", "FROM", "WHERE", "LEFT", "RIGHT", "INNER", "OUTER", "CROSS", "JOIN", "GROUP",
    "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION", "SET", "VALUES", "ON", "AND", "OR",
];

pub fn format_mysql(sql: &str) -> String {
    let tokens = tokenize(sql);
    let mut out = String::with_capacity(sql.len() + 32);
    let mut prev_was_space = true;
    let mut prev_upper = String::new();
    let mut paren_depth: usize = 0;

    for tok in tokens {
        match tok.kind {
            TokenKind::Whitespace => {
                if !prev_was_space && !out.ends_with('\n') {
                    out.push(' ');
                    prev_was_space = true;
                }
            }
            TokenKind::Keyword => {
                let upper = tok.text.to_ascii_uppercase();
                let should_break = paren_depth == 0
                    && !out.is_empty()
                    && NEWLINE_CLAUSES.contains(&upper.as_str())
                    && !(upper == "JOIN" && matches!(prev_upper.as_str(), "LEFT" | "RIGHT" | "INNER" | "OUTER" | "CROSS"))
                    && !(upper == "OUTER" && matches!(prev_upper.as_str(), "LEFT" | "RIGHT"));

                if should_break {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    out.push('\n');
                    if matches!(upper.as_str(), "AND" | "OR" | "ON") {
                        out.push_str("  ");
                    }
                }
                out.push_str(&upper);
                prev_upper = upper;
                prev_was_space = false;
            }
            TokenKind::Punctuation => {
                match tok.text {
                    "(" => {
                        paren_depth += 1;
                        out.push('(');
                        prev_was_space = true;
                    }
                    ")" => {
                        paren_depth = paren_depth.saturating_sub(1);
                        while out.ends_with(' ') {
                            out.pop();
                        }
                        out.push(')');
                        prev_was_space = false;
                    }
                    "," => {
                        while out.ends_with(' ') {
                            out.pop();
                        }
                        out.push_str(", ");
                        prev_was_space = true;
                    }
                    "." => {
                        while out.ends_with(' ') {
                            out.pop();
                        }
                        out.push('.');
                        prev_was_space = true;
                    }
                    ";" => {
                        while out.ends_with(' ') {
                            out.pop();
                        }
                        out.push_str(";\n");
                        prev_was_space = true;
                    }
                    other => {
                        out.push_str(other);
                        prev_was_space = false;
                    }
                }
                prev_upper.clear();
            }
            _ => {
                out.push_str(tok.text);
                prev_was_space = false;
                prev_upper.clear();
            }
        }
    }

    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_select_into_readable_clauses() {
        let formatted = format_mysql("select id, name from users where active = 1 and role = 'admin' order by id desc limit 50");
        assert!(formatted.contains("SELECT id, name\nFROM users\nWHERE active = 1\n  AND role = 'admin'\nORDER BY id DESC\nLIMIT 50"));
    }
}
