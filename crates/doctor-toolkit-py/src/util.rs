//! Hash helpers, type names, resource keys and conversions shared by the other modules.

use std::sync::OnceLock;

use doctor_toolkit::hash;
use doctor_toolkit::navigation::{Dictionary, HashFn, Origin};
use doctor_toolkit::resource::names::{mem_policies, type_tags};
use doctor_toolkit::workspace::ResourceKey;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyString};

/// The bytes a hash function reads: a `str` as UTF-8, or `bytes` as they are.
fn hash_input(data: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(text) = data.cast::<PyString>() {
        Ok(text.to_cow()?.as_bytes().to_vec())
    } else if let Ok(bytes) = data.cast::<PyBytes>() {
        Ok(bytes.as_bytes().to_vec())
    } else {
        Err(PyTypeError::new_err("expected str or bytes"))
    }
}

/// SDBM hash, the hash of type tags, memory policies and (of the lower-case
/// name) SimGroup classes and metamodel properties.
#[pyfunction]
fn sdbm(data: &Bound<'_, PyAny>) -> PyResult<u32> {
    Ok(hash::sdbm(&hash_input(data)?))
}

/// CRC-32 (IEEE), the hash of material and shader names (lower case).
#[pyfunction]
fn crc32(data: &Bound<'_, PyAny>) -> PyResult<u32> {
    Ok(hash::crc32(&hash_input(data)?))
}

/// 32-bit FNV-1a.
#[pyfunction]
fn fnv1a(data: &Bound<'_, PyAny>) -> PyResult<u32> {
    Ok(hash::fnv1a(&hash_input(data)?))
}

/// `text` under every hash function the engine is known to use, by function name
/// (`sdbm`, `sdbm(lower)`, `crc32`, `crc32(lower)`, `fnv1a`, `fnv1a(lower)`).
#[pyfunction]
fn hash_all<'py>(py: Python<'py>, text: &str) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for f in HashFn::ALL {
        dict.set_item(f.name(), f.apply(text))?;
    }
    Ok(dict)
}

/// The type id of a resource type: a known tag (`tg4h`, `mesh`, ...), a
/// hexadecimal id (`0xcdb9d42b`), any other tag (its SDBM hash), or an int.
pub fn resolve_type(value: &Bound<'_, PyAny>) -> PyResult<u32> {
    if let Ok(id) = value.extract::<u32>() {
        return Ok(id);
    }
    let text: String = value
        .extract()
        .map_err(|_| PyTypeError::new_err("a resource type is a tag (str) or a type id (int)"))?;
    let text = text.trim();
    if text.is_empty() {
        return Err(PyValueError::new_err("empty resource type"));
    }
    if let Some(id) = type_tags().resolve(text)
        && (type_tags().get(id).is_some()
            || text.starts_with("0x")
            || (text.len() == 8 && text.chars().all(|c| c.is_ascii_hexdigit())))
    {
        return Ok(id);
    }
    Ok(hash::sdbm(text.to_ascii_lowercase().as_bytes()))
}

/// The type id of a resource type tag or hexadecimal id; see `type_name` for the reverse.
#[pyfunction]
fn type_id(tag: &Bound<'_, PyAny>) -> PyResult<u32> {
    resolve_type(tag)
}

/// The tag of a type id (`tg4h`), or the id in hexadecimal when the tag is not known.
#[pyfunction]
pub fn type_name(type_id: u32) -> String {
    type_tags().display(type_id)
}

/// The name of a memory policy hash, or the hash in hexadecimal.
pub fn mem_policy_name(hash: u32) -> String {
    mem_policies().display(hash)
}

/// The dictionary of the name lists shipped with the library.
pub fn builtin_dictionary() -> &'static Dictionary {
    static DICT: OnceLock<Dictionary> = OnceLock::new();
    DICT.get_or_init(Dictionary::builtin)
}

/// `(name, origin)` pairs of a dictionary lookup.
pub fn lookup_in(dict: &Dictionary, hash: u32) -> Vec<(String, String)> {
    dict.lookup(hash)
        .into_iter()
        .map(|(name, origin): (&str, Origin)| (name.to_owned(), origin.describe()))
        .collect()
}

/// Names of a 32-bit hash known from the builtin name lists, with how each is
/// known (`sdbm`, `crc32(lower)`, ...). `Index.lookup` also knows the names of a game.
#[pyfunction]
fn lookup(hash: u32) -> Vec<(String, String)> {
    lookup_in(builtin_dictionary(), hash)
}

/// Parses a resource key `stream:type:ids`.
pub fn parse_key(text: &str) -> PyResult<ResourceKey> {
    text.parse::<ResourceKey>().map_err(crate::error::to_py)
}

/// The parts of a resource key: `stream`, `type`, `type_id`, `ids`.
#[pyfunction(name = "parse_key")]
fn parse_key_py<'py>(py: Python<'py>, key: &str) -> PyResult<Bound<'py, PyDict>> {
    let key = parse_key(key)?;
    let dict = PyDict::new(py);
    dict.set_item("stream", &key.stream)?;
    dict.set_item("type", key.type_name())?;
    dict.set_item("type_id", key.type_id)?;
    dict.set_item("ids", key.ids)?;
    Ok(dict)
}

/// Converts a serialisable value into Python objects (dicts, lists, numbers, strings).
pub fn to_python<'py, T: serde::Serialize>(
    py: Python<'py>,
    value: &T,
) -> PyResult<Bound<'py, PyAny>> {
    pythonize::pythonize(py, value).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// `bytes` with the contents of `data`.
pub fn bytes<'py>(py: Python<'py>, data: &[u8]) -> Bound<'py, PyBytes> {
    PyBytes::new(py, data)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sdbm, m)?)?;
    m.add_function(wrap_pyfunction!(crc32, m)?)?;
    m.add_function(wrap_pyfunction!(fnv1a, m)?)?;
    m.add_function(wrap_pyfunction!(hash_all, m)?)?;
    m.add_function(wrap_pyfunction!(type_id, m)?)?;
    m.add_function(wrap_pyfunction!(type_name, m)?)?;
    m.add_function(wrap_pyfunction!(lookup, m)?)?;
    m.add_function(wrap_pyfunction!(parse_key_py, m)?)?;
    Ok(())
}
