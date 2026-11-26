use cottondata_core::{DbDiagnostic, DiagnosticSeverity};
use cottondata_driver::DbError;
use std::error::Error as StdError;
use std::io::ErrorKind;
use tokio_postgres::error::{DbError as ServerError, ErrorPosition, SqlState};

pub(crate) fn postgres_error(error: tokio_postgres::Error) -> DbError {
    postgres_error_for(error, None)
}

pub(crate) fn postgres_error_for(error: tokio_postgres::Error, sql: Option<&str>) -> DbError {
    if let Some(server_error) = error.as_db_error() {
        let diagnostic = classify_server_error(server_error, sql);
        return if is_connection_terminating(server_error.code()) {
            DbError::connection_lost(diagnostic)
        } else {
            DbError::new(diagnostic)
        };
    }
    if error.is_closed() {
        return DbError::connection_lost(DbDiagnostic {
            code: None,
            severity: DiagnosticSeverity::Warning,
            summary: "Connection to the server was closed".into(),
            hint: Some("The server restarted, an idle timeout fired, or the SSH tunnel dropped. Reconnect to continue.".into()),
            raw_message: error.to_string(),
        });
    }
    let raw_message = error_chain_text(&error);
    if let Some(kind) = io_error_kind(&error) {
        let diagnostic = DbDiagnostic {
            code: None,
            severity: DiagnosticSeverity::Error,
            summary: network_summary(kind).into(),
            hint: Some("Check the host, port, firewall, and that the server accepts TCP connections (listen_addresses).".into()),
            raw_message,
        };
        return if is_connection_lost_kind(kind) {
            DbError::connection_lost(diagnostic)
        } else {
            DbError::new(diagnostic)
        };
    }
    let lower = raw_message.to_ascii_lowercase();
    if lower.contains("tls") || lower.contains("certificate") || lower.contains("ssl") {
        return DbError::new(DbDiagnostic {
            code: None,
            severity: DiagnosticSeverity::Error,
            summary: "TLS handshake with the server failed".into(),
            hint: Some(
                "Try TLS mode Preferred or Required, or provide the server CA certificate for the Verify modes.".into(),
            ),
            raw_message,
        });
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return DbError::new(DbDiagnostic {
            code: None,
            severity: DiagnosticSeverity::Error,
            summary: "Timed out connecting to the server".into(),
            hint: Some("Check that the host is reachable, or raise the connect timeout in the server settings.".into()),
            raw_message,
        });
    }
    DbError::message(raw_message)
}

fn classify_server_error(server_error: &ServerError, sql: Option<&str>) -> DbDiagnostic {
    let code = server_error.code();
    let mut raw_message = format!("{} {}: {}", server_error.severity(), code.code(), server_error.message());
    if let Some(detail) = server_error.detail() {
        raw_message.push_str(&format!("\nDETAIL: {detail}"));
    }
    if let Some(location) = sql.zip(server_error.position()).and_then(|(text, position)| locate(text, position)) {
        raw_message.push_str(&format!("\nLOCATION: {location}"));
    }
    let (severity, summary, fallback_hint) = describe(code, server_error.message());
    DbDiagnostic {
        code: None,
        severity,
        summary,
        hint: server_error
            .hint()
            .map(str::to_string)
            .or_else(|| fallback_hint.map(str::to_string)),
        raw_message,
    }
}

