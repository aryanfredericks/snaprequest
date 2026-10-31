//! The data model shared by every SnapRequest front end: what to send ([`Request`]),
//! what came back ([`Response`]) and everything that happened in between ([`TraceEntry`]).
//!
//! All types serialize to stable JSON; this is the format of `--output json`,
//! the server API and stored history.

mod body;
mod request;
mod response;
mod settings;
mod trace;

pub(crate) use body::guess_content_type;
pub use body::{Body, KeyValue, MultipartPart, PartValue};
pub use request::Request;
pub use response::{Header, HttpVersion, Response, Sizes, Timings};
pub use settings::{HttpVersionPref, RequestSettings, ResolveOverride};
pub use trace::{CertificateInfo, Phase, TlsInfo, TraceEntry, TraceEvent};
