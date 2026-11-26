use cottondata_core::{ServerProfile, TlsMode};
use tiberius::{AuthMethod, Config, EncryptionLevel};

const APPLICATION_NAME: &str = "CottonData";

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
}
