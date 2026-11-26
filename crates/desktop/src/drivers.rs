use cottondata_core::{DatabaseEngine, ServerProfile};
use cottondata_driver::{DatabaseSession, DbError};
use cottondata_mssql::MssqlSession;
use cottondata_mysql::MySqlSession;
use cottondata_postgres::PostgresSession;
use cottondata_sqlite::SqliteSession;

pub fn install_tls_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

pub fn open_session(profile: &ServerProfile, password: Option<&str>) -> Result<Box<dyn DatabaseSession>, DbError> {
    match profile.engine {
        DatabaseEngine::MySql => Ok(Box::new(MySqlSession::connect(profile, password)?)),
        DatabaseEngine::SqlServer => Ok(Box::new(MssqlSession::connect(profile, password)?)),
        DatabaseEngine::PostgreSql | DatabaseEngine::CockroachDb => {
            Ok(Box::new(PostgresSession::connect(profile, password)?))
        }
        DatabaseEngine::Sqlite => Ok(Box::new(SqliteSession::connect(profile, password)?)),
    }
}

pub fn kill_running_query(profile: &ServerProfile, password: Option<&str>, connection_id: u32) -> Result<(), DbError> {
    match profile.engine {
        DatabaseEngine::MySql => MySqlSession::kill_query_via_aux(profile, password, connection_id),
        DatabaseEngine::SqlServer => MssqlSession::kill_session_via_aux(profile, password, connection_id),
        DatabaseEngine::PostgreSql | DatabaseEngine::CockroachDb => {
            PostgresSession::kill_query_via_aux(profile, password, connection_id)
        }
        DatabaseEngine::Sqlite => SqliteSession::interrupt_running_query(profile, connection_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_client_config_builds_without_ambiguous_crypto_provider_panic() {
        install_tls_crypto_provider();
        let roots = rustls::RootCertStore::empty();
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let installed = rustls::crypto::CryptoProvider::get_default().expect("provider installed");
        assert!(std::sync::Arc::ptr_eq(config.crypto_provider(), installed));
    }
}
