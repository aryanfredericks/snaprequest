//! TLS: trust roots, the traced handshake, and certificate details for the trace.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Instant;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig as RustlsConfig, ClientConnection, RootCertStore};
use time::format_description::well_known::Rfc3339;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use url::{Host, Url};
use x509_parser::prelude::{FromDer, GeneralName, X509Certificate};

use super::error_chain;
use super::recorder::Recorder;
use crate::model::{CertificateInfo, Phase, TlsInfo, TraceEvent};
use crate::{Error, Result};

/// A DER-encoded X.509 certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate(Vec<u8>);

impl Certificate {
    /// Wraps DER bytes.
    pub fn from_der(der: impl Into<Vec<u8>>) -> Self {
        Self(der.into())
    }

    /// Parses every certificate in a PEM bundle.
    pub fn from_pem(pem: &[u8]) -> Result<Vec<Self>> {
        let certs = rustls_pemfile::certs(&mut &pem[..])
            .map(|c| c.map(|c| Self(c.to_vec())))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| Error::InvalidRequest(format!("invalid PEM certificate: {e}")))?;
        if certs.is_empty() {
            return Err(Error::InvalidRequest(
                "no certificates found in PEM data".into(),
            ));
        }
        Ok(certs)
    }

    pub fn as_der(&self) -> &[u8] {
        &self.0
    }
}

