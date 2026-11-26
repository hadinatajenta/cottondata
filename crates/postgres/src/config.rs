use cottondata_core::{ConnectionEndpoint, DatabaseEngine, ServerProfile};
use cottondata_driver::{resolve_tcp_target, DbError, SshTunnel};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::time::Duration;
use tokio_postgres::config::SslMode;
use tokio_postgres::Config;

const APPLICATION_NAME: &str = "CottonData";
const SOCKET_FILE_PREFIX: &str = ".s.PGSQL.";
const DEFAULT_POSTGRES_PORT: u16 = 5432;

pub(crate) struct ConnectPlan {
    pub(crate) config: Config,
    pub(crate) tunnel: Option<SshTunnel>,
}

pub(crate) fn default_database(engine: DatabaseEngine) -> &'static str {
    match engine {
        DatabaseEngine::CockroachDb => "defaultdb",
        _ => "postgres",
    }
}

pub(crate) fn plan_connection(
    profile: &ServerProfile,
    password: Option<&str>,
    ssl_mode: SslMode,
    timeout: Duration,
) -> Result<ConnectPlan, DbError> {
    let mut config = Config::new();
    config
        .user(profile.username.trim())
        .dbname(
            profile
                .default_schema
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| default_database(profile.engine)),
        )
        .application_name(APPLICATION_NAME)
        .connect_timeout(timeout)
        .keepalives(true)
        .keepalives_idle(Duration::from_secs(60))
        .ssl_mode(ssl_mode);
    if let Some(secret) = password.filter(|secret| !secret.is_empty()) {
        config.password(secret);
    }

    let tunnel = match &profile.endpoint {
        ConnectionEndpoint::Tcp { host, .. } => {
            let target = resolve_tcp_target(profile, timeout)?;
            config.host(host.trim()).port(target.port);
            if target.tunnel.is_some() {
                config.hostaddr(IpAddr::V4(Ipv4Addr::LOCALHOST));
            }
            target.tunnel
        }
        ConnectionEndpoint::UnixSocket { path } => {
            apply_unix_socket(&mut config, path)?;
            None
        }
        ConnectionEndpoint::File { path } => {
            return Err(DbError::message(format!(
                "{} needs a host and port or a Unix socket, not the database file {path}",
                profile.engine.label()
            )));
        }
    };
    Ok(ConnectPlan { config, tunnel })
}

#[cfg(unix)]
fn apply_unix_socket(config: &mut Config, path: &str) -> Result<(), DbError> {
    let (directory, port) = split_socket_path(path);
    config.host_path(directory).port(port);
    config.ssl_mode(SslMode::Disable);
    Ok(())
}

#[cfg(not(unix))]
fn apply_unix_socket(_config: &mut Config, path: &str) -> Result<(), DbError> {
    Err(DbError::message(format!(
        "Unix sockets are not supported on this platform: {path}"
    )))
}

#[cfg_attr(not(unix), allow(dead_code))]
fn split_socket_path(path: &str) -> (String, u16) {
    let trimmed = path.trim();
    let candidate = Path::new(trimmed);
    let socket_port = candidate
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(SOCKET_FILE_PREFIX))
        .and_then(|port| port.parse::<u16>().ok());
    match (socket_port, candidate.parent()) {
        (Some(port), Some(directory)) => (directory.to_string_lossy().into_owned(), port),
        _ => (trimmed.to_string(), DEFAULT_POSTGRES_PORT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_postgres::config::Host;

    #[test]
    fn socket_file_paths_split_into_directory_and_port() {
        assert_eq!(
            split_socket_path("/var/run/postgresql/.s.PGSQL.5433"),
            ("/var/run/postgresql".to_string(), 5433)
        );
        assert_eq!(split_socket_path(" /tmp "), ("/tmp".to_string(), 5432));
    }

    #[test]
    fn empty_database_falls_back_to_engine_default() {
        let mut profile = ServerProfile::new_local("crdb", "127.0.0.1", 26257, "root");
        profile.engine = DatabaseEngine::CockroachDb;
        profile.ssh.enabled = false;
        let plan = plan_connection(&profile, None, SslMode::Disable, Duration::from_secs(5)).expect("plan");
        assert_eq!(plan.config.get_dbname(), Some("defaultdb"));
        assert_eq!(plan.config.get_hosts(), &[Host::Tcp("127.0.0.1".to_string())]);
        assert_eq!(plan.config.get_ports(), &[26257]);
        assert_eq!(plan.config.get_application_name(), Some(APPLICATION_NAME));
    }

    #[test]
    fn database_file_endpoints_are_rejected() {
        let mut profile = ServerProfile::new_local("pg", "127.0.0.1", 5432, "postgres");
        profile.engine = DatabaseEngine::PostgreSql;
        profile.endpoint = ConnectionEndpoint::File {
            path: "/tmp/app.db".into(),
        };
        let error = plan_connection(&profile, None, SslMode::Disable, Duration::from_secs(5))
            .err()
            .expect("file endpoint rejected");
        assert!(error.to_string().contains("PostgreSQL needs a host and port"));
    }
}
