//! `logic-run`: the logic of a scope run without the game (stage 38).

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Args;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::logic_graph::run::{self, ManualReaction, RunSpec, What};
use doctor_toolkit::workspace::Workspace;

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct RunArgs {
    pub game_dir: PathBuf,
    /// `mission:<name>`, `checkpoint:<number>`, `stream:<stream>`,
    /// `group:<stream>#<resource>`, `level:<stream>` or `game`
    pub scope: String,
    /// Inputs, in order: `load`, `send:MSG`, `enter:OBJ`, `exit:OBJ`,
    /// `var:NAME=N`, `checkpoint:N`, `wait:SECONDS`, `at:T:<input>`; several
    /// per value with `;`. Without any, the run starts with `load`
    #[arg(long = "events", short = 'e', num_args = 1.., value_name = "EVENT")]
    pub events: Vec<String>,
    /// Stop at this game time, seconds (default: when nothing is left)
    #[arg(long)]
    pub until: Option<f64>,
    /// Stop after this many steps
    #[arg(long, default_value_t = 20_000)]
    pub steps: usize,
    /// Stop before this message is sent (repeatable)
    #[arg(long = "break-message", value_name = "MESSAGE")]
    pub break_message: Vec<String>,
    /// Stop before this object gets a message: GUID, name or #actor (repeatable)
    #[arg(long = "break-object", value_name = "OBJECT")]
    pub break_object: Vec<String>,
    /// Reactions of black-box objects: JSON `[{"object": …, "on": …, "send": [{"message": …, "delay": …}]}]`
    #[arg(long)]
    pub reactions: Option<PathBuf>,
    /// Compare with a journal of the game (JSON, see docs/logic-graph.md §9.5)
    #[arg(long)]
    pub compare: Option<PathBuf>,
    /// Compare every message of the game journal, not only the scope's
    #[arg(long)]
    pub compare_all: bool,
    /// Seed of rand() for TriggerRandom
    #[arg(long, default_value_t = 1)]
    pub seed: u32,
    /// Do not run Lua scripts
    #[arg(long)]
    pub no_lua: bool,
    /// Print the journal as JSON (one entry per line with --lines)
    #[arg(long)]
    pub json: bool,
    /// With --json: one JSON entry per line
    #[arg(long, requires = "json")]
    pub lines: bool,
    /// Write the journal as JSON to this file too
    #[arg(long, short = 'o')]
    pub out: Option<PathBuf>,
    /// List the objects of the run and their models, then stop
    #[arg(long)]
    pub actors: bool,
    /// Read the game files only, without the edits of the workspace
    #[arg(long)]
    pub original: bool,
}

pub fn run(args: &RunArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let (index, _) = game.build_index(|_| {});
    let dir = Workspace::default_dir(game.root());
    let ws = (!args.original && Workspace::exists(&dir))
        .then(|| Workspace::open(&dir))
        .transpose()?;
    let reactions: Vec<ManualReaction> = match &args.reactions {
        Some(path) => {
            let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
            serde_json::from_str(&text)?
        }
        None => Vec::new(),
    };
    let spec = RunSpec {
        scope: args.scope.clone(),
        events: args.events.clone(),
        until: args.until,
        steps: Some(args.steps),
        break_message: args.break_message.clone(),
        break_object: args.break_object.clone(),
        reactions,
        seed: Some(args.seed),
        no_lua: args.no_lua,
    };
    let (mut r, warnings) = spec.prepare(&game, &index, ws.as_ref())?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    if args.actors {
        for (i, a) in r.actors().iter().enumerate() {
            out!(
                "#{i:<4} {:<18} {:<22} {:<28} {}",
                a.model.kind(),
                a.class,
                a.label(),
                a.listens
                    .iter()
                    .map(|(m, pin)| format!("{pin}={}", m.name))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        return Ok(());
    }
    let stop = spec.run(&mut r);
    let journal = r.journal();
    if let Some(path) = &args.out {
        std::fs::write(path, serde_json::to_string_pretty(journal)?)?;
    }
    if args.json {
        if args.lines {
            for e in journal {
                out!("{}", serde_json::to_string(e)?);
            }
        } else {
            out!("{}", serde_json::to_string_pretty(journal)?);
        }
    } else {
        for e in journal {
            out!("{}", e.line());
        }
        let sends = journal
            .iter()
            .filter(|e| matches!(e.what, What::Send { .. }))
            .count();
        let unknown = r.actors().iter().filter(|a| a.model.is_black_box()).count();
        out!(
            "\n{} objects ({} black boxes), {} steps, {} messages sent, time {:.3} s; stopped: {}",
            r.actors().len(),
            unknown,
            r.steps(),
            sends,
            r.time(),
            stop.text()
        );
        for (t, what) in r.pending().iter().take(10) {
            out!("  queued at {t:.3}: {what}");
        }
    }
    if let Some(path) = &args.compare {
        let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        let game_journal = run::parse_game_journal(&text)?;
        let scope = if args.compare_all {
            Default::default()
        } else {
            r.scope_messages()
        };
        let c = run::compare(r.journal(), &game_journal, &scope);
        if args.json {
            eprintln!("{}", serde_json::to_string_pretty(&c)?);
        } else {
            out!(
                "\ncompared with {} game messages ({} outside the scope left out): {} in both, {} only in the run, {} only in the game; time offset {}, worst drift {:.3} s",
                game_journal.len(),
                c.ignored,
                c.matched.len(),
                c.only_run.len(),
                c.only_game.len(),
                c.offset
                    .map_or_else(|| "-".to_owned(), |o| format!("{o:.3} s")),
                c.worst_drift()
            );
            for m in c
                .matched
                .iter()
                .filter(|m| !m.only_run.is_empty() || !m.only_game.is_empty())
            {
                out!(
                    "  {} (entry {}): receivers only in the run {:?}, only in the game {:?}",
                    m.message,
                    m.seq,
                    m.only_run,
                    m.only_game
                );
            }
            for (seq, name) in &c.only_run {
                out!("  only in the run: {name} (entry {seq})");
            }
            for (i, name) in &c.only_game {
                out!("  only in the game: {name} (message {i})");
            }
        }
    }
    Ok(())
}
