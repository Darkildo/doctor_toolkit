//! `mcp`: the Model Context Protocol server on stdin/stdout (stage 50,
//! read-only; `docs/mcp.md`).

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
}

/// Serves the game until the client closes stdin. Messages for people go to
/// stderr; stdout carries only the protocol.
pub fn run(args: McpArgs) -> Result<()> {
    let mut config = doctor_toolkit_mcp::Config::new(&args.game_dir);
    config.workspace = args.workspace;
    config.project = args.project;
    config.max_chars = args.max_response_chars;
    if args.no_cache {
        config.index_cache = None;
    }
    eprintln!(
        "doctor-toolkit MCP server (read-only) for {}; waiting for a client on stdin",
        args.game_dir.display()
    );
    doctor_toolkit_mcp::serve_stdio(&config).map_err(|e| anyhow!("{e}"))
}
