//! `mcp`: the Model Context Protocol server on stdin/stdout (`docs/mcp.md`):
//! read-only by default (stage 50), edits in the workspace with
//! `--allow-edits`, the game written and run with `--allow-game` (stage 51).

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::Args;

#[derive(Args)]
pub struct McpArgs {
    /// The game folder
    pub game_dir: PathBuf,
    /// Workspace directory [default: from the project, else <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    pub workspace: Option<PathBuf>,
    /// A mod project folder, for `project_status`
    #[arg(long, value_name = "DIR")]
    pub project: Option<PathBuf>,
    /// Build the game index in memory instead of keeping it in the cache
    /// (`~/.cache/doctor-toolkit/`)
    #[arg(long)]
    pub no_cache: bool,
    /// Longest tool result in characters (Claude Code refuses results
    /// above 25 000 tokens by default)
    #[arg(long, value_name = "N", default_value_t = doctor_toolkit_mcp::output::DEFAULT_MAX_CHARS)]
    pub max_response_chars: usize,
    /// Tools that record edits in the workspace (undoable; the game files
    /// are not changed): set_attribute, add_object, edit_string, …
    #[arg(long)]
    pub allow_edits: bool,
    /// Tools that write the workspace into the game files, start and stop
    /// the game (on an X server of its own) and act in the running game
    /// over the live link: apply, play, stop, screenshot, teleport, …
    #[arg(long)]
    pub allow_game: bool,
    /// Folder the export tools write files into (models, textures, sounds,
    /// frames, …); without it they refuse
    #[arg(long, value_name = "DIR")]
    pub output_dir: Option<PathBuf>,
}

/// Serves the game until the client closes stdin. Messages for people go to
/// stderr; stdout carries only the protocol.
pub fn run(args: McpArgs) -> Result<()> {
    let mut config = doctor_toolkit_mcp::Config::new(&args.game_dir);
    config.workspace = args.workspace;
    config.project = args.project;
    config.max_chars = args.max_response_chars;
    config.allow_edits = args.allow_edits;
    config.allow_game = args.allow_game;
    if let Some(dir) = &args.output_dir {
        std::fs::create_dir_all(dir)
            .map_err(|e| anyhow!("cannot make the output folder {}: {e}", dir.display()))?;
        config.output_dir = Some(dir.canonicalize()?);
    }
    if args.no_cache {
        config.index_cache = None;
    }
    let mode = match (args.allow_edits, args.allow_game) {
        (false, false) => "read-only",
        (true, false) => "edits in the workspace",
        (false, true) => "writes into the game",
        (true, true) => "edits in the workspace and writes into the game",
    };
    eprintln!(
        "doctor-toolkit MCP server ({mode}) for {}; waiting for a client on stdin",
        args.game_dir.display()
    );
    doctor_toolkit_mcp::serve_stdio(&config).map_err(|e| anyhow!("{e}"))
}
