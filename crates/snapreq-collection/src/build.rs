//! Turning a collection request into a core [`snapreq_core::Request`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use snapreq_core::Request;
use snapreq_core::model::{
    Body, ClientCertificate, KeyValue, MultipartPart, PartValue, RequestSettings,
};

use crate::error::{Error, Result};
use crate::files::{BodySpec, Settings};
use crate::interp::Interpolator;
use crate::kv::KeyValues;
use crate::load::{COLLECTION_FILE, Collection, FOLDER_FILE, RequestItem};
use crate::vars::Scopes;

/// Builds the request at `key_path` (for example `users/create-user`) with every
/// placeholder resolved from `scopes`.
///
/// Headers and settings are inherited from the collection, then each enclosing folder
/// (outermost first), then the request itself:
/// - a header replaces every inherited header with the same name (case-insensitive);
///   a disabled one switches the inherited header off;
/// - a setting overrides the inherited value; relative paths resolve against the
///   file that set them.
///
/// Values of secret variables that were used end up in `settings.secrets`, so they are
/// hidden in traces.
pub fn build_request(collection: &Collection, key_path: &str, scopes: &Scopes) -> Result<Request> {
    let item = collection.find(key_path).ok_or_else(|| {
        Error::invalid(
            &collection.root,
            format!("no request `{key_path}` in the collection"),
        )
    })?;

    let root = &collection.root;
    let mut layers: Vec<(PathBuf, PathBuf, &KeyValues, &Settings)> = vec![(
        root.join(COLLECTION_FILE),
        root.clone(),
        &collection.file.headers,
        &collection.file.settings,
    )];
    for folder in collection.folders_of(key_path) {
        let dir = root.join(&folder.path);
        layers.push((
            dir.join(FOLDER_FILE),
            dir,
            &folder.file.headers,
            &folder.file.settings,
        ));
    }

    let mut b = Builder {
        interp: Interpolator::new(scopes),
        file: PathBuf::new(),
        base_dir: PathBuf::new(),
    };
    let mut headers = Vec::new();
    let mut settings = RequestSettings::default();
    for (file, dir, layer_headers, layer_settings) in layers {
        b.file = file;
        b.base_dir = dir;
        let layer = b.key_values("headers", layer_headers)?;
        merge_headers(&mut headers, layer);
        settings = b.settings(layer_settings, settings)?;
    }

    let file_path = root.join(&item.path);
    b.base_dir = file_path.parent().unwrap_or(root).to_path_buf();
    b.file = file_path;
    let mut req = b.request(item, headers, settings)?;
    req.settings.secrets = b.interp.into_secrets();
    Ok(req)
}

/// Appends `layer` to `inherited`, first removing inherited headers that `layer`
/// names again (case-insensitive).
fn merge_headers(inherited: &mut Vec<KeyValue>, layer: Vec<KeyValue>) {
    inherited.retain(|h| !layer.iter().any(|l| l.name.eq_ignore_ascii_case(&h.name)));
    inherited.extend(layer);
}

struct Builder<'a> {
    interp: Interpolator<'a>,
    /// The file being processed, for error messages.
    file: PathBuf,
    /// Relative paths are resolved against this directory.
    base_dir: PathBuf,
}

