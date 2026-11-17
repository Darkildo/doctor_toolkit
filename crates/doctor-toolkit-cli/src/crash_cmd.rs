//! `crash-report`: the reports the crash module writes when the game
//! crashes (stage 99), the module itself and a crash on purpose for
//! testing. See docs/analysis/crash-reports.md.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;

use doctor_toolkit::crash;

#[derive(Subcommand)]
pub enum CrashCommand {
    /// Print a report: the newest, or number N of `list` (1 is the newest)
    Show {
        game_dir: PathBuf,
        /// Which report: its number in `list`, or its name
        #[arg(value_name = "N|NAME")]
        which: Option<String>,
        /// The text as the module wrote it, without the names of the
        /// game's messages
        #[arg(long)]
        raw: bool,
        /// The summary as JSON
        #[arg(long, conflicts_with = "raw")]
        json: bool,
    },
    /// List the reports of a game, newest first
    List {
        game_dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Whether the module is in the game, and where its reports go
    Status { game_dir: PathBuf },
    /// Put the crash module into scripts/ of the game (needs the ASI
    /// loader). The mod manager and `play` do it themselves
    Install { game_dir: PathBuf },
    /// Take the module out of the game and keep the toolkit from putting
    /// it back
    Uninstall { game_dir: PathBuf },
    /// The module's log of the running or the last launch
    Log { game_dir: PathBuf },
    /// Crash the running game on purpose, to test the reports: the module
    /// sees the request within a second
    Test {
        game_dir: PathBuf,
        /// read: a read of address 0x14 on a thread of the game; jump: a
        /// call of 0x6E756F7B, an address in no module; thread: a read of
        /// address 0 on the module's own thread
        #[arg(long, default_value = "read")]
        kind: String,
    },
    /// What the knowledge base says about an address of godfather2.exe
    Lookup {
        /// Hexadecimal, with or without 0x
        address: String,
    },
}

impl CrashCommand {
    pub fn may_write_game(&self) -> bool {
        matches!(self, Self::Install { .. } | Self::Uninstall { .. })
    }
}

fn summary_json(report: &crash::CrashReport) -> serde_json::Value {
    report.json()
}

fn pick(reports: Vec<crash::CrashReport>, which: Option<&str>) -> Result<crash::CrashReport> {
    let count = reports.len();
    crash::pick(reports, which).with_context(|| match which {
        Some(w) => format!("no report `{w}` ({count} report(s); see `crash-report list`)"),
        None => "no crash reports for this game".to_owned(),
    })
}

pub fn run(command: &CrashCommand) -> Result<()> {
    let mut out = std::io::stdout();
    match command {
        CrashCommand::Show {
            game_dir,
            which,
            raw,
            json,
        } => {
            let report = pick(crash::reports(game_dir), which.as_deref())?;
            if *json {
                writeln!(
                    out,
                    "{}",
                    serde_json::to_string_pretty(&summary_json(&report))?
                )?;
                return Ok(());
            }
            let text = std::fs::read_to_string(&report.path)
                .with_context(|| report.path.display().to_string())?;
            if *raw {
                write!(out, "{text}")?;
            } else {
                write!(out, "{}", crash::annotate(&text))?;
                writeln!(out)?;
                writeln!(out, "report: {}", report.path.display())?;
                if let Some(dump) = &report.dump {
                    writeln!(out, "minidump: {}", dump.display())?;
                }
            }
        }
        CrashCommand::List { game_dir, json } => {
            let reports = crash::reports(game_dir);
            if *json {
                let all: Vec<_> = reports.iter().map(summary_json).collect();
                writeln!(out, "{}", serde_json::to_string_pretty(&all)?)?;
                return Ok(());
            }
            if reports.is_empty() {
                writeln!(
                    out,
                    "no crash reports in {}",
                    crash::dir(game_dir).display()
                )?;
            }
            for (i, report) in reports.iter().enumerate() {
                writeln!(out, "{:>2}  {}", i + 1, report.line())?;
                if let Some(name) = report.place_name() {
                    writeln!(out, "    {name}")?;
                }
            }
        }
        CrashCommand::Status { game_dir } => {
            let state = match crash::installed(game_dir) {
                crash::Installed::Ours => "installed by the toolkit",
                crash::Installed::Foreign => "a file the toolkit did not install (left alone)",
                crash::Installed::No => "not installed",
            };
            writeln!(out, "module: scripts/{}: {state}", crash::MODULE_FILE)?;
            writeln!(
                out,
                "switched off for this game: {}",
                if crash::disabled(game_dir) {
                    "yes"
                } else {
                    "no"
                }
            )?;
            writeln!(
                out,
                "ASI loader: {}",
                match doctor_toolkit::live::loader::find(game_dir) {
                    Some(loader) => loader.file.display().to_string(),
                    None => "none (the game cannot load the module)".to_owned(),
                }
            )?;
            writeln!(
                out,
                "module to install: {}",
                match crash::module_binary() {
                    Some(p) => p.display().to_string(),
                    None => "not built (tools/build-crash-module.sh)".to_owned(),
                }
            )?;
            let reports = crash::reports(game_dir);
            writeln!(
                out,
                "reports: {} in {}",
                reports.len(),
                crash::dir(game_dir).display()
            )?;
            if let Some(last) = reports.first() {
                writeln!(out, "last: {}", last.line())?;
            }
        }
        CrashCommand::Install { game_dir } => {
            let path = crash::module_binary()
                .context("the crash module is not built: run tools/build-crash-module.sh")?;
            let bytes = std::fs::read(&path).with_context(|| path.display().to_string())?;
            if doctor_toolkit::live::loader::find(game_dir).is_none() {
                writeln!(
                    out,
                    "warning: no ASI loader (dinput8.dll) in the game folder: the game will not load the module"
                )?;
            }
            crash::set_disabled(game_dir, false)?;
            let target = crash::install(game_dir, &bytes)?;
            if let Ok(game) = doctor_toolkit::game::GameInstance::open(game_dir.as_path()) {
                crash::write_context(
                    game_dir,
                    &crash::context(&game, "doctor-toolkit", "written"),
                )?;
            }
            writeln!(out, "installed {}", target.display())?;
            writeln!(
                out,
                "reports of crashes go to {}; `crash-report uninstall` takes the module away",
                crash::dir(game_dir).display()
            )?;
        }
        CrashCommand::Uninstall { game_dir } => {
            if crash::uninstall(game_dir)? {
                writeln!(out, "took the crash module out of {}", game_dir.display())?;
            } else {
                writeln!(out, "no crash module in {}", game_dir.display())?;
            }
            writeln!(
                out,
                "the toolkit will not put it back (`crash-report install` does)"
            )?;
        }
        CrashCommand::Log { game_dir } => {
            let path = crash::dir(game_dir).join(crash::LOG_FILE);
            match std::fs::read_to_string(&path) {
                Ok(text) => write!(out, "{text}")?,
                Err(_) => writeln!(out, "no log of the crash module ({})", path.display())?,
            }
        }
        CrashCommand::Test { game_dir, kind } => {
            if crash::installed(game_dir) == crash::Installed::No {
                bail!(
                    "no crash module in {} (`crash-report install`)",
                    game_dir.display()
                );
            }
            crash::request_test(game_dir, kind)?;
            writeln!(
                out,
                "asked the module to crash the game ({kind}); a running game crashes within a second or two, then see `crash-report show`"
            )?;
        }
        CrashCommand::Lookup { address } => {
            let hex = address.trim_start_matches("0x").trim_start_matches("0X");
            let address = u32::from_str_radix(hex, 16)
                .with_context(|| format!("`{address}` is not a hexadecimal address"))?;
            let lines = crash::lookup(address);
            if lines.is_empty() {
                writeln!(
                    out,
                    "{address:#010x}: the knowledge base (docs/kb/exe) has no name for it"
                )?;
            }
            for line in lines {
                writeln!(out, "{line}")?;
            }
        }
    }
    Ok(())
}
