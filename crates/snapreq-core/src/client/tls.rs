//! TLS: trust roots, the traced handshake, and certificate details for the trace.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{
    ClientConfig as RustlsConfig, ClientConnection, DigitallySignedStruct, RootCertStore,
};
use time::format_description::well_known::Rfc3339;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use url::{Host, Url};
use x509_parser::prelude::{FromDer, GeneralName, X509Certificate};

use super::error_chain;
use super::recorder::Recorder;
use crate::model::{
    CertificateInfo, ClientCertificate, HttpVersionPref, Phase, TlsInfo, TlsSettings, TlsVersion,
    TraceEvent,
};
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
        let certs = CertificateDer::pem_slice_iter(pem)
            .map(|c| c.map(|c| Self(c.to_vec())))
            .collect::<std::result::Result<Vec<_>, _>>()
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

/// Client-wide TLS state. Loading the OS trust store is the expensive part, so it
/// happens once; per-request configurations are cheap to build from it.
#[derive(Debug)]
pub(crate) struct TlsContext {
    roots: Arc<RootCertStore>,
    /// Default-settings configs for `Auto`, `Http1` and `Http2` (they differ only in ALPN).
    defaults: [Arc<RustlsConfig>; 3],
}

fn pref_index(pref: HttpVersionPref) -> usize {
    match pref {
        HttpVersionPref::Auto => 0,
        HttpVersionPref::Http1 => 1,
        HttpVersionPref::Http2 => 2,
    }
}

/// ALPN protocols to offer for a version preference.
fn alpn(pref: HttpVersionPref) -> Vec<Vec<u8>> {
    match pref {
        HttpVersionPref::Auto => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        HttpVersionPref::Http1 => vec![b"http/1.1".to_vec()],
        HttpVersionPref::Http2 => vec![b"h2".to_vec()],
    }
}

/// The TLS configuration for one request.
#[derive(Debug, Clone)]
pub(crate) struct RequestTls {
    config: Arc<RustlsConfig>,
    /// Set when verification is off: receives what verification would have said.
    verdict: Option<Arc<Mutex<Option<String>>>>,
    /// Subject of the client certificate, if one is presented.
    client_cert_subject: Option<String>,
}

impl TlsContext {
    /// OS trust store (or Mozilla roots if it is empty or unreadable) plus `extra_roots`.
    pub(crate) fn new(extra_roots: &[Certificate]) -> Result<Self> {
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

        let roots = Arc::new(roots);
        let defaults = [
            HttpVersionPref::Auto,
            HttpVersionPref::Http1,
            HttpVersionPref::Http2,
        ]
        .map(|pref| build(&roots, &TlsSettings::default(), None, None, pref));
        let [auto, h1, h2] = defaults;
        Ok(Self {
            roots,
            defaults: [auto?, h1?, h2?],
        })
    }

    /// Configuration for `settings`. Reads CA and client certificate files, so file
    /// problems surface here, before any connection is made.
    pub(crate) async fn for_request(
        &self,
        settings: &TlsSettings,
        pref: HttpVersionPref,
    ) -> Result<RequestTls> {
        if settings.is_default() {
            return Ok(RequestTls {
                config: self.defaults[pref_index(pref)].clone(),
                verdict: None,
                client_cert_subject: None,
            });
        }

        let roots = match &settings.ca_cert {
            None => self.roots.clone(),
            Some(path) => {
                let mut roots = (*self.roots).clone();
                for cert in Certificate::from_pem(&read_file(path, "CA certificate").await?)
                    .map_err(|e| Error::InvalidRequest(format!("`{}`: {e}", path.display())))?
                {
                    roots.add(CertificateDer::from(cert.0)).map_err(|e| {
                        Error::InvalidRequest(format!(
                            "invalid CA certificate in `{}`: {e}",
                            path.display()
                        ))
                    })?;
                }
                Arc::new(roots)
            }
        };

        let client_auth = match &settings.client_cert {
            None => None,
            Some(cc) => Some(load_client_cert(cc).await?),
        };
        let client_cert_subject = client_auth
            .as_ref()
            .and_then(|(chain, _)| chain.first())
            .map(|leaf| certificate_info(leaf).subject);

        let verdict = (!settings.verify).then(|| Arc::new(Mutex::new(None)));
        let config = build(&roots, settings, client_auth, verdict.clone(), pref)?;
        Ok(RequestTls {
            config,
            verdict,
            client_cert_subject,
        })
    }
}

type ClientAuth = (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>);

