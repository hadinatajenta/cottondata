use cottondata_core::{DbDiagnostic, DiagnosticSeverity, SqlErrorLocation, SqlErrorObjectKind};
use cottondata_driver::DbError;
use std::io::ErrorKind;

const LOGIN_FAILED: u32 = 18456;
const CANNOT_OPEN_DATABASE: u32 = 4060;
const PERMISSION_DENIED_CODES: [u32; 3] = [229, 262, 300];

pub(crate) fn mssql_error(error: tiberius::error::Error) -> DbError {
    match error {
        tiberius::error::Error::Server(token) => {
            let line = usize::try_from(token.line()).ok().filter(|&l| l > 0);
            DbError::new(classify_server_error(token.code(), token.message(), line))
        }
        tiberius::error::Error::Io { kind, message } => {
            let diagnostic = DbDiagnostic::plain(format!("Network error: {message}"), None);
            if is_connection_lost_kind(kind) {
                DbError::connection_lost(diagnostic)
            } else {
                DbError::new(diagnostic)
            }
        }
        tiberius::error::Error::Tls(message) => DbError::new(DbDiagnostic {
            code: None,
            sqlstate: None,
            severity: DiagnosticSeverity::Error,
            summary: "TLS handshake with SQL Server failed".into(),
            hint: Some("Try TLS mode Preferred, or provide the server CA certificate for Verify modes.".into()),
            raw_message: message,
            location: None,
        }),
        other => DbError::message(other.to_string()),
    }
}

fn is_connection_lost_kind(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::BrokenPipe
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::UnexpectedEof
            | ErrorKind::NotConnected
    )
}

fn classify_server_error(code: u32, message: &str, line: Option<usize>) -> DbDiagnostic {
    let (summary, hint, location) = match code {
        LOGIN_FAILED => (
            "Authentication failed (Login failed)".to_string(),
            Some("Check the SQL login and password, and that SQL Server authentication mode is enabled.".to_string()),
            None,
        ),
        CANNOT_OPEN_DATABASE => (
            "Cannot open the requested database".to_string(),
            Some("Check the default database in the server settings and that the login has access to it.".to_string()),
            None,
        ),
        _ if PERMISSION_DENIED_CODES.contains(&code) => (
            "Permission denied".to_string(),
            Some("Ask a DBA to grant the needed permission, for example VIEW SERVER STATE for the monitor.".to_string()),
            None,
        ),
        102 | 156 => {
            let near = extract_single_quoted(message);
            let loc = SqlErrorLocation {
                line,
                near_token: near,
                object_kind: Some(SqlErrorObjectKind::Syntax),
                ..Default::default()
            };
            (
                format!("Msg {code}: {message}"),
                Some("Check T-SQL syntax, keywords, commas, and parentheses near this token.".to_string()),
                (!loc.is_empty()).then_some(loc),
            )
        }
        207 => {
            let col = extract_single_quoted(message);
            let loc = SqlErrorLocation {
                line,
                object_name: col,
                object_kind: Some(SqlErrorObjectKind::Column),
                ..Default::default()
            };
            (
                format!("Msg {code}: {message}"),
                Some("Verify the column name against the table definition or alias.".to_string()),
                (!loc.is_empty()).then_some(loc),
            )
        }
        208 => {
            let tbl = extract_single_quoted(message)
                .map(|full| full.rsplit('.').next().unwrap_or(&full).to_string());
            let loc = SqlErrorLocation {
                line,
                object_name: tbl,
                object_kind: Some(SqlErrorObjectKind::Table),
                ..Default::default()
            };
            (
                format!("Msg {code}: {message}"),
                Some("Check the active database/schema or qualify the table as schema.table (e.g. dbo.table).".to_string()),
                (!loc.is_empty()).then_some(loc),
            )
        }
        209 => {
            let col = extract_single_quoted(message);
            let loc = SqlErrorLocation {
                line,
                object_name: col,
                object_kind: Some(SqlErrorObjectKind::Column),
                ..Default::default()
            };
            (
                format!("Msg {code}: {message}"),
                Some("Qualify this column with its table name or alias (e.g. t.column_name).".to_string()),
                (!loc.is_empty()).then_some(loc),
            )
        }
        8120 => {
            let col = extract_single_quoted(message)
                .map(|full| full.rsplit('.').next().unwrap_or(&full).to_string());
            let loc = SqlErrorLocation {
                line,
                object_name: col,
                object_kind: Some(SqlErrorObjectKind::Column),
                ..Default::default()
            };
            (
                format!("Msg {code}: {message}"),
                Some("Add this column to GROUP BY or wrap it in an aggregate function.".to_string()),
                (!loc.is_empty()).then_some(loc),
            )
        }
        _ => {
            let loc = line.map(|l| {
                Box::new(SqlErrorLocation {
                    line: Some(l),
                    ..Default::default()
                })
            });
            return DbDiagnostic {
                code: Some(code),
                sqlstate: None,
                severity: DiagnosticSeverity::Error,
                summary: format!("Msg {code}: {message}"),
                hint: None,
                raw_message: message.to_string(),
                location: loc,
            };
        }
    };
    DbDiagnostic {
        code: Some(code),
        sqlstate: None,
        severity: DiagnosticSeverity::Error,
        summary,
        hint,
        raw_message: message.to_string(),
        location: location.map(Box::new),
    }
}

fn extract_single_quoted(message: &str) -> Option<String> {
    let start = message.find('\'')?;
    let rest = &message[start + 1..];
    let end = rest.find('\'')?;
    let val = rest[..end].trim();
    (!val.is_empty()).then(|| val.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_failures_get_actionable_summary_and_keep_raw_message() {
        let diagnostic = classify_server_error(18456, "Login failed for user 'sa'.", None);
        assert_eq!(diagnostic.summary, "Authentication failed (Login failed)");
        assert_eq!(diagnostic.code, Some(18456));
        assert_eq!(diagnostic.raw_message, "Login failed for user 'sa'.");
    }

    #[test]
    fn unknown_server_errors_include_message_number_and_location() {
        let diagnostic = classify_server_error(208, "Invalid object name 'missing'.", Some(3));
        assert_eq!(diagnostic.summary, "Msg 208: Invalid object name 'missing'.");
        let loc = diagnostic.location.expect("location");
        assert_eq!(loc.line, Some(3));
        assert_eq!(loc.object_name.as_deref(), Some("missing"));
        assert_eq!(loc.object_kind, Some(SqlErrorObjectKind::Table));
    }

    #[test]
    fn broken_connections_are_flagged_for_reconnect() {
        let lost = mssql_error(tiberius::error::Error::Io {
            kind: ErrorKind::ConnectionReset,
            message: "reset by peer".into(),
        });
        assert!(lost.is_connection_lost);
        let refused = mssql_error(tiberius::error::Error::Io {
            kind: ErrorKind::ConnectionRefused,
            message: "refused".into(),
        });
        assert!(!refused.is_connection_lost);
    }
}
