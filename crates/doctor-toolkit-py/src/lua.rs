//! Lua scripts: decompile, compile, list.

use doctor_toolkit::resource::lua::{self, Chunk, Comparison, opcodes};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::error::{InvalidArgumentError, ResultExt};

pyo3::create_exception!(
    doctor_toolkit,
    LuaSyntaxError,
    InvalidArgumentError,
    "A syntax error in Lua source; `args` is `(message, line)`."
);

/// Decompiles a `lua` resource and checks that the source compiles back.
///
/// Returns `source`, `round_trip` (`"identical"`, `"same_code"` or
/// `"different"`), `detail` (the difference or `None`) and `errors`
/// (functions that could not be decompiled).
#[pyfunction]
fn lua_decompile<'py>(py: Python<'py>, data: &[u8]) -> PyResult<Bound<'py, PyDict>> {
    let chunk = Chunk::parse(data).py()?;
    let rt = py.detach(|| lua::round_trip(&chunk));
    let (kind, detail) = match rt.comparison {
        Comparison::Identical => ("identical", None),
        Comparison::SameCode => ("same_code", None),
        Comparison::Different(why) => ("different", Some(why)),
    };
    let out = PyDict::new(py);
    out.set_item("source", rt.decompiled.source)?;
    out.set_item("round_trip", kind)?;
    out.set_item("detail", detail)?;
    out.set_item("errors", rt.decompiled.errors)?;
    Ok(out)
}

/// Compiles Lua 5.0 source to a chunk (4-byte float numbers like the game by default).
///
/// To replace a game script, pass the source name of the original chunk
/// (`Chunk.source` from `decode("lua", data)["source"]`).
#[pyfunction]
#[pyo3(signature = (source, chunk_name = "=script", number_size = 4))]
fn lua_compile<'py>(
    py: Python<'py>,
    source: &str,
    chunk_name: &str,
    number_size: u8,
) -> PyResult<Bound<'py, PyBytes>> {
    if number_size != 4 && number_size != 8 {
        return Err(InvalidArgumentError::new_err("number_size must be 4 or 8"));
    }
    let chunk =
        lua::compile(source.as_bytes(), chunk_name.as_bytes(), number_size).map_err(|e| {
            LuaSyntaxError::new_err((format!("line {}: {}", e.line, e.message), e.line))
        })?;
    Ok(PyBytes::new(py, &chunk.to_bytes()))
}

/// The instructions of a chunk as text (`luac -l` style).
#[pyfunction]
fn lua_listing(data: &[u8]) -> PyResult<String> {
    Ok(opcodes::listing(&Chunk::parse(data).py()?.main))
}

/// Compiles a visual script graph (its JSON text, `docs/scripts.md`).
///
/// Returns `source` (Lua 5.0), `chunk` (bytes for the game, or `None` when
/// there are errors) and `diagnostics`: dicts with `severity`, `node`,
/// `pin` and `message`.
#[pyfunction]
fn script_compile<'py>(py: Python<'py>, graph_json: &str) -> PyResult<Bound<'py, PyDict>> {
    let graph = doctor_toolkit::script::ScriptGraph::from_json(graph_json).py()?;
    let c = py.detach(|| doctor_toolkit::script::compile(&graph));
    let out = PyDict::new(py);
    out.set_item("source", &c.source)?;
    out.set_item("chunk", c.chunk.as_ref().map(|b| PyBytes::new(py, b)))?;
    let diags = c
        .diagnostics
        .iter()
        .map(|d| {
            let e = PyDict::new(py);
            e.set_item(
                "severity",
                match d.severity {
                    doctor_toolkit::script::Severity::Error => "error",
                    doctor_toolkit::script::Severity::Warning => "warning",
                },
            )?;
            e.set_item("node", d.node)?;
            e.set_item("pin", d.pin.as_deref())?;
            e.set_item("message", &d.message)?;
            Ok(e)
        })
        .collect::<PyResult<Vec<_>>>()?;
    out.set_item("diagnostics", diags)?;
    Ok(out)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("LuaSyntaxError", m.py().get_type::<LuaSyntaxError>())?;
    m.add_function(wrap_pyfunction!(script_compile, m)?)?;
    m.add_function(wrap_pyfunction!(lua_decompile, m)?)?;
    m.add_function(wrap_pyfunction!(lua_compile, m)?)?;
    m.add_function(wrap_pyfunction!(lua_listing, m)?)?;
    Ok(())
}