fn build(
    roots: &Arc<RootCertStore>,
    settings: &TlsSettings,
    client_auth: Option<ClientAuth>,
    verdict: Option<Arc<Mutex<Option<String>>>>,
    pref: HttpVersionPref,
) -> Result<Arc<RustlsConfig>> {
    let min = settings.min_version.unwrap_or(TlsVersion::Tls12);
    let max = settings.max_version.unwrap_or(TlsVersion::Tls13);
    let versions: Vec<&'static rustls::SupportedProtocolVersion> = [
        (TlsVersion::Tls12, &rustls::version::TLS12),
        (TlsVersion::Tls13, &rustls::version::TLS13),
    ]
    .into_iter()
    .filter(|(v, _)| (min..=max).contains(v))
    .map(|(_, p)| p)
    .collect();
    if versions.is_empty() {
        return Err(Error::InvalidRequest(
            "TLS min_version is higher than max_version".into(),
        ));
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = RustlsConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&versions)
        .map_err(|e| Error::InvalidRequest(format!("TLS configuration: {e}")))?;

    let builder = match verdict {
        None => builder.with_root_certificates(roots.clone()),
        Some(verdict) => {
            let inner = WebPkiServerVerifier::builder_with_provider(roots.clone(), provider)
                .build()
                .map_err(|e| Error::InvalidRequest(format!("TLS configuration: {e}")))?;
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(RecordingVerifier { inner, verdict }))
        }
    };

    let mut config = match client_auth {
        None => builder.with_no_client_auth(),
        Some((chain, key)) => builder
            .with_client_auth_cert(chain, key)
            .map_err(|e| Error::InvalidRequest(format!("client certificate: {e}")))?,
    };
    config.alpn_protocols = alpn(pref);
    Ok(Arc::new(config))
}

async fn read_file(path: &Path, what: &str) -> Result<Vec<u8>> {
    tokio::fs::read(path)
        .await
        .map_err(|e| Error::InvalidRequest(format!("cannot read {what} `{}`: {e}", path.display())))
}

async fn load_client_cert(cc: &ClientCertificate) -> Result<ClientAuth> {
    let pem = read_file(&cc.cert, "client certificate").await?;
    let chain: Vec<CertificateDer<'static>> = Certificate::from_pem(&pem)
        .map_err(|e| Error::InvalidRequest(format!("`{}`: {e}", cc.cert.display())))?
        .into_iter()
        .map(|c| CertificateDer::from(c.0))
        .collect();

    let key_pem = read_file(&cc.key, "client key").await?;
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|e| match e {
        rustls::pki_types::pem::Error::NoItemsFound => {
            Error::InvalidRequest(format!("no private key found in `{}`", cc.key.display()))
        }
        e => Error::InvalidRequest(format!("`{}`: {e}", cc.key.display())),
    })?;
    Ok((chain, key))
}

/// Accepts any certificate but records what real verification would have decided,
/// so the trace can say *why* the connection would normally fail. Handshake
/// signatures are still checked.
#[derive(Debug)]
struct RecordingVerifier {
    inner: Arc<WebPkiServerVerifier>,
    verdict: Arc<Mutex<Option<String>>>,
}

impl ServerCertVerifier for RecordingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let outcome = self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        );
        if let Err(e) = outcome {
            *self.verdict.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Performs the TLS handshake over `tcp`, recording `TlsStart`/`TlsEnd`.
/// The certificate is verified against the URL's host, even when the address was pinned.
pub(crate) async fn handshake(
    tls: &RequestTls,
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
    if let Some(subject) = &tls.client_cert_subject {
        rec.record(TraceEvent::Info {
            message: format!("presenting client certificate {subject}"),
        });
    }
    rec.record(TraceEvent::TlsStart {
        server_name: shown.clone(),
    });

    let started = Instant::now();
    let stream = TlsConnector::from(tls.config.clone())
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
    if let Some(verdict) = &tls.verdict {
        let verdict = verdict.lock().unwrap_or_else(|p| p.into_inner()).take();
        rec.record(TraceEvent::Info {
            message: match verdict {
                Some(why) => format!(
                    "TLS verification is DISABLED; this certificate would have been rejected: {why}"
                ),
                None => "TLS verification is disabled (the certificate would have passed)".into(),
            },
        });
    }
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
        assert!(TlsContext::new(&certs).is_ok());
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[tokio::test]
    async fn version_range_must_not_be_empty() {
        let ctx = TlsContext::new(&[]).unwrap();
        let err = ctx
            .for_request(
                &TlsSettings {
                    min_version: Some(TlsVersion::Tls13),
                    max_version: Some(TlsVersion::Tls12),
                    ..TlsSettings::default()
                },
                HttpVersionPref::Auto,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("min_version is higher"), "{err}");
    }

    #[tokio::test]
    async fn missing_files_are_reported_with_their_path() {
        let ctx = TlsContext::new(&[]).unwrap();
        let err = ctx
            .for_request(
                &TlsSettings {
                    ca_cert: Some("missing/ca.pem".into()),
                    ..TlsSettings::default()
                },
                HttpVersionPref::Auto,
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot read CA certificate `missing/ca.pem`"),
            "{err}"
        );

        let err = ctx
            .for_request(
                &TlsSettings {
                    client_cert: Some(ClientCertificate {
                        cert: "missing/me.crt".into(),
                        key: "missing/me.key".into(),
                    }),
                    ..TlsSettings::default()
                },
                HttpVersionPref::Auto,
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("cannot read client certificate"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn default_settings_reuse_the_shared_config() {
        let ctx = TlsContext::new(&[]).unwrap();
        let a = ctx
            .for_request(&TlsSettings::default(), HttpVersionPref::Auto)
            .await
            .unwrap();
        let b = ctx
            .for_request(&TlsSettings::default(), HttpVersionPref::Auto)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&a.config, &b.config));
    }
}
