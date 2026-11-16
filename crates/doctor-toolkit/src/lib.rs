//! Reader and writer for the game file formats of the EARS engine
//! (EA Redwood Shores / Visceral Games).
//!
//! The crate is organised in four layers; dependencies only point downwards:
//!
//! | Layer | Modules | Knows about |
//! |---|---|---|
//! | L0 | [`io`], [`hash`], [`compress`], [`atomic`] | bytes and crash-safe file replacement |
//! | L1 | [`container`], [`toc`], [`profile`] | blocks and resources as typed byte blobs |
//! | L2 | [`resource`], [`export`] | the contents of resource types, conversion to common formats |
//! | L3 | [`game`], [`character`], [`create`], [`workspace`], [`mods`], [`navigation`], [`external`], [`level`], [`logic_graph`], [`game_types`], [`report`], [`kb`] | a game directory: streams, tables of contents, indexes; new resources and streams; edits kept on disk; mods; search, references, unpack and pack; editing in other programs; levels; the logic graph of missions; typed messages, attributes and value lists; logs and error hints; the knowledge base `docs/kb/` |
//!
//! Every parser returns [`Error`] on malformed input and never panics.

pub mod atomic;
pub mod bridge;
pub mod character;
pub mod compress;
pub mod container;
pub mod create;
pub mod cutscene;
pub mod dialogue;
pub mod edition;
pub mod error;
pub mod export;
pub mod external;
pub mod game;
pub mod game_types;
pub mod hash;
pub mod import;
pub mod install;
pub mod io;
pub mod kb;
pub mod level;
pub mod live;
pub mod logic_graph;
pub mod mods;
pub mod navigation;
pub mod play;
pub mod profile;
pub mod project;
pub mod report;
pub mod resource;
pub mod retexture;
pub mod save;
pub mod scene;
pub mod script;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod text;
pub mod time_of_day;
pub mod toc;
pub mod tunables;
pub mod voice;
pub mod workspace;

pub use error::{Error, Result};
