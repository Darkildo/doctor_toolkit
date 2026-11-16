//! `kb-gen` and `kb-lint`: the knowledge base `docs/kb/` (see
//! `docs/kb/README.md`). Both work on a checkout of the repository, not on
//! a game.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use clap::Args;

use doctor_toolkit::kb::{self, generate, lint};

#[derive(Args)]
pub struct KbGen {
    /// The repository root [default: the nearest folder above the current
    /// one that has docs/kb]
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
    /// Write nothing: fail when a run would change a note
    #[arg(long)]
    pub check: bool,
}

#[derive(Args)]
pub struct KbLint {
    /// The repository root [default: the nearest folder above the current
    /// one that has docs/kb]
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
}

/// Path of the knowledge base from the repository root.
const KB: &str = "docs/kb";

fn root(given: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = given {
        return Ok(root);
    }
    let start = std::env::current_dir()?;
    let mut dir: &Path = &start;
    loop {
        if dir.join(KB).is_dir() {
            return Ok(dir.to_owned());
        }
        dir = dir.parent().ok_or_else(|| {
            anyhow!(
                "no {KB} here or above {}; give the repository with --root",
                start.display()
            )
        })?;
    }
}

/// Writes the generated notes: classes, resource types, messages and the
/// lists of the folders.
pub fn generate(args: KbGen) -> Result<()> {
    let root = root(args.root)?;
    let out = generate::run(
        &root.join(KB),
        &generate::Inputs::gf2(),
        &kb::today(),
        args.check,
    )?;
    for rel in &out.changed {
        println!("{} {KB}/{rel}", if args.check { "stale" } else { "wrote" });
    }
    for rel in &out.removed {
        println!(
            "{} {KB}/{rel}",
            if args.check { "extra" } else { "removed" }
        );
    }
    println!(
        "{} generated note(s): {} changed, {} removed",
        out.total,
        out.changed.len(),
        out.removed.len()
    );
    if args.check && !out.is_clean() {
        bail!("the generated notes are out of date; run `doctor-toolkit kb-gen`");
    }
    Ok(())
}

/// Checks every note; fails when one has a problem.
pub fn check(args: KbLint) -> Result<()> {
    let root = root(args.root)?;
    let base = kb::Kb::load(&root.join(KB))?;
    let problems = lint::lint(
        &base,
        &lint::Layout {
            root: &root,
            kb: KB,
        },
    )?;
    for problem in &problems {
        println!("{KB}/{problem}");
    }
    if !problems.is_empty() {
        bail!(
            "{} problem(s) in {} note(s) of {KB}",
            problems.len(),
            base.notes.len()
        );
    }
    println!("{} note(s) of {KB}: no problems", base.notes.len());
    Ok(())
}