fn describe(code: &SqlState, message: &str) -> (DiagnosticSeverity, String, Option<&'static str>) {
    let error = DiagnosticSeverity::Error;
    let with_code = || format!("{}: {message}", code.code());
    match code {
        c if *c == SqlState::INVALID_PASSWORD || *c == SqlState::INVALID_AUTHORIZATION_SPECIFICATION => (
            error,
            "Authentication failed".into(),
            Some("Check the username and password, and that pg_hba.conf allows this client and auth method."),
        ),
        c if *c == SqlState::INVALID_CATALOG_NAME => (
            error,
            with_code(),
            Some("Set an existing database in the server settings (Database field)."),
        ),
        c if *c == SqlState::UNDEFINED_TABLE => (
            error,
            with_code(),
            Some("Check the active schema (search_path) or qualify the table as schema.table."),
        ),
        c if *c == SqlState::INSUFFICIENT_PRIVILEGE => (
            error,
            format!("Permission denied: {message}"),
            Some("Ask a DBA to GRANT the needed privilege to this role."),
        ),
        c if *c == SqlState::QUERY_CANCELED => (DiagnosticSeverity::Info, "Query cancelled".into(), None),
        c if *c == SqlState::IN_FAILED_SQL_TRANSACTION => (
            DiagnosticSeverity::Warning,
            "Transaction is aborted".into(),
            Some("An earlier statement failed inside this transaction. Run ROLLBACK (or Rollback in the toolbar) to continue."),
        ),
        c if *c == SqlState::T_R_SERIALIZATION_FAILURE => (
            DiagnosticSeverity::Warning,
            format!("Transaction must be retried: {message}"),
            Some("Another transaction conflicted with this one. ROLLBACK and run the transaction again."),
        ),
        c if *c == SqlState::T_R_DEADLOCK_DETECTED => (
            DiagnosticSeverity::Warning,
            format!("Deadlock detected: {message}"),
            Some("ROLLBACK and retry; access tables in a consistent order to avoid deadlocks."),
        ),
        c if *c == SqlState::TOO_MANY_CONNECTIONS => (
            error,
            "The server has too many connections".into(),
            Some("Close idle sessions or raise max_connections on the server."),
        ),
        c if *c == SqlState::READ_ONLY_SQL_TRANSACTION => (
            DiagnosticSeverity::Warning,
            format!("Blocked by read-only mode: {message}"),
            Some("Disable Read-Only on this server slot to execute writes."),
        ),
        c if *c == SqlState::FEATURE_NOT_SUPPORTED => (error, format!("Not supported by this server: {message}"), None),
        _ => (error, with_code(), None),
    }
}

fn is_connection_terminating(code: &SqlState) -> bool {
    *code == SqlState::ADMIN_SHUTDOWN
        || *code == SqlState::CRASH_SHUTDOWN
        || *code == SqlState::CANNOT_CONNECT_NOW
        || *code == SqlState::IDLE_SESSION_TIMEOUT
}

fn locate(sql: &str, position: &ErrorPosition) -> Option<String> {
    let ErrorPosition::Original(character_position) = position else {
        return None;
    };
    let character_index = usize::try_from(*character_position).ok()?.checked_sub(1)?;
    let prefix: String = sql.chars().take(character_index).collect();
    let line = prefix.matches('\n').count() + 1;
    let column = prefix.rsplit('\n').next().map(|tail| tail.chars().count()).unwrap_or(0) + 1;
    Some(format!("line {line}, column {column}"))
}

fn error_chain_text(error: &tokio_postgres::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

fn io_error_kind(error: &tokio_postgres::Error) -> Option<ErrorKind> {
    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(io_error) = cause.downcast_ref::<std::io::Error>() {
            return Some(io_error.kind());
        }
        source = cause.source();
    }
    None
}

fn network_summary(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::ConnectionRefused => "Connection refused by the server",
        ErrorKind::TimedOut => "Timed out connecting to the server",
        ErrorKind::NotFound => "Server address or socket not found",
        _ if is_connection_lost_kind(kind) => "Connection to the server was lost",
        _ => "Network error talking to the server",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_positions_are_reported_as_line_and_column() {
        let sql = "SELECT 1;\nSELECT nope FROM t";
        assert_eq!(locate(sql, &ErrorPosition::Original(18)), Some("line 2, column 8".into()));
        assert_eq!(locate(sql, &ErrorPosition::Original(1)), Some("line 1, column 1".into()));
        assert_eq!(locate(sql, &ErrorPosition::Original(0)), None);
    }

    #[test]
    fn sqlstates_map_to_actionable_summaries() {
        let (_, summary, hint) = describe(&SqlState::INVALID_PASSWORD, "password authentication failed");
        assert_eq!(summary, "Authentication failed");
        assert!(hint.unwrap().contains("pg_hba.conf"));
        let (severity, summary, _) = describe(&SqlState::QUERY_CANCELED, "canceling statement");
        assert_eq!((severity, summary.as_str()), (DiagnosticSeverity::Info, "Query cancelled"));
        let (_, summary, hint) = describe(&SqlState::T_R_SERIALIZATION_FAILURE, "restart transaction");
        assert!(summary.starts_with("Transaction must be retried"));
        assert!(hint.is_some());
        let (_, summary, _) = describe(&SqlState::UNDEFINED_COLUMN, "column \"x\" does not exist");
        assert_eq!(summary, "42703: column \"x\" does not exist");
    }

    #[test]
    fn dropped_connections_are_flagged_for_reconnect() {
        assert!(is_connection_lost_kind(ErrorKind::ConnectionReset));
        assert!(!is_connection_lost_kind(ErrorKind::ConnectionRefused));
        assert!(is_connection_terminating(&SqlState::ADMIN_SHUTDOWN));
    }
}
