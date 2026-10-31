//! Serde helpers shared by the model types.
//!
//! Durations are serialized as floating-point **milliseconds** with microsecond precision, which is
//! what people expect to read in JSON output and what the web UI plots.

/// `http::Method` as its string form (`"GET"`, `"PURGE"`, ...).
pub(crate) mod method {
    use http::Method;
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub(crate) fn serialize<S: Serializer>(method: &Method, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(method.as_str())
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Method, D::Error> {
        let raw = String::deserialize(d)?;
        Method::from_bytes(raw.as_bytes())
            .map_err(|_| D::Error::custom(format!("invalid HTTP method `{raw}`")))
    }
}

fn to_ms(d: std::time::Duration) -> f64 {
    // Microsecond precision keeps the value exact when read back.
    d.as_micros() as f64 / 1000.0
}

fn from_ms<E: serde::de::Error>(ms: f64) -> Result<std::time::Duration, E> {
    if !ms.is_finite() || ms < 0.0 {
        return Err(E::custom(format!("invalid duration `{ms}` ms")));
    }
    Ok(std::time::Duration::from_micros(
        (ms * 1000.0).round() as u64
    ))
}

/// `Duration` as milliseconds (`f64`).
pub(crate) mod duration_ms {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub(crate) fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(super::to_ms(*d))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        super::from_ms(f64::deserialize(d)?)
    }
}

/// `Option<Duration>` as milliseconds (`f64`) or `null`.
pub(crate) mod opt_duration_ms {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub(crate) fn serialize<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match d {
            Some(d) => s.serialize_some(&super::to_ms(*d)),
            None => s.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Duration>, D::Error> {
        Option::<f64>::deserialize(d)?
            .map(super::from_ms)
            .transpose()
    }
}

/// Raw bytes as `{ "encoding": "utf8", "data": "..." }` when they are valid UTF-8,
/// otherwise `{ "encoding": "base64", "data": "..." }`.
pub(crate) mod raw_bytes {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Encoding {
        Utf8,
        Base64,
    }

    #[derive(Serialize, Deserialize)]
    struct Encoded<'a> {
        encoding: Encoding,
        #[serde(borrow)]
        data: std::borrow::Cow<'a, str>,
    }

    pub(crate) fn serialize<S: Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        let encoded = match std::str::from_utf8(b) {
            Ok(text) => Encoded {
                encoding: Encoding::Utf8,
                data: text.into(),
            },
            Err(_) => Encoded {
                encoding: Encoding::Base64,
                data: STANDARD.encode(b).into(),
            },
        };
        encoded.serialize(s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let encoded = Encoded::deserialize(d)?;
        match encoded.encoding {
            Encoding::Utf8 => Ok(Bytes::from(encoded.data.into_owned())),
            Encoding::Base64 => STANDARD
                .decode(encoded.data.as_bytes())
                .map(Bytes::from)
                .map_err(D::Error::custom),
        }
    }
}
