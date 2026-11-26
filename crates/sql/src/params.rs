use std::collections::HashMap;

use cottondata_core::SqlDialect;

use crate::lexer::{tokenize, TokenKind};

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParamValueKind {
    #[default]
    Auto,
    String,
    Number,
    Null,
    RawSql,
}

impl ParamValueKind {
    pub const ALL: [ParamValueKind; 5] = [
        Self::Auto,
        Self::String,
        Self::Number,
        Self::Null,
        Self::RawSql,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::String => "Text",
            Self::Number => "Number",
            Self::Null => "NULL",
            Self::RawSql => "Raw SQL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParamBinding {
    pub kind: ParamValueKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryParamSpec {
    pub key: String,
    pub label: String,
    pub is_positional: bool,
    pub occurrences: Vec<(usize, usize)>,
}

fn is_param_ident(text: &str) -> bool {
    let mut bytes = text.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn extract_query_params_with_offset(sql: &str, next_positional: &mut usize) -> Vec<QueryParamSpec> {
    let tokens = tokenize(sql);
    let bytes = sql.as_bytes();
    let mut specs: Vec<QueryParamSpec> = Vec::new();
    let mut named_indices: HashMap<String, usize> = HashMap::new();
    let mut i = 0;

    while i < tokens.len() {
        let tok = &tokens[i];
        if tok.kind == TokenKind::Operator && tok.text == ":" {
            let preceded_by_colon = tok.start > 0 && bytes[tok.start - 1] == b':';
            let next_byte = bytes.get(tok.end).copied();
            let followed_by_disallowed = matches!(next_byte, Some(b'=' | b':'));

            if !preceded_by_colon && !followed_by_disallowed {
                if let Some(next) = tokens.get(i + 1) {
                    if next.start == tok.end
                        && matches!(next.kind, TokenKind::Identifier | TokenKind::Keyword)
                        && is_param_ident(next.text)
                    {
                        let key = format!(":{}", next.text);
                        let span = (tok.start, next.end);
                        if let Some(&idx) = named_indices.get(&key) {
                            specs[idx].occurrences.push(span);
                        } else {
                            named_indices.insert(key.clone(), specs.len());
                            specs.push(QueryParamSpec {
                                label: key.clone(),
                                key,
                                is_positional: false,
                                occurrences: vec![span],
                            });
                        }
                        i += 2;
                        continue;
                    }
                }
            }
        } else if tok.kind == TokenKind::Operator && tok.text == "?" {
            let idx = *next_positional;
            *next_positional += 1;
            specs.push(QueryParamSpec {
                key: format!("?{idx}"),
                label: format!("? #{idx}"),
                is_positional: true,
                occurrences: vec![(tok.start, tok.end)],
            });
        }
        i += 1;
    }

    specs
}

pub fn extract_query_params(sql: &str) -> Vec<QueryParamSpec> {
    let mut next_positional = 1usize;
    extract_query_params_with_offset(sql, &mut next_positional)
}

pub fn extract_params_for_statements(statements: &[String]) -> Vec<QueryParamSpec> {
    let mut specs: Vec<QueryParamSpec> = Vec::new();
    let mut named_indices: HashMap<String, usize> = HashMap::new();
    let mut next_positional = 1usize;

    for stmt in statements {
        for spec in extract_query_params_with_offset(stmt, &mut next_positional) {
            if spec.is_positional {
                specs.push(spec);
            } else if let Some(&idx) = named_indices.get(&spec.key) {
                specs[idx].occurrences.extend(spec.occurrences);
            } else {
                named_indices.insert(spec.key.clone(), specs.len());
                specs.push(spec);
            }
        }
    }

    specs
}

fn is_strict_integer(digits: &str) -> bool {
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits.len() == 1 || !digits.starts_with('0'))
}

fn is_strict_finite_number(trimmed: &str) -> bool {
    let unsigned = trimmed.strip_prefix('-').unwrap_or(trimmed);
    if unsigned.is_empty() {
        return false;
    }

    if let Some((int_part, frac_part)) = unsigned.split_once('.') {
        if !is_strict_integer(int_part)
            || frac_part.is_empty()
            || !frac_part.bytes().all(|b| b.is_ascii_digit())
        {
            return false;
        }
        trimmed.parse::<f64>().is_ok_and(f64::is_finite)
    } else {
        if !is_strict_integer(unsigned) {
            return false;
        }
        trimmed.parse::<i64>().is_ok() || trimmed.parse::<f64>().is_ok_and(f64::is_finite)
    }
}

pub fn format_param_literal(binding: &ParamBinding, dialect: SqlDialect) -> String {
    match binding.kind {
        ParamValueKind::Null => "NULL".into(),
        ParamValueKind::RawSql => {
            let trimmed = binding.value.trim();
            if trimmed.is_empty() {
                "NULL".into()
            } else {
                trimmed.to_string()
            }
        }
        ParamValueKind::Number => {
            let trimmed = binding.value.trim();
            if trimmed.is_empty() {
                "0".into()
            } else {
                trimmed.to_string()
            }
        }
        ParamValueKind::String => dialect.string_literal(&binding.value),
        ParamValueKind::Auto => {
            let trimmed = binding.value.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
                return "NULL".into();
            }
            if trimmed.eq_ignore_ascii_case("true") {
                return match dialect {
                    SqlDialect::MySql => "TRUE".into(),
                    SqlDialect::SqlServer => "1".into(),
                };
            }
            if trimmed.eq_ignore_ascii_case("false") {
                return match dialect {
                    SqlDialect::MySql => "FALSE".into(),
                    SqlDialect::SqlServer => "0".into(),
                };
            }
            if is_strict_finite_number(trimmed) {
                return trimmed.to_string();
            }
            dialect.string_literal(&binding.value)
        }
    }
}

fn apply_bindings(
    sql: &str,
    specs: &[QueryParamSpec],
    dialect: SqlDialect,
    bindings: &HashMap<String, ParamBinding>,
) -> String {
    if specs.is_empty() {
        return sql.to_string();
    }

    let default_binding = ParamBinding::default();
    let literals: Vec<String> = specs
        .iter()
        .map(|spec| {
            let binding = bindings.get(&spec.key).unwrap_or(&default_binding);
            format_param_literal(binding, dialect)
        })
        .collect();

    let mut spans: Vec<(usize, usize, &str)> = Vec::new();
    for (spec, literal) in specs.iter().zip(literals.iter()) {
        for &(start, end) in &spec.occurrences {
            spans.push((start, end, literal.as_str()));
        }
    }
    spans.sort_unstable_by_key(|&(start, _, _)| start);

    let mut out = String::with_capacity(sql.len());
    let mut cursor = 0usize;
    for (start, end, replacement) in spans {
        if start < cursor || end > sql.len() || start > end {
            continue;
        }
        out.push_str(&sql[cursor..start]);
        out.push_str(replacement);
        cursor = end;
    }
    out.push_str(&sql[cursor..]);
    out
}

pub fn bind_query_params(
    sql: &str,
    dialect: SqlDialect,
    bindings: &HashMap<String, ParamBinding>,
) -> String {
    let specs = extract_query_params(sql);
    apply_bindings(sql, &specs, dialect, bindings)
}

pub fn bind_statements_params(
    statements: &[String],
    dialect: SqlDialect,
    bindings: &HashMap<String, ParamBinding>,
) -> Vec<String> {
    let mut next_positional = 1usize;
    statements
        .iter()
        .map(|stmt| {
            let specs = extract_query_params_with_offset(stmt, &mut next_positional);
            apply_bindings(stmt, &specs, dialect, bindings)
        })
        .collect()
}

fn fnv1a_byte(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
}

pub fn query_param_fingerprint(sql: &str) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for (idx, chunk) in sql.split_whitespace().enumerate() {
        if idx > 0 {
            hash = fnv1a_byte(hash, b' ');
        }
        for byte in chunk.bytes() {
            hash = fnv1a_byte(hash, byte);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(kind: ParamValueKind, value: &str) -> ParamBinding {
        ParamBinding {
            kind,
            value: value.to_string(),
        }
    }

    #[test]
    fn param_value_kind_labels_and_all_order() {
        let labels: Vec<&str> = ParamValueKind::ALL.iter().map(|k| k.label()).collect();
        assert_eq!(labels, vec!["Auto", "Text", "Number", "NULL", "Raw SQL"]);
        assert_eq!(ParamBinding::default(), binding(ParamValueKind::Auto, ""));
    }

    #[test]
    fn extracts_named_repeated_and_mixed_positional_params() {
        let sql = "SELECT * FROM orders WHERE user_id = :user_id AND status = :status AND owner_id = :user_id AND region = ? AND tier = ?";
        let specs = extract_query_params(sql);

        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].key, ":user_id");
        assert_eq!(specs[0].label, ":user_id");
        assert!(!specs[0].is_positional);
        assert_eq!(specs[0].occurrences.len(), 2);
        assert_eq!(&sql[specs[0].occurrences[0].0..specs[0].occurrences[0].1], ":user_id");
        assert_eq!(&sql[specs[0].occurrences[1].0..specs[0].occurrences[1].1], ":user_id");

        assert_eq!(specs[1].key, ":status");
        assert!(!specs[1].is_positional);
        assert_eq!(specs[1].occurrences.len(), 1);

        assert_eq!(specs[2].key, "?1");
        assert_eq!(specs[2].label, "? #1");
        assert!(specs[2].is_positional);
        assert_eq!(specs[2].occurrences.len(), 1);
        assert_eq!(&sql[specs[2].occurrences[0].0..specs[2].occurrences[0].1], "?");

        assert_eq!(specs[3].key, "?2");
        assert_eq!(specs[3].label, "? #2");
        assert!(specs[3].is_positional);
        assert_eq!(specs[3].occurrences.len(), 1);
    }

