use crate::lexer::{tokenize, TokenKind, MYSQL_KEYWORDS};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    Function,
    SystemVar,
    Table,
    Column,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    pub kind: CompletionKind,
}

#[derive(Debug, Clone)]
pub struct SchemaTableHint {
    pub name: String,
    pub columns: Vec<(String, String)>,
}

pub const MYSQL_FUNCTIONS: &[(&str, &str)] = &[
    ("ABS", "ABS(x)"),
    ("AVG", "AVG([DISTINCT] expr)"),
    ("BIN_TO_UUID", "BIN_TO_UUID(binary_uuid[, swap_flag])"),
    ("CAST", "CAST(expr AS type)"),
    ("CEIL", "CEIL(x)"),
    ("CHAR_LENGTH", "CHAR_LENGTH(str)"),
    ("COALESCE", "COALESCE(val1, val2, ...)"),
    ("CONCAT", "CONCAT(str1, str2, ...)"),
    ("CONCAT_WS", "CONCAT_WS(separator, str1, str2, ...)"),
    ("CONVERT", "CONVERT(expr, type)"),
    ("COUNT", "COUNT(* | expr)"),
    ("CURDATE", "CURDATE()"),
    ("CURRENT_TIMESTAMP", "CURRENT_TIMESTAMP([fsp])"),
    ("DATABASE", "DATABASE()"),
    ("DATE", "DATE(expr)"),
    ("DATE_ADD", "DATE_ADD(date, INTERVAL expr unit)"),
    ("DATE_FORMAT", "DATE_FORMAT(date, format)"),
    ("DATE_SUB", "DATE_SUB(date, INTERVAL expr unit)"),
    ("DATEDIFF", "DATEDIFF(expr1, expr2)"),
    ("DENSE_RANK", "DENSE_RANK() OVER (...)"),
    ("FLOOR", "FLOOR(x)"),
    ("FROM_UNIXTIME", "FROM_UNIXTIME(unix_timestamp[, format])"),
    ("GROUP_CONCAT", "GROUP_CONCAT(expr SEPARATOR ',')"),
    ("HEX", "HEX(val)"),
    ("IF", "IF(expr1, expr2, expr3)"),
    ("IFNULL", "IFNULL(expr1, expr2)"),
    ("INET6_ATON", "INET6_ATON(expr)"),
    ("INET6_NTOA", "INET6_NTOA(expr)"),
    ("INSTR", "INSTR(str, substr)"),
    ("JSON_ARRAY", "JSON_ARRAY([val, ...])"),
    ("JSON_CONTAINS", "JSON_CONTAINS(target, candidate[, path])"),
    ("JSON_EXTRACT", "JSON_EXTRACT(json_doc, path, ...)"),
    ("JSON_OBJECT", "JSON_OBJECT([key, val, ...])"),
    ("JSON_PRETTY", "JSON_PRETTY(json_val)"),
    ("JSON_SET", "JSON_SET(json_doc, path, val, ...)"),
    ("JSON_TABLE", "JSON_TABLE(expr, path COLUMNS (...))"),
    ("JSON_UNQUOTE", "JSON_UNQUOTE(json_val)"),
    ("LAG", "LAG(expr[, N[, default]]) OVER (...)"),
    ("LAST_INSERT_ID", "LAST_INSERT_ID()"),
    ("LEAD", "LEAD(expr[, N[, default]]) OVER (...)"),
    ("LEFT", "LEFT(str, len)"),
    ("LENGTH", "LENGTH(str)"),
    ("LOWER", "LOWER(str)"),
    ("LPAD", "LPAD(str, len, padstr)"),
    ("LTRIM", "LTRIM(str)"),
    ("MAX", "MAX(expr)"),
    ("MD5", "MD5(str)"),
    ("MIN", "MIN(expr)"),
    ("NOW", "NOW([fsp])"),
    ("NULLIF", "NULLIF(expr1, expr2)"),
    ("RANK", "RANK() OVER (...)"),
    ("REGEXP_LIKE", "REGEXP_LIKE(expr, pat[, match_type])"),
    ("REGEXP_REPLACE", "REGEXP_REPLACE(expr, pat, repl)"),
    ("REPLACE", "REPLACE(str, from_str, to_str)"),
    ("RIGHT", "RIGHT(str, len)"),
    ("ROUND", "ROUND(x[, d])"),
    ("ROW_NUMBER", "ROW_NUMBER() OVER (...)"),
    ("RPAD", "RPAD(str, len, padstr)"),
    ("RTRIM", "RTRIM(str)"),
    ("SHA2", "SHA2(str, hash_length)"),
    ("SUBSTRING", "SUBSTRING(str, pos[, len])"),
    ("SUBSTRING_INDEX", "SUBSTRING_INDEX(str, delim, count)"),
    ("SUM", "SUM(expr)"),
    ("TIMESTAMPDIFF", "TIMESTAMPDIFF(unit, datetime1, datetime2)"),
    ("TRIM", "TRIM([remstr FROM] str)"),
    ("UNIX_TIMESTAMP", "UNIX_TIMESTAMP([date])"),
    ("UPPER", "UPPER(str)"),
    ("USER", "USER()"),
    ("UUID", "UUID()"),
    ("UUID_TO_BIN", "UUID_TO_BIN(string_uuid[, swap_flag])"),
    ("VERSION", "VERSION()"),
];

