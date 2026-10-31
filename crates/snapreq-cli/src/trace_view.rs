//! The verbose trace view (`-v`, `-vv`), written to stderr in a curl-like style:
//! `*` engine events, `>` request lines, `<` response lines.

use std::io::{self, Write};
use std::time::Duration;

use anstyle::{AnsiColor, Style};
use snapreq_core::model::{CertificateInfo, Header, TraceEntry, TraceEvent};

use crate::output::human_bytes;

const INFO: Style = Style::new().dimmed();
const SENT: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Cyan)));
const RECEIVED: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green)));
const ERROR: Style = Style::new()
    .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Red)))
    .bold();
const STAMP: Style = Style::new().dimmed();

fn ms(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 10.0 {
        format!("{ms:.2} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

struct View<'a> {
    out: &'a mut dyn Write,
    level: u8,
}

impl View<'_> {
    fn line(&mut self, at: Duration, style: Style, prefix: char, text: &str) -> io::Result<()> {
        if self.level >= 2 {
            write!(
                self.out,
                "{STAMP}[{:>9.3}]{STAMP:#} ",
                at.as_secs_f64() * 1000.0
            )?;
        }
        writeln!(self.out, "{style}{prefix} {text}{style:#}")
    }

    fn info(&mut self, at: Duration, text: &str) -> io::Result<()> {
        self.line(at, INFO, '*', text)
    }

    fn headers(
        &mut self,
        at: Duration,
        style: Style,
        prefix: char,
        headers: &[Header],
    ) -> io::Result<()> {
        for h in headers {
            self.line(at, style, prefix, &format!("{}: {}", h.name, h.value))?;
        }
        Ok(())
    }

    fn certificate(&mut self, at: Duration, index: usize, c: &CertificateInfo) -> io::Result<()> {
        let role = if index == 0 {
            "server certificate"
        } else {
            "issuer"
        };
        self.info(at, &format!("  {role}: {}", c.subject))?;
        if self.level >= 2 {
            self.info(at, &format!("    issued by: {}", c.issuer))?;
            self.info(
                at,
                &format!("    valid: {} → {}", c.not_before, c.not_after),
            )?;
            if !c.subject_alt_names.is_empty() {
                self.info(
                    at,
                    &format!("    names: {}", c.subject_alt_names.join(", ")),
                )?;
            }
            self.info(at, &format!("    serial: {}", c.serial))?;
            self.info(at, &format!("    sha256: {}", c.sha256_fingerprint))?;
        } else if index == 0 {
            self.info(at, &format!("    valid until {}", c.not_after))?;
        }
        Ok(())
    }

    fn wire(
        &mut self,
        at: Duration,
        label: &str,
        style: Style,
        prefix: char,
        data: &[u8],
    ) -> io::Result<()> {
        self.info(at, &format!("{label} ({})", human_bytes(data.len() as u64)))?;
        if data.is_empty() {
            return Ok(());
        }
        match std::str::from_utf8(data) {
            Ok(text) => {
                for line in text.split_inclusive('\n') {
                    let shown: String = line
                        .trim_end_matches(['\r', '\n'])
                        .chars()
                        .map(|c| if c.is_control() && c != '\t' { '·' } else { c })
                        .collect();
                    self.line(at, style, prefix, &shown)?;
                }
                Ok(())
            }
            Err(_) => {
                let head_end = data
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4);
                if let Some(end) = head_end.filter(|e| std::str::from_utf8(&data[..*e]).is_ok()) {
                    self.wire(at, "headers", style, prefix, &data[..end])?;
                    self.info(
                        at,
                        &format!(
                            "  [{} of binary body]",
                            human_bytes((data.len() - end) as u64)
                        ),
                    )
                } else {
                    self.info(at, "  [binary data]")
                }
            }
        }
    }
}

