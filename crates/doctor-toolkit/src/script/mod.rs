//! Visual scripts: graphs of nodes compiled to Lua 5.0 for the game.
//!
//! A script ([`ScriptGraph`]) is a dataflow and exec-flow graph — events,
//! flow control, values, game queries and actions ([`library`]) — kept as
//! JSON. [`compile`] turns it into readable Lua and a chunk for the game;
//! [`deploy`] puts the chunk into a level as a `lua` resource run by a
//! `LuaAudioScript` object; [`sim`] runs it against a mock of the game API.
//! See `docs/scripts.md`.

pub mod compile;
pub mod deploy;
pub mod graph;
pub mod library;
pub mod sim;
pub mod trial;

pub use compile::{Compiled, Diagnostic, Severity, compile};
pub use graph::{Link, Node, PinRef, PinType, ScriptGraph, Variable};

#[cfg(test)]
mod deploy_tests;
#[cfg(test)]
mod tests;
