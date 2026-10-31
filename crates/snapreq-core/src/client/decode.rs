//! Response body decompression (`Content-Encoding`).
//!
//! Decoding never fails the request: when an encoding is unknown or the data is
//! corrupt, the raw body is kept and the reason is reported, because seeing what
//! actually arrived is more useful when debugging than an error.

use std::io::Read;

use bytes::Bytes;

/// Value sent in `Accept-Encoding` when decompression is enabled.
pub(crate) const ACCEPT_ENCODING: &str = "gzip, deflate, br, zstd";

/// Upper bound for a decoded body, to survive decompression bombs.
const MAX_DECODED: u64 = 1 << 30;

/// Result of decoding a body.
#[derive(Debug)]
pub(crate) enum DecodeOutcome {
    /// No `Content-Encoding` (or `identity`): body unchanged.
    Identity(Bytes),
    /// Successfully decoded through the listed encodings (in the order applied by the server).
    Decoded { body: Bytes, encodings: Vec<String> },
    /// Kept the raw body; the string explains why.
    Raw { body: Bytes, reason: String },
}

/// Decodes `body` according to the `Content-Encoding` header values.
///
/// Encodings are listed in the order the server applied them, so they are undone
/// in reverse.
pub(crate) fn decode(body: Bytes, content_encoding: &[&str]) -> DecodeOutcome {
    let encodings: Vec<String> = content_encoding
        .iter()
        .flat_map(|v| v.split(','))
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e != "identity")
        .collect();
    if encodings.is_empty() || body.is_empty() {
        return DecodeOutcome::Identity(body);
    }

    let mut data = body.clone();
    for encoding in encodings.iter().rev() {
        match decode_one(encoding, &data) {
            Ok(decoded) => data = decoded,
            Err(reason) => {
                return DecodeOutcome::Raw {
                    body,
                    reason: format!("could not decode `{encoding}` body: {reason}"),
                };
            }
        }
    }
    DecodeOutcome::Decoded {
        body: data,
        encodings,
    }
}

fn decode_one(encoding: &str, data: &[u8]) -> Result<Bytes, String> {
    match encoding {
        "gzip" | "x-gzip" => read_all(flate2::read::MultiGzDecoder::new(data)),
        // HTTP "deflate" is zlib-wrapped (RFC 9110), but some servers send raw deflate.
        "deflate" => read_all(flate2::read::ZlibDecoder::new(data))
            .or_else(|_| read_all(flate2::read::DeflateDecoder::new(data))),
        "br" => read_all(brotli_decompressor::Decompressor::new(data, 64 * 1024)),
        "zstd" => {
            let decoder =
                ruzstd::decoding::StreamingDecoder::new(data).map_err(|e| e.to_string())?;
            read_all(decoder)
        }
        other => Err(format!("unsupported encoding `{other}`")),
    }
}

fn read_all(reader: impl Read) -> Result<Bytes, String> {
    let mut out = Vec::new();
    reader
        .take(MAX_DECODED + 1)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > MAX_DECODED {
        return Err(format!("decoded body exceeds {} MiB", MAX_DECODED >> 20));
    }
    Ok(Bytes::from(out))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub(crate) fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn raw_deflate(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn brotli(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
            w.write_all(data).unwrap();
        }
        out
    }

    fn zstd(data: &[u8]) -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
    }

    const TEXT: &[u8] =
        b"SnapRequest decodes compressed bodies. SnapRequest decodes compressed bodies.";

    fn decoded(body: Vec<u8>, ce: &[&str]) -> (Bytes, Vec<String>) {
        match decode(Bytes::from(body), ce) {
            DecodeOutcome::Decoded { body, encodings } => (body, encodings),
            other => panic!("expected decoded body, got {other:?}"),
        }
    }

    #[test]
    fn every_supported_encoding() {
        for (name, data) in [
            ("gzip", gzip(TEXT)),
            ("deflate", zlib(TEXT)),
            ("br", brotli(TEXT)),
            ("zstd", zstd(TEXT)),
        ] {
            let (body, encodings) = decoded(data, &[name]);
            assert_eq!(&body[..], TEXT, "{name}");
            assert_eq!(encodings, [name]);
        }
    }

    #[test]
    fn raw_deflate_is_accepted() {
        assert_eq!(&decoded(raw_deflate(TEXT), &["deflate"]).0[..], TEXT);
    }

    #[test]
    fn stacked_encodings_are_undone_in_reverse() {
        let twice = brotli(&gzip(TEXT));
        let (body, encodings) = decoded(twice, &["gzip, br"]);
        assert_eq!(&body[..], TEXT);
        assert_eq!(encodings, ["gzip", "br"]);
    }

    #[test]
    fn identity_and_empty_bodies_are_untouched() {
        assert!(matches!(
            decode(Bytes::from_static(TEXT), &[]),
            DecodeOutcome::Identity(_)
        ));
        assert!(matches!(
            decode(Bytes::from_static(TEXT), &["identity"]),
            DecodeOutcome::Identity(_)
        ));
        assert!(matches!(
            decode(Bytes::new(), &["gzip"]),
            DecodeOutcome::Identity(_)
        ));
    }

    #[test]
    fn corrupt_or_unknown_encodings_keep_the_raw_body() {
        match decode(Bytes::from_static(b"not gzip"), &["gzip"]) {
            DecodeOutcome::Raw { body, reason } => {
                assert_eq!(&body[..], b"not gzip");
                assert!(
                    reason.starts_with("could not decode `gzip` body"),
                    "{reason}"
                );
            }
            other => panic!("{other:?}"),
        }
        match decode(Bytes::from_static(b"x"), &["compress"]) {
            DecodeOutcome::Raw { reason, .. } => assert!(reason.contains("unsupported encoding")),
            other => panic!("{other:?}"),
        }
    }
}
