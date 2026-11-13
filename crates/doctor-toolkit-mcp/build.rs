//! Embeds the documents the server offers as MCP resources: every
//! `docs/formats/*.md` and a few guides of `docs/`. A new format document is
//! picked up without touching the code.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Guides of `docs/` the server offers besides the format documents.
const GUIDES: &[&str] = &[
    "mcp.md",
    "navigation.md",
    "logic-graph.md",
    "levels.md",
    "workspace.md",
    "project.md",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let docs = manifest.join("../../docs");
    let formats = docs.join("formats");
    println!("cargo:rerun-if-changed={}", formats.display());
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(&formats)
        .expect("docs/formats")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .map(|p| (format!("formats/{}", name(&p)), p))
        .collect();
    files.sort();
    for guide in GUIDES {
        let path = docs.join(guide);
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_file() {
            files.push(((*guide).to_owned(), path));
        }
    }
    let mut out = String::from("/// `(path below docs/, text)` of the embedded documents.\n");
    out.push_str("pub static DOCS: &[(&str, &str)] = &[\n");
    for (rel, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let abs = path.canonicalize().expect("document path");
        writeln!(
            out,
            "    ({rel:?}, include_str!({:?})),",
            abs.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("docs.rs");
    std::fs::write(dest, out).unwrap();
}

fn name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}
