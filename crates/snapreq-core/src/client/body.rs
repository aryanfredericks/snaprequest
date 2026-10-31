//! Request body encoding and streaming.
//!
//! A body is prepared once into a list of [`Segment`]s (in-memory bytes or a file
//! range) with an exact total length. It can then be turned into a fresh streaming
//! hyper body any number of times, which the stale-connection retry relies on.
//! Files are read in chunks while sending and never loaded fully into memory.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt, stream};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use crate::model::{Body, PartValue, guess_content_type};
use crate::{Error, Result};

/// The body type sent through hyper.
pub(crate) type ReqBody = UnsyncBoxBody<Bytes, io::Error>;

/// Read size for streaming files.
const FILE_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone)]
enum Segment {
    Bytes(Bytes),
    File { path: PathBuf, len: u64 },
}

impl Segment {
    fn len(&self) -> u64 {
        match self {
            Self::Bytes(b) => b.len() as u64,
            Self::File { len, .. } => *len,
        }
    }
}

/// An encoded body: what to send, how long it is and its content type.
#[derive(Debug, Clone, Default)]
pub(crate) struct EncodedBody {
    segments: Vec<Segment>,
    pub len: u64,
    /// Content type implied by the body (multipart includes its boundary).
    pub content_type: Option<String>,
    /// Multipart boundary, so a user-supplied `Content-Type` can be completed.
    pub boundary: Option<String>,
}

impl EncodedBody {
    fn from_segments(segments: Vec<Segment>, content_type: Option<String>) -> Self {
        let len = segments.iter().map(Segment::len).sum();
        Self {
            segments,
            len,
            content_type,
            boundary: None,
        }
    }

    /// Builds a fresh streaming body. Files are opened only when the body is polled.
    pub(crate) fn to_body(&self) -> ReqBody {
        match self.segments.as_slice() {
            [] => Full::new(Bytes::new())
                .map_err(|never| match never {})
                .boxed_unsync(),
            [Segment::Bytes(b)] => Full::new(b.clone())
                .map_err(|never| match never {})
                .boxed_unsync(),
            segments => {
                let chunks = stream::iter(segments.to_vec())
                    .flat_map(segment_stream)
                    .map_ok(Frame::data);
                StreamBody::new(chunks).boxed_unsync()
            }
        }
    }
}

fn segment_stream(seg: Segment) -> stream::BoxStream<'static, io::Result<Bytes>> {
    match seg {
        Segment::Bytes(b) => stream::once(async move { Ok(b) }).boxed(),
        Segment::File { path, len } => {
            let sent = Arc::new(AtomicU64::new(0));
            let counter = sent.clone();
            let shown = path.display().to_string();
            let open = async move {
                let file = tokio::fs::File::open(&path).await.map_err(|e| {
                    io::Error::new(e.kind(), format!("cannot open `{}`: {e}", path.display()))
                })?;
                // `take` stops at the size measured when preparing, so the body
                // never exceeds the Content-Length we announced.
                Ok::<_, io::Error>(ReaderStream::with_capacity(file.take(len), FILE_CHUNK))
            };
            let data = stream::once(open).try_flatten().inspect_ok(move |chunk| {
                counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            });
            let check = stream::once(async move {
                let got = sent.load(Ordering::Relaxed);
                if got < len {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("`{shown}` shrank while being sent ({got} of {len} bytes)"),
                    ))
                } else {
                    Ok(Bytes::new())
                }
            });
            data.chain(check)
                .try_filter(|chunk| std::future::ready(!chunk.is_empty()))
                .boxed()
        }
    }
}

/// Encodes a model body. Reads file metadata (not contents) to know exact sizes.
pub(crate) async fn encode(body: &Body) -> Result<EncodedBody> {
    let content_type = body.default_content_type().map(str::to_owned);
    Ok(match body {
        Body::None => EncodedBody::default(),
        Body::Text { content, .. } | Body::Json { content } => EncodedBody::from_segments(
            vec![Segment::Bytes(Bytes::from(content.clone()))],
            content_type,
        ),
        Body::Form { fields } => {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for f in fields.iter().filter(|f| f.enabled) {
                ser.append_pair(&f.name, &f.value);
            }
            EncodedBody::from_segments(
                vec![Segment::Bytes(Bytes::from(ser.finish()))],
                content_type,
            )
        }
        Body::File { path, .. } => {
            let len = file_len(path).await?;
            EncodedBody::from_segments(
                vec![Segment::File {
                    path: path.clone(),
                    len,
                }],
                content_type,
            )
        }
        Body::Multipart { parts } => encode_multipart(parts, &new_boundary()).await?,
    })
}