impl Builder<'_> {
    /// Interpolates `text`; `field` names the place for error messages.
    fn text(&mut self, field: &str, text: &str) -> Result<String> {
        self.interp
            .interpolate(text)
            .map_err(|e| Error::invalid(&self.file, format!("{field}: {e}")))
    }

    fn path(&mut self, field: &str, path: &Path) -> Result<PathBuf> {
        let resolved = PathBuf::from(self.text(field, &path.to_string_lossy())?);
        Ok(if resolved.is_absolute() {
            resolved
        } else {
            self.base_dir.join(resolved)
        })
    }

    /// Enabled entries are interpolated; disabled ones are kept as written.
    fn key_values(&mut self, field: &str, kv: &KeyValues) -> Result<Vec<KeyValue>> {
        kv.0.iter()
            .map(|e| {
                if !e.enabled {
                    return Ok(e.clone());
                }
                let place = format!("{field}.{}", e.name);
                Ok(KeyValue {
                    name: self.text(&place, &e.name)?,
                    value: self.text(&place, &e.value)?,
                    enabled: true,
                })
            })
            .collect()
    }

    fn request(
        &mut self,
        item: &RequestItem,
        mut headers: Vec<KeyValue>,
        settings: RequestSettings,
    ) -> Result<Request> {
        let f = &item.file;
        let method = http::Method::from_bytes(f.method().as_bytes())
            .map_err(|_| Error::invalid(&self.file, format!("invalid method `{}`", f.method())))?;
        let mut req = Request::new(method, self.text("url", &f.url)?);
        req.query = self.key_values("query", &f.query)?;
        let own = self.key_values("headers", &f.headers)?;
        merge_headers(&mut headers, own);
        req.headers = headers;
        if let Some(body) = &f.body {
            req.body = self.body(body)?;
        }
        req.settings = self.settings(&f.settings, settings)?;
        Ok(req)
    }

    fn body(&mut self, body: &BodySpec) -> Result<Body> {
        Ok(match body {
            BodySpec::Text {
                content: Some(c),
                content_type,
                ..
            } => Body::Text {
                content: self.text("body.content", c)?,
                content_type: content_type.clone(),
            },
            BodySpec::Text {
                file: Some(p),
                content_type,
                ..
            } => Body::File {
                path: self.path("body.file", p)?,
                content_type: Some(
                    content_type
                        .clone()
                        .unwrap_or_else(|| "text/plain; charset=utf-8".into()),
                ),
            },
            BodySpec::Json {
                content: Some(c), ..
            } => Body::Json {
                content: self.text("body.content", c)?,
            },
            BodySpec::Json { file: Some(p), .. } => Body::File {
                path: self.path("body.file", p)?,
                content_type: Some("application/json".into()),
            },
            // `validate()` guarantees `content` or `file` for text and JSON bodies.
            BodySpec::Text { .. } | BodySpec::Json { .. } => Body::None,
            BodySpec::Form { fields } => Body::Form {
                fields: self.key_values("body.fields", fields)?,
            },
            BodySpec::Multipart { parts } => {
                let mut out = Vec::with_capacity(parts.len());
                for p in parts {
                    let place = format!("body.parts.{}", p.name);
                    let value = match (&p.value, &p.file) {
                        (Some(v), _) => PartValue::Text {
                            value: if p.enabled {
                                self.text(&place, v)?
                            } else {
                                v.clone()
                            },
                        },
                        (None, Some(path)) => PartValue::File {
                            path: if p.enabled {
                                self.path(&place, path)?
                            } else {
                                path.clone()
                            },
                            filename: p.filename.clone(),
                            content_type: p.content_type.clone(),
                        },
                        (None, None) => continue,
                    };
                    out.push(MultipartPart {
                        name: p.name.clone(),
                        value,
                        enabled: p.enabled,
                    });
                }
                Body::Multipart { parts: out }
            }
            BodySpec::File { file, content_type } => Body::File {
                path: self.path("body.file", file)?,
                content_type: content_type.clone(),
            },
        })
    }

    /// Applies the keys set in `s` on top of `base`.
    fn settings(&mut self, s: &Settings, mut base: RequestSettings) -> Result<RequestSettings> {
        let ms = |v: u64| (v > 0).then(|| Duration::from_millis(v));
        if let Some(v) = s.timeout_ms {
            base.timeout = ms(v);
        }
        if let Some(v) = s.connect_timeout_ms {
            base.connect_timeout = ms(v);
        }
        if let Some(v) = s.follow_redirects {
            base.follow_redirects = v;
        }
        if let Some(v) = s.max_redirects {
            base.max_redirects = v;
        }
        if let Some(v) = s.decompress {
            base.decompress = v;
        }
        if let Some(v) = s.cookies {
            base.cookies = v;
        }
        if let Some(v) = s.http_version {
            base.http_version = v;
        }
        if let Some(p) = &s.proxy {
            base.proxy = match p {
                snapreq_core::model::ProxyMode::Url(u) => {
                    snapreq_core::model::ProxyMode::try_from(self.text("settings.proxy", u)?)
                        .map_err(|e| Error::invalid(&self.file, format!("settings.proxy: {e}")))?
                }
                other => other.clone(),
            };
        }
        if let Some(v) = s.verify_tls {
            base.tls.verify = v;
        }
        if let Some(p) = &s.ca_cert {
            base.tls.ca_cert = Some(self.path("settings.ca_cert", p)?);
        }
        if let (Some(cert), Some(key)) = (&s.client_cert, &s.client_key) {
            base.tls.client_cert = Some(ClientCertificate {
                cert: self.path("settings.client_cert", cert)?,
                key: self.path("settings.client_key", key)?,
            });
        }
        if let Some(v) = s.tls_min_version {
            base.tls.min_version = Some(v);
        }
        if let Some(v) = s.tls_max_version {
            base.tls.max_version = Some(v);
        }
        Ok(base)
    }
}
