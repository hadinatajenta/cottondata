use cottondata_core::{DbDiagnostic, DiagnosticSeverity, SqlErrorLocation, SqlErrorObjectKind};
use cottondata_driver::DbError;
use rusqlite::ErrorCode;

pub(crate) fn sqlite_error(error: rusqlite::Error) -> DbError {
    let raw_message = error.to_string();
    let Some(code) = error.sqlite_error_code() else {
        return DbError::message(raw_message);
    };
    let (location, parsed_hint) = parse_sqlite_query_location(&raw_message);
    let (severity, summary, hint) = match code {
        ErrorCode::OperationInterrupted => (DiagnosticSeverity::Info, "Query cancelled".to_string(), None),
        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => (
            DiagnosticSeverity::Warning,
            "The database file is locked by another connection".to_string(),
            Some("Another process or open transaction holds the lock. Commit or close it, then retry."),
        ),
        ErrorCode::ReadOnly => (
            DiagnosticSeverity::Warning,
            "The database is read-only".to_string(),
            Some("Disable Read-Only on this connection, or check the file permissions."),
        ),
        ErrorCode::CannotOpen => (
            DiagnosticSeverity::Error,
            "Could not open the database file".to_string(),
            Some("Check that the folder exists and that CottonData may read and write it."),
        ),
        ErrorCode::NotADatabase => (
            DiagnosticSeverity::Error,
            "The file is not a SQLite database".to_string(),
            Some("Pick a SQLite database file (.db, .sqlite, .sqlite3)."),
        ),
        ErrorCode::ConstraintViolation => (DiagnosticSeverity::Error, format!("Constraint failed: {raw_message}"), None),
        _ => (DiagnosticSeverity::Error, raw_message.clone(), None),
    };
    DbError::new(DbDiagnostic {
        code: None,
        sqlstate: None,
        severity,
        summary,
        hint: hint.map(str::to_string).or(parsed_hint),
        raw_message,
        location: location.map(Box::new),
    })
}

fn parse_sqlite_query_location(raw: &str) -> (Option<SqlErrorLocation>, Option<String>) {
    let lower = raw.to_ascii_lowercase();
    if let Some(pos) = lower.find("near \"") {
        let rest = &raw[pos + 6..];
        if let Some(end) = rest.find('"') {
            let tok = rest[..end].trim();
            if !tok.is_empty() {
                return (
                    Some(SqlErrorLocation {
                        near_token: Some(tok.to_string()),
                        object_kind: Some(SqlErrorObjectKind::Syntax),
                        ..Default::default()
                    }),
                    Some("Check SQL syntax, commas, parentheses, and keywords near this token.".into()),
                );
            }
        }
    }
    if let Some(pos) = lower.find("no such table:") {
        let name = extract_sqlite_object(&raw[pos + 14..]);
        if let Some(table) = name {
            return (
                Some(SqlErrorLocation {
                    object_name: Some(table),
                    object_kind: Some(SqlErrorObjectKind::Table),
                    ..Default::default()
                }),
                Some("Verify the table name or attached database name.".into()),
            );
        }
    }
    if let Some(pos) = lower.find("no such column:") {
        let name = extract_sqlite_object(&raw[pos + 15..]);
        if let Some(col) = name {
            return (
                Some(SqlErrorLocation {
                    object_name: Some(col),
                    object_kind: Some(SqlErrorObjectKind::Column),
                    ..Default::default()
                }),
                Some("Verify the column name against the table schema (or use single quotes for string literals).".into()),
            );
        }
    }
    if let Some(pos) = lower.find("ambiguous column name:") {
        let name = extract_sqlite_object(&raw[pos + 22..]);
        if let Some(col) = name {
            return (
                Some(SqlErrorLocation {
                    object_name: Some(col),
                    object_kind: Some(SqlErrorObjectKind::Column),
                    ..Default::default()
                }),
                Some("Qualify this column with its table name or alias (e.g. t.column_name).".into()),
            );
        }
    }
    if let Some(pos) = lower.find("no such function:") {
        let name = extract_sqlite_object(&raw[pos + 17..]);
        if let Some(func) = name {
            return (
                Some(SqlErrorLocation {
                    object_name: Some(func),
                    object_kind: Some(SqlErrorObjectKind::Function),
                    ..Default::default()
                }),
                Some("Check the function spelling or use a built-in SQLite equivalent.".into()),
            );
        }
    }
    (None, None)
}

fn extract_sqlite_object(rest: &str) -> Option<String> {
    let trimmed = rest.trim_start();
    let token: String = trimmed
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, ')' | ',' | ';'))
        .collect();
    let clean = token.trim_matches(|c| matches!(c, '"' | '\'' | '`')).to_string();
    (!clean.is_empty()).then_some(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_and_locked_errors_get_friendly_summaries() {
        let interrupted = sqlite_error(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
            None,
        ));
        assert_eq!(interrupted.diagnostic.summary, "Query cancelled");
        let locked = sqlite_error(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".into()),
        ));
        assert!(locked.diagnostic.summary.contains("locked"));
        assert!(locked.diagnostic.hint.is_some());
    }

    #[test]
    fn extracts_sqlite_syntax_and_object_locations() {
        let (loc, _) = parse_sqlite_query_location("near \"FORM\": syntax error");
        let loc = loc.expect("syntax location");
        assert_eq!(loc.near_token.as_deref(), Some("FORM"));
        assert_eq!(loc.object_kind, Some(SqlErrorObjectKind::Syntax));

        let (tbl, _) = parse_sqlite_query_location("no such table: usres");
        let tbl = tbl.expect("table location");
        assert_eq!(tbl.object_name.as_deref(), Some("usres"));
        assert_eq!(tbl.object_kind, Some(SqlErrorObjectKind::Table));
    }
}
