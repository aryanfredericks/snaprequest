//! SnapRequest collections: the on-disk file format (`docs/FORMAT.md`), loading and saving.
//! Variables, environments and turning collection items into requests build on this.

pub mod assert;
pub mod build;
pub mod capture;
pub mod error;
pub mod files;
pub mod flow;
pub mod interp;
pub mod kv;
pub mod load;
pub mod save;
pub mod sealed;
pub mod secrets;
pub mod vars;

pub use assert::{AssertSpec, Check, Checks, JsonType};
pub use build::{
    GrpcCall, build_assertions, build_file, build_request, effective_auth, grpc_call,
    interpolate_assertions, websocket_messages,
};
pub use capture::{CaptureSource, CaptureSpec, Captures};
pub use error::{Error, Result, Warning};
pub use files::{
    AuthSpec, BodySpec, CollectionFile, FORMAT_VERSION, FolderFile, GrpcSpec, OAuth2Spec, PartSpec,
    RequestFile, RequestKind, ScriptSpec, Settings, WebSocketSpec, WsMessageKind, WsMessageSpec,
};
pub use flow::{FLOW_SUFFIX, FlowFile, RetrySpec, StepSpec};
pub use kv::KeyValues;
pub use load::{Collection, FlowItem, Folder, Item, RequestItem};
pub use secrets::Secrets;
pub use vars::{Environment, Scope, Scopes, Variable, Variables};
