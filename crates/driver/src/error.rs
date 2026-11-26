use cottondata_core::DbDiagnostic;

#[derive(Debug, Clone)]
pub struct DbError {
    pub diagnostic: DbDiagnostic,
    pub is_connection_lost: bool,
}

impl DbError {
    pub fn new(diagnostic: DbDiagnostic) -> Self {
        Self {
            diagnostic,
            is_connection_lost: false,
        }
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self::new(DbDiagnostic::plain(message, None))
    }

    pub fn connection_lost(diagnostic: DbDiagnostic) -> Self {
        Self {
            diagnostic,
            is_connection_lost: true,
        }
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.diagnostic.summary)?;
        if !self.diagnostic.raw_message.is_empty() && self.diagnostic.raw_message != self.diagnostic.summary {
            write!(formatter, " [{}]", self.diagnostic.raw_message)?;
        }
        if let Some(hint) = &self.diagnostic.hint {
            write!(formatter, " - {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DbError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_raw_detail_only_when_it_adds_information() {
        let plain = DbError::message("Not connected");
        assert_eq!(plain.to_string(), "Not connected");

        let mut diagnostic = DbDiagnostic::plain("Login failed for user 'sa'", Some(18456));
        diagnostic.summary = "Authentication failed".into();
        diagnostic.hint = Some("Check the SQL login".into());
        assert_eq!(
            DbError::new(diagnostic).to_string(),
            "Authentication failed [Login failed for user 'sa'] - Check the SQL login"
        );
    }
}