async fn encode_multipart(
    parts: &[crate::model::MultipartPart],
    boundary: &str,
) -> Result<EncodedBody> {
    let mut segments = Vec::new();
    let mut head = String::new();

    for part in parts.iter().filter(|p| p.enabled) {
        head.push_str(&format!("--{boundary}\r\n"));
        head.push_str(&format!(
            "Content-Disposition: form-data; name=\"{}\"",
            escape_quoted(&part.name)
        ));
        match &part.value {
            PartValue::Text { value } => {
                head.push_str("\r\n\r\n");
                head.push_str(value);
                head.push_str("\r\n");
            }
            PartValue::File {
                path,
                filename,
                content_type,
            } => {
                let filename = filename.clone().unwrap_or_else(|| {
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
                let ct = content_type
                    .as_deref()
                    .unwrap_or_else(|| guess_content_type(path));
                head.push_str(&format!(
                    "; filename=\"{}\"\r\nContent-Type: {ct}\r\n\r\n",
                    escape_quoted(&filename)
                ));
                segments.push(Segment::Bytes(Bytes::from(std::mem::take(&mut head))));
                segments.push(Segment::File {
                    path: path.clone(),
                    len: file_len(path).await?,
                });
                head.push_str("\r\n");
            }
        }
    }
    head.push_str(&format!("--{boundary}--\r\n"));
    segments.push(Segment::Bytes(Bytes::from(head)));

    let mut encoded = EncodedBody::from_segments(
        segments,
        Some(format!("multipart/form-data; boundary={boundary}")),
    );
    encoded.boundary = Some(boundary.to_owned());
    Ok(encoded)
}

/// Escapes a multipart name or filename the way browsers do (WHATWG HTML spec).
fn escape_quoted(s: &str) -> String {
    s.replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// A boundary that is unique per body and practically never appears in content.
/// It does not need to be secret, only unlikely to collide.
fn new_boundary() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed =
        nanos ^ (u64::from(std::process::id()) << 32) ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    format!("----snapreq-{mixed:016x}{n:04x}")
}

async fn file_len(path: &Path) -> Result<u64> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| {
        Error::InvalidRequest(format!("cannot read file `{}`: {e}", path.display()))
    })?;
    if !meta.is_file() {
        return Err(Error::InvalidRequest(format!(
            "`{}` is not a regular file",
            path.display()
        )));
    }
    Ok(meta.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{KeyValue, MultipartPart};

    async fn collect(body: &EncodedBody) -> io::Result<Vec<u8>> {
        Ok(body.to_body().collect().await?.to_bytes().to_vec())
    }

    fn temp_file(name: &str, contents: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snapreq-body-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[tokio::test]
    async fn text_and_form_bodies() {
        let text = encode(&Body::Text {
            content: "hi".into(),
            content_type: None,
        })
        .await
        .unwrap();
        assert_eq!(text.len, 2);
        assert_eq!(
            text.content_type.as_deref(),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(collect(&text).await.unwrap(), b"hi");

        let form = encode(&Body::Form {
            fields: vec![
                KeyValue::new("q", "a b&c"),
                KeyValue::new("off", "x").disabled(),
                KeyValue::new("n", "1"),
            ],
        })
        .await
        .unwrap();
        assert_eq!(collect(&form).await.unwrap(), b"q=a+b%26c&n=1");
    }

    #[tokio::test]
    async fn file_body_streams_the_whole_file_and_can_be_rebuilt() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let path = temp_file("big.bin", &data);
        let body = encode(&Body::File {
            path: path.clone(),
            content_type: None,
        })
        .await
        .unwrap();

        assert_eq!(body.len, data.len() as u64);
        assert_eq!(
            body.content_type.as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(collect(&body).await.unwrap(), data);
        // A second body from the same encoding re-reads the file (used by retries).
        assert_eq!(collect(&body).await.unwrap(), data);
    }

    #[tokio::test]
    async fn missing_file_is_an_error_before_sending() {
        let err = encode(&Body::File {
            path: "/definitely/not/here.bin".into(),
            content_type: None,
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("cannot read file"), "{err}");

        let err = encode(&Body::File {
            path: std::env::temp_dir(),
            content_type: None,
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not a regular file"), "{err}");
    }

    #[tokio::test]
    async fn file_that_shrinks_after_preparing_fails() {
        let path = temp_file("shrinks.txt", b"0123456789");
        let body = encode(&Body::File {
            path: path.clone(),
            content_type: None,
        })
        .await
        .unwrap();
        std::fs::write(&path, b"0123").unwrap();

        let err = collect(&body).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("shrank while being sent (4 of 10 bytes)"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn file_that_grows_is_truncated_to_announced_length() {
        let path = temp_file("grows.txt", b"0123");
        let body = encode(&Body::File {
            path: path.clone(),
            content_type: None,
        })
        .await
        .unwrap();
        std::fs::write(&path, b"0123456789").unwrap();
        assert_eq!(collect(&body).await.unwrap(), b"0123");
    }

    #[tokio::test]
    async fn multipart_layout() {
        let path = temp_file("photo.png", b"PNGDATA");
        let parts = vec![
            MultipartPart {
                name: "title".into(),
                value: PartValue::Text {
                    value: "Hello \"world\"".into(),
                },
                enabled: true,
            },
            MultipartPart {
                name: "skip".into(),
                value: PartValue::Text { value: "x".into() },
                enabled: false,
            },
            MultipartPart {
                name: "photo".into(),
                value: PartValue::File {
                    path,
                    filename: None,
                    content_type: None,
                },
                enabled: true,
            },
        ];
        let body = encode_multipart(&parts, "BOUNDARY").await.unwrap();
        let bytes = collect(&body).await.unwrap();
        let expected = "--BOUNDARY\r\n\
            Content-Disposition: form-data; name=\"title\"\r\n\r\n\
            Hello \"world\"\r\n\
            --BOUNDARY\r\n\
            Content-Disposition: form-data; name=\"photo\"; filename=\"photo.png\"\r\n\
            Content-Type: image/png\r\n\r\n\
            PNGDATA\r\n\
            --BOUNDARY--\r\n";
        assert_eq!(String::from_utf8(bytes.clone()).unwrap(), expected);
        assert_eq!(body.len, bytes.len() as u64);
        assert_eq!(
            body.content_type.as_deref(),
            Some("multipart/form-data; boundary=BOUNDARY")
        );
    }

    #[test]
    fn names_are_escaped_and_boundaries_unique() {
        assert_eq!(escape_quoted("a\"b\r\nc"), "a%22b%0D%0Ac");
        assert_ne!(new_boundary(), new_boundary());
    }
}
