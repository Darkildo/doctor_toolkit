//! Library errors as Python exceptions.

use doctor_toolkit::Error;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

pyo3::create_exception!(
    doctor_toolkit,
    DoctorToolkitError,
    PyException,
    "Base class of every error raised by the library."
);
pyo3::create_exception!(
    doctor_toolkit,
    FormatError,
    DoctorToolkitError,
    "Malformed or unsupported data: bad magic, unexpected end, unknown version, RefPack."
);
pyo3::create_exception!(
    doctor_toolkit,
    InvalidArgumentError,
    DoctorToolkitError,
    "An argument the operation cannot honour, e.g. an unknown stream or a refused write."
);
pyo3::create_exception!(
    doctor_toolkit,
    ModError,
    DoctorToolkitError,
    "A malformed mod package, or a mod operation that cannot be carried out safely."
);
pyo3::create_exception!(
    doctor_toolkit,
    IoError,
    DoctorToolkitError,
    "An operating system error on a file; the message names the path."
);

/// Converts a library error into the matching Python exception, keeping its message.
pub fn to_py(error: Error) -> PyErr {
    let message = error.to_string();
    match error {
        Error::Io { .. } => IoError::new_err(message),
        Error::UnexpectedEof { .. }
        | Error::BadMagic { .. }
        | Error::UnsupportedVersion { .. }
        | Error::Malformed { .. }
        | Error::Refpack(_) => FormatError::new_err(message),
        Error::InvalidArgument(_) | Error::NotAGame { .. } => {
            InvalidArgumentError::new_err(message)
        }
        Error::Mod(_) => ModError::new_err(message),
    }
}

/// `?`-friendly conversion of library results.
pub trait ResultExt<T> {
    fn py(self) -> PyResult<T>;
}

impl<T> ResultExt<T> for doctor_toolkit::Result<T> {
    fn py(self) -> PyResult<T> {
        self.map_err(to_py)
    }
}

/// An [`InvalidArgumentError`] with `message`.
pub fn invalid(message: impl Into<String>) -> PyErr {
    InvalidArgumentError::new_err(message.into())
}

/// Registers the exception classes in the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("DoctorToolkitError", py.get_type::<DoctorToolkitError>())?;
    m.add("FormatError", py.get_type::<FormatError>())?;
    m.add(
        "InvalidArgumentError",
        py.get_type::<InvalidArgumentError>(),
    )?;
    m.add("ModError", py.get_type::<ModError>())?;
    m.add("IoError", py.get_type::<IoError>())?;
    Ok(())
}
