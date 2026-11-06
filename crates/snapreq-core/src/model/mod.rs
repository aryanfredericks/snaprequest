//! The data model shared by every SnapRequest front end: what to send ([`Request`]),
//! what came back ([`Response`]) and everything that happened in between ([`TraceEntry`]).
//!
//! All types serialize to stable JSON; this is the format of `--output json`,
//! the server API and stored history.

pub(crate) mod auth;
mod body;
mod request;
mod response;
mod settings;
mod trace;

pub use auth::{ApiKeyLocation, Auth, ClientAuth, OAuth2, OAuth2Grant};
pub(crate) use body::guess_content_type;
pub use body::{Body, KeyValue, MultipartPart, PartValue, graphql_document};
pub use request::Request;
pub use response::{Header, HttpVersion, Response, Sizes, Timings};
pub use settings::{
    ClientCertificate, HttpVersionPref, ProxyMode, RequestSettings, ResolveOverride, TlsSettings,
    TlsVersion,
};
pub use trace::{CertificateInfo, Phase, TlsInfo, TraceEntry, TraceEvent};
