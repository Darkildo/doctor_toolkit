//! Records the commit, target, profile and compiler of the build for the
//! About window (`src/build_info.rs`).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn main() {
    let commit = match git(&["rev-parse", "--short=10", "HEAD"]) {
        Some(hash) => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
            if dirty {
                format!("{hash}+changes")
            } else {
                hash
            }
        }
        None => "unknown".to_owned(),
    };
    println!("cargo:rustc-env=DOCTOR_TOOLKIT_COMMIT={commit}");
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    println!("cargo:rustc-env=DOCTOR_TOOLKIT_TARGET={}", var("TARGET"));
    println!("cargo:rustc-env=DOCTOR_TOOLKIT_PROFILE={}", var("PROFILE"));
    let rustc = Command::new(var("RUSTC"))
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    println!("cargo:rustc-env=DOCTOR_TOOLKIT_RUSTC={rustc}");
    // Build again when the commit changes, not only when a source file does.
    for path in ["HEAD", "packed-refs"] {
        if let Some(file) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={file}");
        }
    }
    if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(file) = git(&["rev-parse", "--git-path", &head])
    {
        println!("cargo:rerun-if-changed={file}");
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
    embed_knowledge_base();
}

/// Embeds the notes of the knowledge base `docs/kb/` for the Knowledge base
/// tab, as the MCP server embeds them: `(path below docs/kb/, text)`.
fn embed_knowledge_base() {
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};

    fn collect(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        println!("cargo:rerun-if-changed={}", dir.display());
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if path.is_dir() {
                collect(&path, &rel, out);
            } else if name.ends_with(".md") {
                out.push((rel, path));
            }
        }
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let kb = manifest.join("../../docs/kb");
    let mut notes = Vec::new();
    collect(&kb, "", &mut notes);
    notes.sort();
    let mut out =
        String::from("/// `(path below docs/kb/, text)` of the notes of the knowledge base.\n");
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
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("kb.rs");
    std::fs::write(dest, out).unwrap();
}
