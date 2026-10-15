//! TLS for the VeNCrypt X509 login methods.
//!
//! Raspberry Pis use a self-signed certificate, so instead of checking it
//! against certificate authorities we remember its fingerprint the first
//! time (like SSH does) and warn if it ever changes. The handshake
//! signatures are still verified normally.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::crypto::{CryptoProvider, ring, verify_tls12_signature, verify_tls13_signature};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{self, ClientConfig, DigitallySignedStruct, SignatureScheme};

use crate::error::SessionError;
use crate::session::BoxedStream;

#[derive(Debug)]
struct RememberCertificate {
    provider: Arc<CryptoProvider>,
    seen: Arc<Mutex<Option<Vec<u8>>>>,
}

impl ServerCertVerifier for RememberCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.seen.lock() = Some(end_entity.to_vec());
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Run the TLS handshake. Returns the encrypted stream and the server
/// certificate (DER) so the caller can check its fingerprint.
pub async fn connect(stream: BoxedStream, host: &str) -> Result<(BoxedStream, Vec<u8>), SessionError> {
    let provider = Arc::new(ring::default_provider());
    let seen = Arc::new(Mutex::new(None));
    let verifier = RememberCertificate { provider: provider.clone(), seen: seen.clone() };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| SessionError::protocol(format!("encryption setup failed ({e})")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    let name = ServerName::try_from(host.to_owned()).unwrap_or_else(|_| ServerName::try_from("localhost").unwrap());
    let tls = TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await
        .map_err(|e| SessionError::protocol(format!("the encrypted (TLS) connection could not be set up ({e})")))?;
    let cert = seen.lock().take().ok_or_else(|| SessionError::protocol("the server sent no certificate"))?;
    Ok((Box::new(tls), cert))
}
