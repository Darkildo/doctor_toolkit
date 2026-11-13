//! Resource containers (`.str` files).
//!
//! The engine has two container backends. [`ustream`] is used by
//! The Godfather II and Dead Space. The older RenderWare-based RwStream
//! (The Godfather 1) is not implemented: no sample files are available.

pub mod big;
pub mod ustream;
