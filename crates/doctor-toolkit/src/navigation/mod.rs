//! Navigation across a whole game: an index of every resource with its names
//! and references, a dictionary of hashed names, global search, and
//! `unpack` / `pack` of resources in common file formats.
//!
//! # The game index
//!
//! [`GameIndex`] holds one [`Record`] per resource of every stream: its key,
//! size, the names that can be read from its data (texture and package names,
//! material and model names, SimGroup object names and classes, rig, font and
//! sound names, …) and its outgoing references (a material's textures, a
//! mesh's materials and skeleton, a metamodel's meshes and collision, a
//! SimGroup object's metamodel, …). References are resolved to the records
//! they point at; the reverse direction gives "used by" for any resource.
//!
//! Building the index reads and parses about 1.2 GB of resource data; the
//! result is cached on disk per stream, keyed by the size, modification time
//! and inode of the stream file, so later opens only re-read the streams that
//! changed (after `workspace apply`, for example). See `docs/navigation.md`.

mod cache;
pub mod dictionary;
mod harvest;
pub mod index;
pub mod placement;
pub mod search;
pub mod unpack;

#[cfg(test)]
mod tests;

pub use dictionary::{Dictionary, HashFn, Origin};
pub use index::{
    Anchor, GameIndex, HashKind, NameKind, OpenReport, Record, RecordId, Ref, ResolveStats, Target,
    TextEntry,
};
pub use placement::Placement;
pub use search::{Hit, Query, SearchResult};
