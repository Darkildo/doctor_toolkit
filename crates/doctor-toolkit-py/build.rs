//! Linker arguments of a Python extension module (`-undefined dynamic_lookup`
//! on macOS), so that plain `cargo build` links it as maturin does.

fn main() {
    pyo3_build_config::add_extension_module_link_args();
}
