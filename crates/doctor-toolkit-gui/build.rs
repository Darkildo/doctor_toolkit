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
}
