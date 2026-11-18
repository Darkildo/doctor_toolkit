//! `unify`: the unified game — the same game folder for everybody, made
//! from the disc or the installed copy a player has and a pack with what
//! that disc lacks (see the library of this crate).

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use doctor_toolkit_unify::{self as unify, EditionTree, Manifest};

#[cfg(feature = "gui")]
mod gui;

/// The unified game of The Godfather II: one game folder for everybody,
/// made from your disc or installed copy (PAL or NTSC) and a pack with what
/// it lacks
#[derive(Parser)]
#[command(name = "gf2-unify", version)]
struct Cli {
    #[command(subcommand)]
    command: UnifyCommand,
}

fn main() {
    // No arguments (a double click): the window.
    #[cfg(feature = "gui")]
    if std::env::args_os().len() <= 1 {
        hide_console();
        if let Err(error) = gui::run(gui::Preset::default()) {
            eprintln!("error: the window could not be opened: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run(Cli::parse().command) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

/// On Windows a double click gives the program a console window of its
/// own; the window of the program replaces it.
#[cfg(all(feature = "gui", windows))]
fn hide_console() {
    unsafe extern "system" {
        fn FreeConsole() -> i32;
    }
    unsafe { FreeConsole() };
}

#[cfg(all(feature = "gui", not(windows)))]
fn hide_console() {}

#[derive(Subcommand)]
enum UnifyCommand {
    /// Make the unified game in a new folder from a pack and your copy
    ///
    /// SOURCE is the folder of a game disc (a mounted image, with the
    /// `data_*.cab` files) or an installed copy, of the PAL or the NTSC
    /// edition; it is only read. Every file taken from it is checked
    /// against the pack, the sound archives are rebuilt, and the result is
    /// byte-identical whichever edition it is made from. Run again after an
    /// interruption: finished files are kept.
    Apply {
        /// The pack folder (with `unified.json`)
        pack: PathBuf,
        /// The disc folder or the installed copy
        source: PathBuf,
        /// The game folder to make (not SOURCE)
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Check a game folder against a pack
    Verify { pack: PathBuf, game_dir: PathBuf },
    /// Show what a pack makes and what it needs from each edition
    Info { pack: PathBuf },
    /// Open the window with the folders filled in (without arguments the
    /// program opens it empty)
    #[cfg(feature = "gui")]
    Window {
        /// The disc folder or the installed copy
        #[arg(long, value_name = "DIR")]
        source: Option<PathBuf>,
        /// The pack folder [default: `pack` next to the program]
        #[arg(long, value_name = "DIR")]
        pack: Option<PathBuf>,
        /// The game folder to make
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
    },
    /// Set the language of the game: en, es, ru, cs, nl, pl
    ///
    /// On Windows writes the registry value the game reads (asks for
    /// administrator rights) and the settings of the game; elsewhere
    /// prints what to run in the Wine prefix of the game.
    Language { code: String },
    /// Make a pack from the unified tree and clean trees of the editions
    ///
    /// For the maintainers of the pack; `tools/unified-build/build.sh`
    /// makes the trees from the discs.
    Pack {
        /// The unified game folder
        unified: PathBuf,
        /// The pack folder to write
        #[arg(short, long)]
        output: PathBuf,
        /// A clean installed copy of the NTSC disc
        #[arg(long, value_name = "DIR")]
        ntsc: PathBuf,
        /// A clean installed copy of the PAL disc
        #[arg(long, value_name = "DIR")]
        pal: PathBuf,
        /// The file map of the NTSC disc (`disc_map.py`)
        #[arg(long, value_name = "TSV")]
        ntsc_disc_map: Option<PathBuf>,
        /// The file map of the PAL disc
        #[arg(long, value_name = "TSV")]
        pal_disc_map: Option<PathBuf>,
        /// Name of the unified game, for people
        #[arg(long, default_value = "The Godfather II 1.0.789.0, en es ru cs nl pl")]
        name: String,
    },
}

fn gib(bytes: u64) -> String {
    format!("{:.2} GiB", bytes as f64 / (1u64 << 30) as f64)
}

fn run(command: UnifyCommand) -> Result<()> {
    match command {
        UnifyCommand::Apply {
            pack,
            source,
            output,
        } => {
            let started = Instant::now();
            let mut last = Instant::now();
            let mut progress = |done: u64, total: u64, what: &str| {
                if last.elapsed().as_millis() < 500 && done < total {
                    return;
                }
                last = Instant::now();
                let percent = (done * 100).checked_div(total).unwrap_or(100);
                eprint!(
                    "\r\x1b[K{percent:3}%  {} of {}  {what}",
                    gib(done),
                    gib(total)
                );
                let _ = std::io::stderr().flush();
            };
            eprintln!("{}", pack_origin(&pack)?);
            let report = unify::apply(&pack, &source, &output, &mut progress);
            eprintln!();
            let report = report
                .with_context(|| format!("making the unified game in {}", output.display()))?;
            println!(
                "{}: {} file(s) from {}, {} from the pack, {} sound archive(s) built, {} already in place ({:.0} s)",
                output.display(),
                report.from_source,
                source.display(),
                report.from_pack,
                report.archives_built,
                report.kept,
                started.elapsed().as_secs_f64()
            );
            if !report.changed_in_source.is_empty() {
                println!(
                    "{} file(s) of the source differ from the disc and came from the pack: {}",
                    report.changed_in_source.len(),
                    report.changed_in_source.join(", ")
                );
            }
            Ok(())
        }
        UnifyCommand::Verify { pack, game_dir } => {
            let wrong = unify::verify(&pack, &game_dir)?;
            if wrong.is_empty() {
                println!(
                    "{}: the unified game, every file as in the pack",
                    game_dir.display()
                );
                return Ok(());
            }
            for path in &wrong {
                println!("{path}");
            }
            bail!(
                "{} file(s) of {} are absent or differ",
                wrong.len(),
                game_dir.display()
            )
        }
        #[cfg(feature = "gui")]
        UnifyCommand::Window {
            source,
            pack,
            output,
        } => gui::run(gui::Preset {
            source,
            pack,
            output,
        })
        .map_err(|e| anyhow::anyhow!("the window could not be opened: {e}")),
        UnifyCommand::Language { code } => {
            println!("{}", unify::locale::set_language(&code)?);
            Ok(())
        }
        UnifyCommand::Info { pack } => {
            let manifest = Manifest::load(&pack)?;
            println!("{}", manifest.name);
            println!("{}", pack_origin(&pack)?);
            println!(
                "{} file(s) and {} sound archive(s), {}",
                manifest.files.len(),
                manifest.archives.len(),
                gib(manifest.total_size())
            );
            for edition in &manifest.editions {
                let files: u64 = manifest
                    .files
                    .iter()
                    .filter(|f| !f.from.contains_key(&edition.id))
                    .map(|f| f.size)
                    .sum();
                let sounds = manifest
                    .archives
                    .iter()
                    .filter(|a| a.lacking.contains_key(&edition.id))
                    .count();
                println!(
                    "from {} ({}): the pack adds {} of files, and sounds to {} archive(s) (parts `common`, `{}`)",
                    edition.name,
                    edition.tag,
                    gib(files),
                    sounds,
                    edition.part
                );
            }
            Ok(())
        }
        UnifyCommand::Pack {
            unified,
            output,
            ntsc,
            pal,
            ntsc_disc_map,
            pal_disc_map,
            name,
        } => {
            let editions = [
                EditionTree {
                    id: "ntsc".into(),
                    name: "NTSC (1.0.766.0)".into(),
                    part: "for-ntsc".into(),
                    tree: ntsc,
                    disc_map: ntsc_disc_map,
                },
                EditionTree {
                    id: "pal".into(),
                    name: "PAL (1.0.764.0)".into(),
                    part: "for-pal".into(),
                    tree: pal,
                    disc_map: pal_disc_map,
                },
            ];
            let manifest = unify::build_pack(&name, &unified, &editions, &output, |line| {
                eprintln!("{line}")
            })?;
            println!(
                "{}: {} file(s), {} sound archive(s), {}",
                output.display(),
                manifest.files.len(),
                manifest.archives.len(),
                gib(manifest.total_size())
            );
            Ok(())
        }
    }
}

/// Whether the pack is a published one, for people.
fn pack_origin(pack: &std::path::Path) -> Result<String> {
    Ok(match unify::known_pack(pack)? {
        Some(known) => format!("the published pack, release {}", known.release),
        None => format!(
            "warning: not a pack published with this program (nobody vouches for its files); \
             the published one: {}",
            unify::latest_pack().magnet
        ),
    })
}
