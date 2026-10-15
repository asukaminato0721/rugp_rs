//! Native readers for rUGP's serialized objects and split RIO archives.
pub mod archive;
pub mod binary;
pub mod crmt;
pub mod crypto;
pub mod ici;
pub mod image;
pub mod native_schema;
pub mod object;
pub mod project;
pub mod rip;
mod rip_quant;
pub mod script;

pub use archive::RioArchive;
pub use ici::InstallManifest;
pub use object::{ClassId, ObjectNode};
