//! The table of capabilities (`data/toolkit/capabilities.toml`): the
//! document written from it is up to date (`doctor-toolkit parity-gen`),
//! and the Python names of its rows are in the bindings. The tools of the
//! MCP server, the commands of the editor and of the command line are
//! checked by tests of their crates.

use std::path::Path;

use doctor_toolkit::parity::{self, Kind, document};

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_document_is_up_to_date() {
    let stale = document::write(&root(), true).unwrap();
    assert!(
        stale.is_none(),
        "run `doctor-toolkit parity-gen`: {} is out of date",
        document::DOCUMENT
    );
}

#[test]
fn the_document_names_no_closed_file() {
    let text = std::fs::read_to_string(root().join(document::DOCUMENT)).unwrap();
    for closed in ["PLAN.md", "visceral-plugin"] {
        assert!(!text.contains(closed), "{closed}");
    }
}

/// The names the Python module gives, read from the sources of the
/// bindings: functions (`#[pyfunction]`) and `Class.method` of every
/// `#[pymethods]` block.
fn python_names() -> Vec<String> {
    let dir = root().join("crates/doctor-toolkit-py/src");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let mut class: Option<&str> = None;
        let mut function = false;
        let mut renamed: Option<String> = None;
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if let Some(rest) = line.strip_prefix("impl ")
                && i > 0
                && lines[i - 1].contains("#[pymethods]")
            {
                class = rest.split([' ', '{']).next();
                continue;
            }
            if line.starts_with('}') {
                class = None;
            }
            if trimmed.starts_with("#[pyfunction") {
                function = true;
            }
            if (trimmed.starts_with("#[pyfunction") || trimmed.starts_with("#[pyo3(name"))
                && let Some(at) = trimmed.find("name = \"")
            {
                let name = &trimmed[at + 8..];
                renamed = name.split('"').next().map(str::to_owned);
                continue;
            }
            let Some(rest) = trimmed
                .strip_prefix("pub fn ")
                .or_else(|| trimmed.strip_prefix("fn "))
            else {
                continue;
            };
            let own = rest.split(['(', '<']).next().unwrap_or_default().to_owned();
            let name = renamed.take().unwrap_or(own);
            if function {
                out.push(name);
            } else if let Some(class) = class
                && line.starts_with("    ")
                && !line.starts_with("     ")
            {
                out.push(format!("{class}.{name}"));
            }
            function = false;
        }
    }
    out
}

#[test]
fn the_python_names_of_the_table_exist() {
    let names = python_names();
    assert!(names.len() > 150, "{}", names.len());
    assert!(names.iter().any(|n| n == "Workspace.undo"));
    assert!(names.iter().any(|n| n == "logic_graph"));
    assert!(names.iter().any(|n| n == "ModManager.move"));
    let problems = parity::table().check_names(Kind::Python, names.iter().map(String::as_str), []);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