pub const MYSQL_SYSVARS: &[&str] = &[
    "@@autocommit",
    "@@character_set_client",
    "@@character_set_connection",
    "@@character_set_results",
    "@@character_set_server",
    "@@collation_connection",
    "@@default_storage_engine",
    "@@event_scheduler",
    "@@gtid_mode",
    "@@innodb_buffer_pool_size",
    "@@innodb_lock_wait_timeout",
    "@@lower_case_table_names",
    "@@max_allowed_packet",
    "@@max_connections",
    "@@max_execution_time",
    "@@read_only",
    "@@sql_mode",
    "@@time_zone",
    "@@transaction_isolation",
    "@@version",
    "@@version_comment",
    "@@wait_timeout",
];

pub struct CompletionProvider;

impl CompletionProvider {
    pub fn suggest(
        sql: &str,
        cursor_byte: usize,
        tables: &[SchemaTableHint],
        limit: usize,
    ) -> Vec<CompletionItem> {
        let clamped = cursor_byte.min(sql.len());
        let before = &sql[..clamped];

        let prefix_start = before
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '.' | '`')))
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let raw_prefix = &before[prefix_start..];
        let mut results = Vec::new();

        if raw_prefix.starts_with("@@") {
            let lower = raw_prefix.to_ascii_lowercase();
            for &var in MYSQL_SYSVARS {
                if var.starts_with(&lower) {
                    results.push(CompletionItem {
                        label: var.to_string(),
                        detail: "MySQL system variable".into(),
                        kind: CompletionKind::SystemVar,
                    });
                }
            }
            results.truncate(limit);
            return results;
        }

        if let Some((qualifier, col_prefix)) = raw_prefix.rsplit_once('.') {
            let qual_clean = qualifier.trim_matches('`').to_ascii_lowercase();
            let col_lower = col_prefix.trim_matches('`').to_ascii_lowercase();
            let aliases = extract_table_aliases(sql);
            let resolved_table = aliases.get(&qual_clean).map(|s| s.as_str()).unwrap_or(&qual_clean);

            for table in tables {
                if table.name.eq_ignore_ascii_case(resolved_table) {
                    for (col, dtype) in &table.columns {
                        if col.to_ascii_lowercase().starts_with(&col_lower) {
                            results.push(CompletionItem {
                                label: col.clone(),
                                detail: format!("{} · {}", table.name, dtype),
                                kind: CompletionKind::Column,
                            });
                        }
                    }
                }
            }
            results.truncate(limit);
            return results;
        }

        let clean = raw_prefix.trim_matches('`').to_ascii_lowercase();
        if clean.is_empty() {
            return results;
        }

        for table in tables {
            if table.name.to_ascii_lowercase().starts_with(&clean) {
                results.push(CompletionItem {
                    label: table.name.clone(),
                    detail: format!("table ({} cols)", table.columns.len()),
                    kind: CompletionKind::Table,
                });
            }
        }

        let aliases = extract_table_aliases(sql);
        for table in tables {
            let is_referenced = aliases.values().any(|t| t.eq_ignore_ascii_case(&table.name));
            if is_referenced {
                for (col, dtype) in &table.columns {
                    if col.to_ascii_lowercase().starts_with(&clean) {
                        results.push(CompletionItem {
                            label: col.clone(),
                            detail: format!("{} · {}", table.name, dtype),
                            kind: CompletionKind::Column,
                        });
                    }
                }
            }
        }

        for &(func, sig) in MYSQL_FUNCTIONS {
            if func.to_ascii_lowercase().starts_with(&clean) {
                results.push(CompletionItem {
                    label: func.to_string(),
                    detail: sig.to_string(),
                    kind: CompletionKind::Function,
                });
            }
        }

        for &kw in MYSQL_KEYWORDS {
            if kw.to_ascii_lowercase().starts_with(&clean) {
                results.push(CompletionItem {
                    label: kw.to_string(),
                    detail: "keyword".into(),
                    kind: CompletionKind::Keyword,
                });
            }
        }

        results.truncate(limit);
        results
    }
}

fn extract_table_aliases(sql: &str) -> HashMap<String, String> {
    let tokens: Vec<_> = tokenize(sql)
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    let mut map = HashMap::new();
    let mut i = 0;

    while i < tokens.len() {
        let upper = tokens[i].text.to_ascii_uppercase();
        if matches!(upper.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
            if let Some(tbl_tok) = tokens.get(i + 1) {
                if matches!(tbl_tok.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) {
                    let mut table_name = tbl_tok.text.trim_matches('`').to_string();
                    let mut next_idx = i + 2;
                    if tokens.get(next_idx).map(|t| t.text) == Some(".") {
                        if let Some(sub_tbl) = tokens.get(next_idx + 1) {
                            table_name = sub_tbl.text.trim_matches('`').to_string();
                            next_idx += 2;
                        }
                    }
                    map.insert(table_name.to_ascii_lowercase(), table_name.clone());

                    if let Some(cand) = tokens.get(next_idx) {
                        if cand.text.eq_ignore_ascii_case("AS") {
                            if let Some(alias) = tokens.get(next_idx + 1) {
                                map.insert(alias.text.trim_matches('`').to_ascii_lowercase(), table_name);
                            }
                        } else if matches!(cand.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier) {
                            map.insert(cand.text.trim_matches('`').to_ascii_lowercase(), table_name);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_table_alias_dot_completion() {
        let tables = vec![SchemaTableHint {
            name: "orders".into(),
            columns: vec![
                ("id".into(), "bigint unsigned".into()),
                ("order_total".into(), "decimal(12,2)".into()),
                ("created_at".into(), "timestamp".into()),
            ],
        }];
        let sql = "SELECT o.ord FROM orders o WHERE o.id > 10";
        let cursor = sql.find(" FROM").unwrap();
        let items = CompletionProvider::suggest(sql, cursor, &tables, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "order_total");
        assert_eq!(items[0].kind, CompletionKind::Column);
    }
}
