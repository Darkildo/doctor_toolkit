//! Autodesk FBX: reading binary (7.x, 32- and 64-bit offsets,
//! zlib-compressed arrays) and ASCII 7.x files, writing binary 7.4.
//!
//! [`node`] is the generic node tree, [`read`](read::read) maps it to a
//! [`Scene`], [`write`](write::write) builds it from one.

pub mod node;
mod read;
mod write;

pub use read::KTIME;
pub use write::VERSION;

use super::{Scene, SideFiles};
use crate::Result;

/// Reads a binary or ASCII FBX file.
pub fn read(bytes: &[u8], files: &SideFiles) -> Result<Scene> {
    let document = node::parse(bytes)?;
    read::read(&document, files)
}

/// Writes a scene as binary FBX 7.4.
pub fn write(scene: &Scene) -> Result<Vec<u8>> {
    write::write(scene)
}
