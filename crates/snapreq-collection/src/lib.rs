//! SnapRequest collections: the on-disk file format (`docs/FORMAT.md`), loading and saving.
//! Variables, environments and turning collection items into requests build on this.

pub mod error;
pub mod files;
pub mod kv;
pub mod load;
pub mod save;

pub use error::{Error, Result, Warning};
pub use files::{
    BodySpec, CollectionFile, FORMAT_VERSION, FolderFile, PartSpec, RequestFile, Settings,
};
pub use kv::KeyValues;
pub use load::{Collection, Folder, Item, RequestItem};
