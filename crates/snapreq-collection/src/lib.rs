//! SnapRequest collections: the on-disk file format (`docs/FORMAT.md`), loading and saving.
//! Variables, environments and turning collection items into requests build on this.

pub mod assert;
pub mod build;
pub mod error;
pub mod files;
pub mod interp;
pub mod kv;
pub mod load;
pub mod save;
pub mod secrets;
pub mod vars;

pub use assert::{AssertSpec, Check, Checks, JsonType};
pub use build::{build_assertions, build_request, effective_auth};
pub use error::{Error, Result, Warning};
pub use files::{
    AuthSpec, BodySpec, CollectionFile, FORMAT_VERSION, FolderFile, OAuth2Spec, PartSpec,
    RequestFile, Settings,
};
pub use kv::KeyValues;
pub use load::{Collection, Folder, Item, RequestItem};
pub use secrets::Secrets;
pub use vars::{Environment, Scope, Scopes, Variable, Variables};
