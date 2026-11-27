#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlErrorObjectKind {
    Table,
    Column,
    Function,
    Syntax,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqlErrorLocation {
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub char_offset: Option<usize>,
    pub near_token: Option<String>,
    pub object_name: Option<String>,
    pub object_kind: Option<SqlErrorObjectKind>,
}

impl SqlErrorLocation {
    pub fn is_empty(&self) -> bool {
        self.line.is_none()
            && self.column.is_none()
            && self.char_offset.is_none()
            && self.near_token.is_none()
            && self.object_name.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbDiagnostic {
    pub code: Option<u32>,
    pub sqlstate: Option<Box<str>>,
    pub severity: DiagnosticSeverity,
    pub summary: String,
    pub hint: Option<String>,
    pub raw_message: String,
    pub location: Option<Box<SqlErrorLocation>>,
}

impl DbDiagnostic {
    pub fn plain(raw: impl Into<String>, code: Option<u32>) -> Self {
        let raw_message = raw.into();
        Self {
            code,
            sqlstate: None,
            severity: DiagnosticSeverity::Error,
            summary: raw_message.clone(),
            hint: None,
            raw_message,
            location: None,
        }
    }

    pub fn classify(raw: impl Into<String>, code: Option<u32>) -> Self {
        let raw_message = raw.into();
        let lower = raw_message.to_ascii_lowercase();

        if code == Some(1820) || lower.contains("must reset your password") {
            return Self {
                code: Some(1820),
                sqlstate: None,
                severity: DiagnosticSeverity::Warning,
                summary: "MySQL password has expired".into(),
                hint: Some("Run ALTER USER USER() IDENTIFIED BY '<new_password>' or use the Reset Password flow.".into()),
                raw_message,
                location: None,
            };
        }

        if code == Some(1045) || lower.contains("access denied for user") {
            return Self {
                code: Some(1045),
                sqlstate: None,
                severity: DiagnosticSeverity::Error,
                summary: "Authentication failed (Access Denied)".into(),
                hint: Some("Verify username, password, allowed client host, and MySQL auth plugin (caching_sha2_password).".into()),
                raw_message,
                location: None,
            };
        }

        if lower.contains("server does not have this capability")
            || lower.contains("requires secure connection")
        {
            return Self {
                code,
                sqlstate: None,
                severity: DiagnosticSeverity::Warning,
                summary: "MySQL server does not support TLS/SSL".into(),
                hint: Some(
                    "This MySQL server has SSL disabled. Switch TLS Mode to 'Disabled' or 'Preferred'."
                        .into(),
                ),
                raw_message,
                location: None,
            };
        }

        if lower.contains("tls") || lower.contains("ssl") || lower.contains("handshake") || lower.contains("certificate") {
            return Self {
                code,
                sqlstate: None,
                severity: DiagnosticSeverity::Error,
                summary: "TLS / SSL handshake failed".into(),
                hint: Some("Check the server's TLS mode (Preferred / Required / Verify CA) or provide a valid CA certificate.".into()),
                raw_message,
                location: None,
            };
        }

        if lower.contains("time zone") || lower.contains("timezone") {
            return Self {
                code,
                sqlstate: None,
                severity: DiagnosticSeverity::Warning,
                summary: "Unrecognized MySQL server time zone".into(),
                hint: Some("Set session time zone explicitly (SET time_zone = '+00:00') or load MySQL tzinfo tables.".into()),
                raw_message,
                location: None,
            };
        }

        if matches!(code, Some(2006) | Some(2013)) || lower.contains("server has gone away") || lower.contains("lost connection") {
            return Self {
                code,
                sqlstate: None,
                severity: DiagnosticSeverity::Warning,
                summary: "MySQL connection lost".into(),
                hint: Some("The server idle timeout or SSH tunnel dropped. Reconnect or check wait_timeout / max_allowed_packet.".into()),
                raw_message,
                location: None,
            };
        }

        if code == Some(1317) || lower.contains("query execution was interrupted") {
            return Self {
                code: Some(1317),
                sqlstate: None,
                severity: DiagnosticSeverity::Info,
                summary: "Query cancelled by user".into(),
                hint: None,
                raw_message,
                location: None,
            };
        }

        let (clean_summary, hint, location) = parse_mysql_query_diagnostic(code, &raw_message);
        let summary = clean_summary.unwrap_or_else(|| {
            strip_mysql_error_wrapper(raw_message.lines().next().unwrap_or("MySQL error"))
        });

        Self {
            code,
            sqlstate: None,
            severity: DiagnosticSeverity::Error,
            summary,
            hint,
            raw_message,
            location: location.map(Box::new),
        }
    }
}

fn strip_mysql_error_wrapper(line: &str) -> String {
    let trimmed = line.trim();
    if let Some(inner) = trimmed
        .strip_prefix("MySqlError {")
        .and_then(|s| s.strip_suffix('}'))
    {
        return inner.trim().to_string();
    }
    trimmed.to_string()
}

fn parse_mysql_query_diagnostic(
    code: Option<u32>,
    raw: &str,
) -> (Option<String>, Option<String>, Option<SqlErrorLocation>) {
    let lower = raw.to_ascii_lowercase();

    if code == Some(1064) || lower.contains("error in your sql syntax") {
        let mut loc = SqlErrorLocation {
            object_kind: Some(SqlErrorObjectKind::Syntax),
            ..Default::default()
        };
        if let Some(near_pos) = lower.find("near '") {
            let after = &raw[near_pos + 6..];
            if let Some(end_quote) = after.rfind("' at line ") {
                let snippet = after[..end_quote].trim();
                if !snippet.is_empty() {
                    loc.near_token = Some(snippet.to_string());
                }
                let line_part = after[end_quote + 10..].trim();
                let digits: String = line_part.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(line_num) = digits.parse::<usize>() {
                    if line_num > 0 {
                        loc.line = Some(line_num);
                    }
                }
            } else if let Some(end_quote) = after.find('\'') {
                let snippet = after[..end_quote].trim();
                if !snippet.is_empty() {
                    loc.near_token = Some(snippet.to_string());
                }
            }
        }
        let summary = match &loc.near_token {
            Some(tok) => {
                let first_line = tok.lines().next().unwrap_or(tok).trim();
                let short: String = if first_line.chars().count() > 36 {
                    format!("{}…", first_line.chars().take(36).collect::<String>())
                } else {
                    first_line.to_string()
                };
                Some(format!("SQL syntax error near '{short}' (ERROR 1064)"))
            }
            None => Some("SQL syntax error (ERROR 1064)".into()),
        };
        let hint = Some("Check keywords, commas, parentheses, and quotes around the highlighted token.".into());
        return (summary, hint, (!loc.is_empty()).then_some(loc));
    }

    if code == Some(1054) || lower.contains("unknown column '") {
        let col = extract_single_quoted_after(raw, "Unknown column '")
            .or_else(|| extract_single_quoted_after_ci(raw, "unknown column '"));
        let summary = col
            .as_deref()
            .map(|c| format!("Unknown column '{c}' (ERROR 1054)"));
        let loc = col.map(|name| SqlErrorLocation {
            object_name: Some(name),
            object_kind: Some(SqlErrorObjectKind::Column),
            ..Default::default()
        });
        let hint = Some("Verify the column name against the table schema or qualify it with the right table alias.".into());
        return (summary, hint, loc);
    }

    if code == Some(1146) || (lower.contains("table '") && lower.contains("doesn't exist")) {
        let full_table = extract_single_quoted_after_ci(raw, "table '");
        let summary = full_table
            .as_deref()
            .map(|t| format!("Table '{t}' doesn't exist (ERROR 1146)"));
        let table = full_table.map(|full| full.rsplit('.').next().unwrap_or(&full).to_string());
        let loc = table.map(|name| SqlErrorLocation {
            object_name: Some(name),
            object_kind: Some(SqlErrorObjectKind::Table),
            ..Default::default()
        });
        let hint = Some("Check the active schema or qualify the table name as schema.table.".into());
        return (summary, hint, loc);
    }

    if code == Some(1052) || lower.contains("is ambiguous") {
        let col = extract_single_quoted_after_ci(raw, "column '");
        let summary = col
            .as_deref()
            .map(|c| format!("Column '{c}' is ambiguous (ERROR 1052)"));
        let loc = col.map(|name| SqlErrorLocation {
            object_name: Some(name),
            object_kind: Some(SqlErrorObjectKind::Column),
            ..Default::default()
        });
        let hint = Some("Qualify this column with its table name or alias (e.g. t.column_name).".into());
        return (summary, hint, loc);
    }

    if code == Some(1055) || lower.contains("only_full_group_by") {
        let col = extract_single_quoted_after_ci(raw, "column '");
        let summary = col
            .as_deref()
            .map(|c| format!("Column '{c}' not in GROUP BY (ERROR 1055)"));
        let loc = col.map(|full| SqlErrorLocation {
            object_name: Some(full.rsplit('.').next().unwrap_or(&full).to_string()),
            object_kind: Some(SqlErrorObjectKind::Column),
            ..Default::default()
        });
        let hint = Some(
            "Add non-aggregated SELECT columns to GROUP BY, or wrap them in an aggregate function like MAX() / ANY_VALUE().".into(),
        );
        return (summary, hint, loc);
    }

    if code == Some(1305) || lower.contains("does not exist") && lower.contains("function ") {
        let full_func = extract_word_after_ci(raw, "function ");
        let summary = full_func
            .as_deref()
            .map(|f| format!("Function '{f}' does not exist (ERROR 1305)"));
        let func = full_func.map(|full| full.rsplit('.').next().unwrap_or(&full).to_string());
        let loc = func.map(|name| SqlErrorLocation {
            object_name: Some(name),
            object_kind: Some(SqlErrorObjectKind::Function),
            ..Default::default()
        });
        let hint = Some("Check the function spelling and whether it is supported by this MySQL version.".into());
        return (summary, hint, loc);
    }

    (None, None, None)
}

fn extract_single_quoted_after(text: &str, prefix: &str) -> Option<String> {
    let idx = text.find(prefix)?;
    let rest = &text[idx + prefix.len()..];
    let end = rest.find('\'')?;
    let val = rest[..end].trim();
    (!val.is_empty()).then(|| val.to_string())
}

fn extract_single_quoted_after_ci(text: &str, prefix_lower: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let idx = lower.find(prefix_lower)?;
    let rest = &text[idx + prefix_lower.len()..];
    let end = rest.find('\'')?;
    let val = rest[..end].trim();
    (!val.is_empty()).then(|| val.to_string())
}

fn extract_word_after_ci(text: &str, prefix_lower: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let idx = lower.find(prefix_lower)?;
    let rest = text[idx + prefix_lower.len()..].trim_start();
    let token: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_expired_password_and_tls_errors() {
        let exp = DbDiagnostic::classify("You must reset your password using ALTER USER", Some(1820));
        assert_eq!(exp.code, Some(1820));
        assert!(exp.hint.unwrap().contains("ALTER USER"));

        let tls = DbDiagnostic::classify("Remote host terminated the SSL handshake", None);
        assert!(tls.summary.contains("TLS"));

        let no_tls = DbDiagnostic::classify(
            "DriverError { Client requires secure connection but server does not have this capability }",
            None,
        );
        assert!(no_tls.summary.contains("does not support TLS/SSL"));
    }

    #[test]
    fn parses_mysql_syntax_and_object_locations() {
        let syn = DbDiagnostic::classify(
            "You have an error in your SQL syntax; check the manual near 'FORM users' at line 2",
            Some(1064),
        );
        let loc = syn.location.expect("syntax location");
        assert_eq!(loc.line, Some(2));
        assert_eq!(loc.near_token.as_deref(), Some("FORM users"));

        let col = DbDiagnostic::classify("Unknown column 'u.emial' in 'field list'", Some(1054));
        let col_loc = col.location.expect("column location");
        assert_eq!(col_loc.object_name.as_deref(), Some("u.emial"));
        assert_eq!(col_loc.object_kind, Some(SqlErrorObjectKind::Column));

        let tbl = DbDiagnostic::classify("Table 'shop.usres' doesn't exist", Some(1146));
        let tbl_loc = tbl.location.expect("table location");
        assert_eq!(tbl_loc.object_name.as_deref(), Some("usres"));
        assert_eq!(tbl_loc.object_kind, Some(SqlErrorObjectKind::Table));
    }
}