/// Renders a trace. `level` is the number of `-v` flags (1 or more).
pub(crate) fn render(out: &mut dyn Write, trace: &[TraceEntry], level: u8) -> io::Result<()> {
    let mut v = View { out, level };
    for entry in trace {
        let at = entry.at;
        match &entry.event {
            TraceEvent::DnsStart { host } => {
                if level >= 2 {
                    v.info(at, &format!("resolving {host}"))?;
                }
            }
            TraceEvent::DnsEnd {
                host,
                addrs,
                duration,
            } => {
                let addrs: Vec<String> = addrs.iter().map(ToString::to_string).collect();
                v.info(
                    at,
                    &format!("resolved {host} → {} ({})", addrs.join(", "), ms(*duration)),
                )?;
            }
            TraceEvent::ConnectStart { addr } => {
                if level >= 2 {
                    v.info(at, &format!("connecting to {addr}"))?;
                }
            }
            TraceEvent::ConnectEnd {
                addr,
                local_addr,
                duration,
            } => {
                v.info(
                    at,
                    &format!("connected to {addr} from {local_addr} ({})", ms(*duration)),
                )?;
            }
            TraceEvent::ConnectionReused { addr } => {
                v.info(at, &format!("reusing connection to {addr}"))?;
            }
            TraceEvent::TlsStart { server_name } => {
                if level >= 2 {
                    v.info(at, &format!("TLS handshake with {server_name}"))?;
                }
            }
            TraceEvent::TlsEnd { info, duration } => {
                let alpn = info
                    .alpn
                    .as_deref()
                    .map(|a| format!(", ALPN {a}"))
                    .unwrap_or_default();
                v.info(
                    at,
                    &format!("{} {}{alpn} ({})", info.version, info.cipher, ms(*duration)),
                )?;
                let shown = if level >= 2 {
                    info.peer_certificates.len()
                } else {
                    1
                };
                for (i, c) in info.peer_certificates.iter().take(shown).enumerate() {
                    v.certificate(at, i, c)?;
                }
            }
            TraceEvent::RequestHeaders {
                method,
                url,
                version,
                headers,
            } => {
                v.line(at, SENT, '>', &format!("{method} {url} {version}"))?;
                v.headers(at, SENT, '>', headers)?;
            }
            TraceEvent::RequestSent {
                body_bytes,
                duration,
            } => {
                let body = if *body_bytes > 0 {
                    format!(" with {} body", human_bytes(*body_bytes))
                } else {
                    String::new()
                };
                v.info(at, &format!("request sent{body} ({})", ms(*duration)))?;
            }
            TraceEvent::ResponseFirstByte { wait } => {
                v.info(at, &format!("first response byte after {}", ms(*wait)))?;
            }
            TraceEvent::ResponseHeaders {
                status,
                version,
                headers,
            } => {
                let reason = http::StatusCode::from_u16(*status)
                    .ok()
                    .and_then(|s| s.canonical_reason())
                    .unwrap_or("");
                v.line(at, RECEIVED, '<', &format!("{version} {status} {reason}"))?;
                v.headers(at, RECEIVED, '<', headers)?;
            }
            TraceEvent::ResponseComplete {
                body_bytes,
                download,
            } => {
                v.info(
                    at,
                    &format!(
                        "{} body received ({})",
                        human_bytes(*body_bytes),
                        ms(*download)
                    ),
                )?;
            }
            TraceEvent::Wire {
                sent,
                received,
                truncated,
            } => {
                if level >= 2 {
                    v.wire(at, "wire: sent", SENT, '>', sent)?;
                    v.wire(at, "wire: received", RECEIVED, '<', received)?;
                    if *truncated {
                        v.info(at, "wire capture truncated (limit reached)")?;
                    }
                }
            }
            TraceEvent::Redirect { status, to, .. } => {
                v.info(at, &format!("{status} redirect to {to}"))?;
            }
            TraceEvent::Info { message } => v.info(at, message)?,
            TraceEvent::Error { phase, message } => {
                v.line(at, ERROR, '!', &format!("{phase} failed: {message}"))?;
            }
            _ => {}
        }
    }
    if let Some(last) = trace.last() {
        v.info(last.at, &format!("total {}", ms(last.at)))?;
    }
    v.out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapreq_core::model::{HttpVersion, Phase, TlsInfo};

    fn at(ms: u64, event: TraceEvent) -> TraceEntry {
        TraceEntry {
            at: Duration::from_millis(ms),
            event,
        }
    }

    fn sample() -> Vec<TraceEntry> {
        vec![
            at(
                0,
                TraceEvent::DnsStart {
                    host: "api.test".into(),
                },
            ),
            at(
                5,
                TraceEvent::DnsEnd {
                    host: "api.test".into(),
                    addrs: vec!["10.0.0.1".parse().unwrap()],
                    duration: Duration::from_millis(5),
                },
            ),
            at(
                5,
                TraceEvent::ConnectStart {
                    addr: "10.0.0.1:443".parse().unwrap(),
                },
            ),
            at(
                9,
                TraceEvent::ConnectEnd {
                    addr: "10.0.0.1:443".parse().unwrap(),
                    local_addr: "10.0.0.2:5000".parse().unwrap(),
                    duration: Duration::from_millis(4),
                },
            ),
            at(
                20,
                TraceEvent::TlsEnd {
                    info: TlsInfo {
                        version: "TLSv1.3".into(),
                        cipher: "TLS13_AES_128_GCM_SHA256".into(),
                        alpn: Some("http/1.1".into()),
                        peer_certificates: vec![CertificateInfo {
                            subject: "CN=api.test".into(),
                            issuer: "CN=Test CA".into(),
                            not_before: "2026-01-01T00:00:00Z".into(),
                            not_after: "2027-01-01T00:00:00Z".into(),
                            serial: "01".into(),
                            subject_alt_names: vec!["api.test".into()],
                            sha256_fingerprint: "AA:BB".into(),
                        }],
                    },
                    duration: Duration::from_millis(11),
                },
            ),
            at(
                20,
                TraceEvent::RequestHeaders {
                    method: "GET".into(),
                    url: "https://api.test/".into(),
                    version: HttpVersion::Http11,
                    headers: vec![Header::new("Host", "api.test")],
                },
            ),
            at(
                21,
                TraceEvent::RequestSent {
                    body_bytes: 0,
                    duration: Duration::from_micros(500),
                },
            ),
            at(
                30,
                TraceEvent::ResponseFirstByte {
                    wait: Duration::from_millis(9),
                },
            ),
            at(
                30,
                TraceEvent::ResponseHeaders {
                    status: 200,
                    version: HttpVersion::Http11,
                    headers: vec![Header::new("content-length", "2")],
                },
            ),
            at(
                31,
                TraceEvent::ResponseComplete {
                    body_bytes: 2,
                    download: Duration::from_millis(1),
                },
            ),
            at(
                31,
                TraceEvent::Wire {
                    sent: bytes::Bytes::from_static(b"GET / HTTP/1.1\r\nHost: api.test\r\n\r\n"),
                    received: bytes::Bytes::from_static(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok",
                    ),
                    truncated: false,
                },
            ),
        ]
    }

    fn plain(trace: &[TraceEntry], level: u8) -> String {
        let mut buf = Vec::new();
        {
            let mut out = anstream::StripStream::new(&mut buf);
            render(&mut out, trace, level).unwrap();
        }
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn single_v_is_compact() {
        assert_eq!(
            plain(&sample(), 1),
            "\
* resolved api.test → 10.0.0.1 (5.00 ms)
* connected to 10.0.0.1:443 from 10.0.0.2:5000 (4.00 ms)
* TLSv1.3 TLS13_AES_128_GCM_SHA256, ALPN http/1.1 (11.0 ms)
*   server certificate: CN=api.test
*     valid until 2027-01-01T00:00:00Z
> GET https://api.test/ HTTP/1.1
> Host: api.test
* request sent (0.50 ms)
* first response byte after 9.00 ms
< HTTP/1.1 200 OK
< content-length: 2
* 2 B body received (1.00 ms)
* total 31.0 ms
"
        );
    }

    #[test]
    fn double_v_adds_timestamps_certificates_and_wire() {
        let text = plain(&sample(), 2);
        assert!(
            text.starts_with("[    0.000] * resolving api.test\n"),
            "{text}"
        );
        assert!(text.contains("*     sha256: AA:BB\n"), "{text}");
        assert!(
            text.contains("[   31.000] * wire: sent (34 B)\n[   31.000] > GET / HTTP/1.1\n"),
            "{text}"
        );
        assert!(text.contains("< ok\n"), "{text}");
    }

    #[test]
    fn errors_are_marked() {
        let trace = vec![at(
            3,
            TraceEvent::Error {
                phase: Phase::Connect,
                message: "connection refused".into(),
            },
        )];
        assert_eq!(
            plain(&trace, 1),
            "! TCP connect failed: connection refused\n* total 3.00 ms\n"
        );
    }
}
