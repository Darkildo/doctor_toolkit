//! `kb-gen` and `kb-lint`: the knowledge base `docs/kb/` (see
//! `docs/kb/README.md`); `parity-gen`: the document of what the toolkit can
//! do and where. All work on a checkout of the repository, not on a game.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use clap::Args;

use doctor_toolkit::kb::{self, generate, lint};
use doctor_toolkit::parity;

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

#[derive(Args)]
pub struct ParityGen {
    /// The repository root [default: the nearest folder above the current
    /// one that has docs/kb]
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
    /// Write nothing: fail when a run would change the document
    #[arg(long)]
    pub check: bool,
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
/// lists of the folders; and the table of named addresses of the executable.
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
    // The table of named addresses the crash reports use (stage 99).
    let symbols = kb::symbols::run(&root, &root.join(KB), args.check)?;
    if symbols {
        println!(
            "{} {}",
            if args.check { "stale" } else { "wrote" },
            kb::symbols::SYMBOLS_FILE
        );
    }
    if args.check && (!out.is_clean() || symbols) {
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

/// Writes `docs/parity.md` from the table of capabilities.
pub fn parity(args: ParityGen) -> Result<()> {
    let root = root(args.root)?;
    let table = parity::table();
    let problems = table.problems();
    for problem in &problems {
        println!("{}: {problem}", parity::TABLE_FILE);
    }
    if !problems.is_empty() {
        bail!("{} problem(s) in {}", problems.len(), parity::TABLE_FILE);
    }
    let changed = parity::document::write(&root, args.check)?.is_some();
    let counts = parity::document::counts(table);
    println!(
        "{} capabilities: {}",
        counts.capabilities,
        match (changed, args.check) {
            (false, _) => format!("{} is up to date", parity::document::DOCUMENT),
            (true, false) => format!("wrote {}", parity::document::DOCUMENT),
            (true, true) => format!("stale {}", parity::document::DOCUMENT),
        }
    );
    if args.check && changed {
        bail!("the document is out of date; run `doctor-toolkit parity-gen`");
    }
    Ok(())
}
