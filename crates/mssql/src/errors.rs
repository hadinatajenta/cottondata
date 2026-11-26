use cottondata_core::{DbDiagnostic, DiagnosticSeverity};
use cottondata_driver::DbError;
use std::io::ErrorKind;

const LOGIN_FAILED: u32 = 18456;
const CANNOT_OPEN_DATABASE: u32 = 4060;
const PERMISSION_DENIED_CODES: [u32; 3] = [229, 262, 300];

pub(crate) fn mssql_error(error: tiberius::error::Error) -> DbError {
    match error {
        tiberius::error::Error::Server(token) => DbError::new(classify_server_error(token.code(), token.message())),
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
            severity: DiagnosticSeverity::Error,
            summary: "TLS handshake with SQL Server failed".into(),
            hint: Some("Try TLS mode Preferred, or provide the server CA certificate for Verify modes.".into()),
            raw_message: message,
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

fn classify_server_error(code: u32, message: &str) -> DbDiagnostic {
    let (summary, hint) = match code {
        LOGIN_FAILED => (
            "Authentication failed (Login failed)",
            Some("Check the SQL login and password, and that SQL Server authentication mode is enabled."),
        ),
        CANNOT_OPEN_DATABASE => (
            "Cannot open the requested database",
            Some("Check the default database in the server settings and that the login has access to it."),
        ),
        _ if PERMISSION_DENIED_CODES.contains(&code) => (
            "Permission denied",
            Some("Ask a DBA to grant the needed permission, for example VIEW SERVER STATE for the monitor."),
        ),
        _ => return DbDiagnostic::plain(format!("Msg {code}: {message}"), Some(code)),
    };
    DbDiagnostic {
        code: Some(code),
        severity: DiagnosticSeverity::Error,
        summary: summary.into(),
        hint: hint.map(str::to_string),
        raw_message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_failures_get_actionable_summary_and_keep_raw_message() {
        let diagnostic = classify_server_error(18456, "Login failed for user 'sa'.");
        assert_eq!(diagnostic.summary, "Authentication failed (Login failed)");
        assert_eq!(diagnostic.code, Some(18456));
        assert_eq!(diagnostic.raw_message, "Login failed for user 'sa'.");
    }

    #[test]
    fn unknown_server_errors_include_message_number() {
        let diagnostic = classify_server_error(208, "Invalid object name 'missing'.");
        assert_eq!(diagnostic.summary, "Msg 208: Invalid object name 'missing'.");
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
