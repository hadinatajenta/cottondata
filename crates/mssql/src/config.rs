use cottondata_core::{ConnectionEndpoint, ServerProfile, TlsMode};
use cottondata_driver::{DbError, SshTunnel};
use std::time::Duration;
use tiberius::{AuthMethod, Config, EncryptionLevel};

const APPLICATION_NAME: &str = "CottonData";

pub(crate) struct ConnectTarget {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) tunnel: Option<SshTunnel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CertificateTrust {
    AcceptAny,
    SystemRoots,
    CustomAuthority(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TlsPolicy {
    pub(crate) encryption: EncryptionLevel,
    pub(crate) trust: CertificateTrust,
}

pub(crate) fn tls_policy(mode: TlsMode, ca_cert_path: Option<&str>) -> TlsPolicy {
    let custom_authority = ca_cert_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| CertificateTrust::CustomAuthority(path.to_string()));
    match mode {
        TlsMode::Disabled => TlsPolicy {
            encryption: EncryptionLevel::NotSupported,
            trust: CertificateTrust::AcceptAny,
        },
        TlsMode::Preferred => TlsPolicy {
            encryption: EncryptionLevel::On,
            trust: CertificateTrust::AcceptAny,
        },
        TlsMode::Required => TlsPolicy {
            encryption: EncryptionLevel::Required,
            trust: CertificateTrust::AcceptAny,
        },
        TlsMode::VerifyCa | TlsMode::VerifyIdentity => TlsPolicy {
            encryption: EncryptionLevel::Required,
            trust: custom_authority.unwrap_or(CertificateTrust::SystemRoots),
        },
    }
}

pub(crate) fn resolve_target(profile: &ServerProfile, timeout: Duration) -> Result<ConnectTarget, DbError> {
    let ConnectionEndpoint::Tcp { host, port } = &profile.endpoint else {
        return Err(DbError::message(
            "SQL Server connections need a host and port, not a Unix socket",
        ));
    };
    if !profile.ssh.enabled {
        return Ok(ConnectTarget {
            host: host.clone(),
            port: *port,
            tunnel: None,
        });
    }
    let tunnel = SshTunnel::open(&profile.ssh, host, *port, timeout).map_err(DbError::message)?;
    Ok(ConnectTarget {
        host: "127.0.0.1".into(),
        port: tunnel.local_port(),
        tunnel: Some(tunnel),
    })
}

pub(crate) fn build_config(profile: &ServerProfile, password: Option<&str>, host: &str, port: u16) -> Config {
    let mut config = Config::new();
    config.host(host);
    config.port(port);
    config.application_name(APPLICATION_NAME);
    config.authentication(AuthMethod::sql_server(&profile.username, password.unwrap_or_default()));
    if let Some(database) = profile
        .default_schema
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        config.database(database);
    }
    let policy = tls_policy(profile.tls, profile.ca_cert_path.as_deref());
    config.encryption(policy.encryption);
    match policy.trust {
        CertificateTrust::AcceptAny => config.trust_cert(),
        CertificateTrust::CustomAuthority(path) => config.trust_cert_ca(path),
        CertificateTrust::SystemRoots => {}
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_and_preferred_modes_skip_certificate_validation() {
        assert_eq!(
            tls_policy(TlsMode::Disabled, None),
            TlsPolicy {
                encryption: EncryptionLevel::NotSupported,
                trust: CertificateTrust::AcceptAny,
            }
        );
        assert_eq!(tls_policy(TlsMode::Preferred, None).encryption, EncryptionLevel::On);
        assert_eq!(tls_policy(TlsMode::Required, None).trust, CertificateTrust::AcceptAny);
    }

    #[test]
    fn verify_modes_require_encryption_and_validate_against_custom_or_system_roots() {
        assert_eq!(
            tls_policy(TlsMode::VerifyCa, Some(" /certs/ca.pem ")),
            TlsPolicy {
                encryption: EncryptionLevel::Required,
                trust: CertificateTrust::CustomAuthority("/certs/ca.pem".into()),
            }
        );
        assert_eq!(
            tls_policy(TlsMode::VerifyIdentity, Some("")).trust,
            CertificateTrust::SystemRoots
        );
    }

    #[test]
    fn unix_socket_endpoints_are_rejected_with_a_clear_message() {
        let mut profile = ServerProfile::new_local("mssql", "127.0.0.1", 1433, "sa");
        profile.ssh.enabled = false;
        profile.endpoint = ConnectionEndpoint::UnixSocket {
            path: "/tmp/x.sock".into(),
        };
        let error = resolve_target(&profile, Duration::from_secs(1))
            .err()
            .expect("socket rejected");
        assert!(error.to_string().contains("Unix socket"));
    }
}
