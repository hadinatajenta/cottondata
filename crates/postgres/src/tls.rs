use cottondata_core::TlsMode;
use cottondata_driver::DbError;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use tokio_postgres::config::SslMode;
use tokio_postgres_rustls::MakeRustlsConnect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CertificateCheck {
    AcceptAny,
    ChainOnly,
    ChainAndHostname,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TlsPolicy {
    pub(crate) ssl_mode: SslMode,
    pub(crate) check: CertificateCheck,
}

pub(crate) fn tls_policy(mode: TlsMode) -> TlsPolicy {
    match mode {
        TlsMode::Disabled => TlsPolicy {
            ssl_mode: SslMode::Disable,
            check: CertificateCheck::AcceptAny,
        },
        TlsMode::Preferred => TlsPolicy {
            ssl_mode: SslMode::Prefer,
            check: CertificateCheck::AcceptAny,
        },
        TlsMode::Required => TlsPolicy {
            ssl_mode: SslMode::Require,
            check: CertificateCheck::AcceptAny,
        },
        TlsMode::VerifyCa => TlsPolicy {
            ssl_mode: SslMode::Require,
            check: CertificateCheck::ChainOnly,
        },
        TlsMode::VerifyIdentity => TlsPolicy {
            ssl_mode: SslMode::Require,
            check: CertificateCheck::ChainAndHostname,
        },
    }
}

pub(crate) fn build_tls_connector(
    check: CertificateCheck,
    ca_cert_path: Option<&str>,
) -> Result<MakeRustlsConnect, DbError> {
    let provider = crypto_provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|err| DbError::message(format!("Could not configure TLS: {err}")))?;
    let config = match check {
        CertificateCheck::AcceptAny => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate { provider }))
            .with_no_client_auth(),
        CertificateCheck::ChainOnly => {
            let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(load_roots(ca_cert_path)?), provider)
                .build()
                .map_err(|err| DbError::message(format!("Could not build the certificate verifier: {err}")))?;
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(IgnoreHostnameVerifier { inner }))
                .with_no_client_auth()
        }
        CertificateCheck::ChainAndHostname => builder
            .with_root_certificates(load_roots(ca_cert_path)?)
            .with_no_client_auth(),
    };
    Ok(MakeRustlsConnect::new(config))
}

fn crypto_provider() -> Arc<CryptoProvider> {
    CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()))
}

fn load_roots(ca_cert_path: Option<&str>) -> Result<RootCertStore, DbError> {
    let mut roots = RootCertStore::empty();
    match ca_cert_path.map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => {
            let file = File::open(path)
                .map_err(|err| DbError::message(format!("Could not open CA certificate {path}: {err}")))?;
            for certificate in rustls_pemfile::certs(&mut BufReader::new(file)) {
                let certificate = certificate
                    .map_err(|err| DbError::message(format!("Invalid CA certificate in {path}: {err}")))?;
                roots
                    .add(certificate)
                    .map_err(|err| DbError::message(format!("Rejected CA certificate in {path}: {err}")))?;
            }
            if roots.is_empty() {
                return Err(DbError::message(format!("No PEM certificates found in {path}")));
            }
        }
        None => {
            let native = rustls_native_certs::load_native_certs();
            let (added, _) = roots.add_parsable_certificates(native.certs);
            if added == 0 {
                return Err(DbError::message(
                    "No trusted system certificates were found; provide the server CA certificate file instead",
                ));
            }
        }
    }
    Ok(roots)
}

#[derive(Debug)]
struct AcceptAnyCertificate {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[derive(Debug)]
struct IgnoreHostnameVerifier {
    inner: Arc<WebPkiServerVerifier>,
}

impl ServerCertVerifier for IgnoreHostnameVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self
            .inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_modes_map_to_ssl_mode_and_certificate_checks() {
        assert_eq!(tls_policy(TlsMode::Disabled).ssl_mode, SslMode::Disable);
        assert_eq!(tls_policy(TlsMode::Preferred).ssl_mode, SslMode::Prefer);
        assert_eq!(tls_policy(TlsMode::Required).check, CertificateCheck::AcceptAny);
        assert_eq!(tls_policy(TlsMode::VerifyCa).check, CertificateCheck::ChainOnly);
        assert_eq!(
            tls_policy(TlsMode::VerifyIdentity),
            TlsPolicy {
                ssl_mode: SslMode::Require,
                check: CertificateCheck::ChainAndHostname,
            }
        );
    }

    #[test]
    fn accept_any_connector_builds_without_system_roots() {
        assert!(build_tls_connector(CertificateCheck::AcceptAny, None).is_ok());
    }

    #[test]
    fn missing_ca_file_reports_the_path() {
        let error = build_tls_connector(CertificateCheck::ChainOnly, Some("/nonexistent/ca.pem"))
            .err()
            .expect("missing file");
        assert!(error.to_string().contains("/nonexistent/ca.pem"));
    }
}
