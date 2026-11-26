use cottondata_core::{DbDiagnostic, DiagnosticSeverity};
use cottondata_driver::DbError;
use rusqlite::ErrorCode;

pub(crate) fn sqlite_error(error: rusqlite::Error) -> DbError {
    let raw_message = error.to_string();
    let Some(code) = error.sqlite_error_code() else {
        return DbError::message(raw_message);
    };
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
        severity,
        summary,
        hint: hint.map(str::to_string),
        raw_message,
    })
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
}