    #[test]
    fn named_params_are_case_sensitive() {
        let sql = "SELECT :id, :ID, :id";
        let specs = extract_query_params(sql);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].key, ":id");
        assert_eq!(specs[0].occurrences.len(), 2);
        assert_eq!(specs[1].key, ":ID");
        assert_eq!(specs[1].occurrences.len(), 1);
    }

    #[test]
    fn ignores_placeholders_inside_strings_quoted_identifiers_and_comments() {
        let sql = r#"
            SELECT ':ignored_single', "?:ignored_double", `col:name?`,
                   :real_a
            FROM t -- line comment :ignored_dash ?
            # hash comment :ignored_hash ?
            /* block comment :ignored_block ? */
            WHERE x = ?
        "#;
        let specs = extract_query_params(sql);
        let keys: Vec<&str> = specs.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec![":real_a", "?1"]);
    }

    #[test]
    fn ignores_mysql_assignment_and_double_colon_casts() {
        let sql = "SET @x := 1; SELECT val::text, @y:=:bound_val, num::int FROM t WHERE a = :other";
        let specs = extract_query_params(sql);
        let keys: Vec<&str> = specs.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec![":bound_val", ":other"]);
    }

    #[test]
    fn formats_auto_values_with_leading_zero_preservation_and_dialect_escaping() {
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, ""), SqlDialect::MySql), "NULL");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "   "), SqlDialect::MySql), "NULL");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "NULL"), SqlDialect::MySql), "NULL");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "null"), SqlDialect::SqlServer), "NULL");

        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "true"), SqlDialect::MySql), "TRUE");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "FALSE"), SqlDialect::MySql), "FALSE");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "true"), SqlDialect::SqlServer), "1");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "false"), SqlDialect::SqlServer), "0");

        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "0"), SqlDialect::MySql), "0");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "42"), SqlDialect::MySql), "42");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "-3.14"), SqlDialect::MySql), "-3.14");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "0.5"), SqlDialect::SqlServer), "0.5");

        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "01"), SqlDialect::MySql), "'01'");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "007"), SqlDialect::MySql), "'007'");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "00123"), SqlDialect::SqlServer), "N'00123'");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "1.2.3"), SqlDialect::MySql), "'1.2.3'");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Auto, "+42"), SqlDialect::MySql), "'+42'");

        let escaped = binding(ParamValueKind::Auto, r"C:\dir\O'Brien");
        assert_eq!(format_param_literal(&escaped, SqlDialect::MySql), r"'C:\\dir\\O''Brien'");
        assert_eq!(format_param_literal(&escaped, SqlDialect::SqlServer), r"N'C:\dir\O''Brien'");
    }

    #[test]
    fn formats_explicit_kinds() {
        assert_eq!(format_param_literal(&binding(ParamValueKind::Null, "anything"), SqlDialect::MySql), "NULL");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Number, "  123  "), SqlDialect::MySql), "123");
        assert_eq!(format_param_literal(&binding(ParamValueKind::Number, "   "), SqlDialect::MySql), "0");
        assert_eq!(format_param_literal(&binding(ParamValueKind::String, "42"), SqlDialect::MySql), "'42'");
        assert_eq!(format_param_literal(&binding(ParamValueKind::String, r"a\b'c"), SqlDialect::MySql), r"'a\\b''c'");
        assert_eq!(format_param_literal(&binding(ParamValueKind::String, r"a\b'c"), SqlDialect::SqlServer), r"N'a\b''c'");
        assert_eq!(format_param_literal(&binding(ParamValueKind::RawSql, "  NOW()  "), SqlDialect::MySql), "NOW()");
        assert_eq!(format_param_literal(&binding(ParamValueKind::RawSql, "  "), SqlDialect::MySql), "NULL");
    }

    #[test]
    fn binds_single_query_with_named_and_positional_params() {
        let sql = "SELECT * FROM users WHERE id = :id AND code = ? AND alt = :id AND note = ':id'";
        let mut bindings = HashMap::new();
        bindings.insert(":id".into(), binding(ParamValueKind::Auto, "42"));
        bindings.insert("?1".into(), binding(ParamValueKind::Auto, "007"));

        let bound = bind_query_params(sql, SqlDialect::MySql, &bindings);
        assert_eq!(
            bound,
            "SELECT * FROM users WHERE id = 42 AND code = '007' AND alt = 42 AND note = ':id'"
        );
    }

    #[test]
    fn multi_statement_extract_and_bind_preserves_continuous_positional_numbering() {
        let statements = vec![
            "SELECT * FROM a WHERE tenant = :tenant AND x = ?".to_string(),
            "UPDATE b SET flag = ? WHERE tenant = :tenant AND y = ?".to_string(),
        ];

        let specs = extract_params_for_statements(&statements);
        let keys: Vec<&str> = specs.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec![":tenant", "?1", "?2", "?3"]);
        assert_eq!(specs[0].occurrences.len(), 2);
        assert_eq!(specs[1].label, "? #1");
        assert_eq!(specs[2].label, "? #2");
        assert_eq!(specs[3].label, "? #3");

        let mut bindings = HashMap::new();
        bindings.insert(":tenant".into(), binding(ParamValueKind::String, "acme"));
        bindings.insert("?1".into(), binding(ParamValueKind::Auto, "10"));
        bindings.insert("?2".into(), binding(ParamValueKind::Auto, "true"));
        bindings.insert("?3".into(), binding(ParamValueKind::Auto, "01"));

        let bound = bind_statements_params(&statements, SqlDialect::SqlServer, &bindings);
        assert_eq!(
            bound,
            vec![
                "SELECT * FROM a WHERE tenant = N'acme' AND x = 10",
                "UPDATE b SET flag = 1 WHERE tenant = N'acme' AND y = N'01'",
            ]
        );
    }

    #[test]
    fn fingerprint_ignores_whitespace_differences() {
        let a = "SELECT * FROM users WHERE id = :id AND active = ?";
        let b = "  SELECT   *\n\tFROM users\r\n  WHERE id = :id   AND active = ?  ";
        let c = "SELECT * FROM orders WHERE id = :id AND active = ?";

        assert_eq!(query_param_fingerprint(a), query_param_fingerprint(b));
        assert_ne!(query_param_fingerprint(a), query_param_fingerprint(c));
    }
}
