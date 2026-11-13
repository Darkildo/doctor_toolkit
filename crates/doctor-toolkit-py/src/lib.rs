//! Python bindings of `doctor-toolkit`: the extension module
//! `doctor_toolkit._native`, re-exported by the Python package `doctor_toolkit`.
//!
//! The bindings are thin: every object wraps a library type and every method
//! calls one library function. Long operations (indexing, model export,
//! applying a workspace or mods, unpacking) release the GIL. Library errors
//! become the exceptions of [`error`], with the library's message.
//!
//! The Python API is described by `python/doctor_toolkit/_native.pyi` and
//! `docs/python.md`.

mod characters;
mod error;
mod formats;
mod game;
mod logic;
mod lua;
mod mods;
mod navigation;
mod project;
mod testing;
mod util;
mod workspace;

use pyo3::prelude::*;

#[pymodule]
#[pyo3(name = "_native")]
fn doctor_toolkit_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    error::register(m)?;
    util::register(m)?;
    game::register(m)?;
    formats::register(m)?;
    lua::register(m)?;
    logic::register(m)?;
    characters::register(m)?;
    navigation::register(m)?;
    workspace::register(m)?;
    project::register(m)?;
    mods::register(m)?;
    testing::register(m)?;
    Ok(())
}
