#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbDiagnostic {
    pub code: Option<u32>,
    pub severity: DiagnosticSeverity,
    pub summary: String,
    pub hint: Option<String>,
    pub raw_message: String,
}

impl DbDiagnostic {
    pub fn plain(raw: impl Into<String>, code: Option<u32>) -> Self {
        let raw_message = raw.into();
        Self {
            code,
            severity: DiagnosticSeverity::Error,
            summary: raw_message.clone(),
            hint: None,
            raw_message,
        }
    }

    pub fn classify(raw: impl Into<String>, code: Option<u32>) -> Self {
        let raw_message = raw.into();
        let lower = raw_message.to_ascii_lowercase();

        if code == Some(1820) || lower.contains("must reset your password") {
            return Self {
                code: Some(1820),
                severity: DiagnosticSeverity::Warning,
                summary: "MySQL password has expired".into(),
                hint: Some("Run ALTER USER USER() IDENTIFIED BY '<new_password>' or use the Reset Password flow.".into()),
                raw_message,
            };
        }

        if code == Some(1045) || lower.contains("access denied for user") {
            return Self {
                code: Some(1045),
                severity: DiagnosticSeverity::Error,
                summary: "Authentication failed (Access Denied)".into(),
                hint: Some("Verify username, password, allowed client host, and MySQL auth plugin (caching_sha2_password).".into()),
                raw_message,
            };
        }

        if lower.contains("server does not have this capability")
            || lower.contains("requires secure connection")
        {
            return Self {
                code,
                severity: DiagnosticSeverity::Warning,
                summary: "MySQL server does not support TLS/SSL".into(),
                hint: Some(
                    "This MySQL server has SSL disabled. Switch TLS Mode to 'Disabled' or 'Preferred'."
                        .into(),
                ),
                raw_message,
            };
        }

        if lower.contains("tls") || lower.contains("ssl") || lower.contains("handshake") || lower.contains("certificate") {
            return Self {
                code,
                severity: DiagnosticSeverity::Error,
                summary: "TLS / SSL handshake failed".into(),
                hint: Some("Check the server's TLS mode (Preferred / Required / Verify CA) or provide a valid CA certificate.".into()),
                raw_message,
            };
        }

        if lower.contains("time zone") || lower.contains("timezone") {
            return Self {
                code,
                severity: DiagnosticSeverity::Warning,
                summary: "Unrecognized MySQL server time zone".into(),
                hint: Some("Set session time zone explicitly (SET time_zone = '+00:00') or load MySQL tzinfo tables.".into()),
                raw_message,
            };
        }

        if matches!(code, Some(2006) | Some(2013)) || lower.contains("server has gone away") || lower.contains("lost connection") {
            return Self {
                code,
                severity: DiagnosticSeverity::Warning,
                summary: "MySQL connection lost".into(),
                hint: Some("The server idle timeout or SSH tunnel dropped. Reconnect or check wait_timeout / max_allowed_packet.".into()),
                raw_message,
            };
        }

        if code == Some(1317) || lower.contains("query execution was interrupted") {
            return Self {
                code: Some(1317),
                severity: DiagnosticSeverity::Info,
                summary: "Query cancelled by user".into(),
                hint: None,
                raw_message,
            };
        }

        Self {
            code,
            severity: DiagnosticSeverity::Error,
            summary: raw_message.lines().next().unwrap_or("MySQL error").to_string(),
            hint: None,
            raw_message,
        }
    }
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
}
