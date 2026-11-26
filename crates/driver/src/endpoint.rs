use crate::{DbError, SshTunnel};
use cottondata_core::{ConnectionEndpoint, ServerProfile};
use std::time::Duration;

pub struct TcpTarget {
    pub host: String,
    pub port: u16,
    pub tunnel: Option<SshTunnel>,
}

pub fn resolve_tcp_target(profile: &ServerProfile, timeout: Duration) -> Result<TcpTarget, DbError> {
    let ConnectionEndpoint::Tcp { host, port } = &profile.endpoint else {
        return Err(DbError::message(format!(
            "{} connections need a host and port, not {}",
            profile.engine.label(),
            profile.endpoint.display_address()
        )));
    };
    if !profile.ssh.enabled {
        return Ok(TcpTarget {
            host: host.clone(),
            port: *port,
            tunnel: None,
        });
    }
    let tunnel = SshTunnel::open(&profile.ssh, host, *port, timeout).map_err(DbError::message)?;
    Ok(TcpTarget {
        host: "127.0.0.1".into(),
        port: tunnel.local_port(),
        tunnel: Some(tunnel),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cottondata_core::DatabaseEngine;

    #[test]
    fn non_tcp_endpoints_are_rejected_with_the_engine_name() {
        let mut profile = ServerProfile::new_local("mssql", "127.0.0.1", 1433, "sa");
        profile.engine = DatabaseEngine::SqlServer;
        profile.ssh.enabled = false;
        profile.endpoint = ConnectionEndpoint::UnixSocket {
            path: "/tmp/x.sock".into(),
        };
        let error = resolve_tcp_target(&profile, Duration::from_secs(1))
            .err()
            .expect("socket rejected");
        assert!(error.to_string().contains("SQL Server connections need a host and port"));
    }

    #[test]
    fn direct_tcp_endpoints_resolve_without_a_tunnel() {
        let mut profile = ServerProfile::new_local("pg", "db.internal", 5432, "postgres");
        profile.ssh.enabled = false;
        let target = resolve_tcp_target(&profile, Duration::from_secs(1)).expect("tcp target");
        assert_eq!((target.host.as_str(), target.port), ("db.internal", 5432));
        assert!(target.tunnel.is_none());
    }
}
