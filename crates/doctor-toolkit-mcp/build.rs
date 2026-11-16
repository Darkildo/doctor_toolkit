//! Embeds the documents the server offers as MCP resources: every
//! `docs/formats/*.md`, a few guides of `docs/` and the notes of the
//! knowledge base `docs/kb/`. A new format document or note is picked up
//! without touching the code.

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

    // The knowledge base: every note below docs/kb, by its path there.
    let kb = docs.join("kb");
    println!("cargo:rerun-if-changed={}", kb.display());
    let mut notes: Vec<(String, PathBuf)> = Vec::new();
    collect(&kb, "", &mut notes);
    notes.sort();
    out.push_str("/// `(path below docs/kb/, text)` of the notes of the knowledge base.\n");
    out.push_str("pub static KB: &[(&str, &str)] = &[\n");
    for (rel, path) in &notes {
        println!("cargo:rerun-if-changed={}", path.display());
        let abs = path.canonicalize().expect("note path");
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

/// The `.md` files below `dir`, with their `/`-separated paths below it.
fn collect(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    println!("cargo:rerun-if-changed={}", dir.display());
    for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
        let file = name(&path);
        if file.starts_with('.') {
            continue;
        }
        let rel = if rel.is_empty() {
            file
        } else {
            format!("{rel}/{file}")
        };
        if path.is_dir() {
            collect(&path, &rel, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push((rel, path));
        }
    }
}

fn name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}