/// Builds the rustls configuration: OS trust store (or Mozilla roots if the OS store
/// is empty or unreadable) plus any extra roots.
pub(crate) fn build_config(extra_roots: &[Certificate]) -> Result<Arc<RustlsConfig>> {
    let mut roots = RootCertStore::empty();

    let native = rustls_native_certs::load_native_certs();
    for err in &native.errors {
        tracing::debug!(error = %err, "could not load some OS root certificates");
    }
    let (added, ignored) = roots.add_parsable_certificates(native.certs);
    tracing::debug!(added, ignored, "loaded OS root certificates");
    if added == 0 {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        tracing::debug!("OS trust store empty, using Mozilla roots");
    }

    for cert in extra_roots {
        roots
            .add(CertificateDer::from(cert.as_der().to_vec()))
            .map_err(|e| Error::InvalidRequest(format!("invalid root certificate: {e}")))?;
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = RustlsConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::InvalidRequest(format!("TLS configuration: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    // HTTP/2 negotiation arrives with the HTTP/2 transport.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// Performs the TLS handshake over `tcp`, recording `TlsStart`/`TlsEnd`.
/// The certificate is verified against the URL's host, even when the address was pinned.
pub(crate) async fn handshake(
    config: Arc<RustlsConfig>,
    tcp: TcpStream,
    url: &Url,
    rec: &mut Recorder,
) -> Result<TlsStream<TcpStream>> {
    rec.phase = Phase::Tls;
    let server_name = match url.host() {
        Some(Host::Domain(d)) => ServerName::try_from(d.to_owned())
            .map_err(|_| rec.fail(format!("`{d}` is not a valid TLS server name"), false))?,
        Some(Host::Ipv4(ip)) => ServerName::IpAddress(IpAddr::V4(ip).into()),
        Some(Host::Ipv6(ip)) => ServerName::IpAddress(IpAddr::V6(ip).into()),
        None => return Err(rec.fail("URL has no host", false)),
    };
    let shown = url.host_str().unwrap_or_default().to_owned();
    rec.record(TraceEvent::TlsStart {
        server_name: shown.clone(),
    });

    let started = Instant::now();
    let stream = TlsConnector::from(config)
        .connect(server_name, tcp)
        .await
        .map_err(|e| {
            rec.fail(
                format!("TLS handshake with `{shown}` failed: {}", error_chain(&e)),
                false,
            )
        })?;
    let duration = started.elapsed();

    rec.record(TraceEvent::TlsEnd {
        info: tls_info(stream.get_ref().1),
        duration,
    });
    Ok(stream)
}

fn tls_info(conn: &ClientConnection) -> TlsInfo {
    let version = match conn.protocol_version() {
        Some(rustls::ProtocolVersion::TLSv1_3) => "TLSv1.3".to_owned(),
        Some(rustls::ProtocolVersion::TLSv1_2) => "TLSv1.2".to_owned(),
        Some(other) => format!("{other:?}"),
        None => "unknown".to_owned(),
    };
    TlsInfo {
        version,
        cipher: conn
            .negotiated_cipher_suite()
            .map_or_else(|| "unknown".to_owned(), |c| format!("{:?}", c.suite())),
        alpn: conn
            .alpn_protocol()
            .map(|p| String::from_utf8_lossy(p).into_owned()),
        peer_certificates: conn
            .peer_certificates()
            .unwrap_or_default()
            .iter()
            .map(|der| certificate_info(der))
            .collect(),
    }
}

/// Summarizes a DER certificate. Never fails: unparseable certificates are still
/// listed with their fingerprint.
pub(crate) fn certificate_info(der: &[u8]) -> CertificateInfo {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    let sha256_fingerprint = digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":");

    let cert = match X509Certificate::from_der(der) {
        Ok((_, cert)) => cert,
        Err(e) => {
            return CertificateInfo {
                subject: format!("<unparseable certificate: {e}>"),
                issuer: String::new(),
                not_before: String::new(),
                not_after: String::new(),
                serial: String::new(),
                subject_alt_names: Vec::new(),
                sha256_fingerprint,
            };
        }
    };

    let rfc3339 =
        |t: &x509_parser::time::ASN1Time| t.to_datetime().format(&Rfc3339).unwrap_or_default();
    let mut subject_alt_names = Vec::new();
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for name in &san.value.general_names {
            match name {
                GeneralName::DNSName(d) => subject_alt_names.push((*d).to_owned()),
                GeneralName::IPAddress(bytes) => {
                    if let Ok(v4) = <[u8; 4]>::try_from(*bytes) {
                        subject_alt_names.push(Ipv4Addr::from(v4).to_string());
                    } else if let Ok(v6) = <[u8; 16]>::try_from(*bytes) {
                        subject_alt_names.push(Ipv6Addr::from(v6).to_string());
                    }
                }
                _ => {}
            }
        }
    }

    CertificateInfo {
        subject: cert.subject().to_string(),
        issuer: cert.issuer().to_string(),
        not_before: rfc3339(&cert.validity().not_before),
        not_after: rfc3339(&cert.validity().not_after),
        serial: cert.raw_serial_as_string(),
        subject_alt_names,
        sha256_fingerprint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn self_signed() -> (rcgen::Certificate, rcgen::KeyPair) {
        let mut params =
            rcgen::CertificateParams::new(vec!["snap.test".into(), "127.0.0.1".into()]).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "SnapRequest Test");
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        (cert, key)
    }

    #[test]
    fn certificate_details() {
        let (cert, _) = self_signed();
        let info = certificate_info(cert.der());
        assert_eq!(info.subject, "CN=SnapRequest Test");
        assert_eq!(info.issuer, "CN=SnapRequest Test");
        assert_eq!(info.subject_alt_names, ["snap.test", "127.0.0.1"]);
        assert_eq!(info.sha256_fingerprint.len(), 32 * 3 - 1);
        assert!(info.not_before.ends_with('Z'), "{}", info.not_before);
    }

    #[test]
    fn garbage_certificate_is_still_summarized() {
        let info = certificate_info(b"not a certificate");
        assert!(info.subject.starts_with("<unparseable certificate"));
        assert!(!info.sha256_fingerprint.is_empty());
    }

    #[test]
    fn pem_parsing() {
        let (cert, _) = self_signed();
        let certs = Certificate::from_pem(cert.pem().as_bytes()).unwrap();
        assert_eq!(certs, [Certificate::from_der(cert.der().to_vec())]);
        assert!(Certificate::from_pem(b"nothing here").is_err());
        assert!(build_config(&certs).is_ok());
    }
}
