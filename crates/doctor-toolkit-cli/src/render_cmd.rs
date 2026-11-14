//! `render`: the graphics module (stage 59) outside a launch — its
//! configuration, keeping it in the game for good, its log.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

use doctor_toolkit::play::render;

use crate::play_cmd::GraphicsArgs;

#[derive(Subcommand)]
pub enum RenderCommand {
    /// Print the module's configuration (doctor_render.cfg) for these
    /// options
    Config {
        #[command(flatten)]
        graphics: GraphicsArgs,
    },
    /// Put the module and its configuration into scripts/ of the game for
    /// good (every launch, also without the toolkit; needs the ASI loader).
    /// "Play" then uses it instead of installing its own
    Install {
        game_dir: PathBuf,
        #[command(flatten)]
        graphics: GraphicsArgs,
    },
    /// Take the module (installed by `render install`) out of the game
    Uninstall { game_dir: PathBuf },
    /// The module's log: of the running or the last launch
    Log { game_dir: PathBuf },
}

impl RenderCommand {
    pub fn may_write_game(&self) -> bool {
        matches!(self, Self::Install { .. } | Self::Uninstall { .. })
    }
}

pub fn run(command: &RenderCommand) -> Result<()> {
    let mut out = std::io::stdout();
    match command {
        RenderCommand::Config { graphics } => {
            write!(out, "{}", graphics.options().config().to_text())?;
        }
        RenderCommand::Install { game_dir, graphics } => {
            let path = render::module_binary()
                .context("the graphics module is not built: run tools/build-render-module.sh")?;
            let bytes = std::fs::read(&path).with_context(|| path.display().to_string())?;
            if doctor_toolkit::live::loader::find(game_dir).is_none() {
                writeln!(
                    out,
                    "warning: no ASI loader (dinput8.dll) in the game folder: the game will not load the module"
                )?;
            }
            let config = graphics.options().config();
            let target = render::install_permanent(game_dir, &bytes, &config)?;
            writeln!(out, "installed {}: {}", target.display(), config.summary())?;
            writeln!(
                out,
                "edit {} next to it to change the options; `render uninstall` takes it away",
                render::CONFIG_FILE
            )?;
        }
        RenderCommand::Uninstall { game_dir } => {
            if render::uninstall_permanent(game_dir)? {
                writeln!(
                    out,
                    "took the graphics module out of {}",
                    game_dir.display()
                )?;
            } else {
                writeln!(out, "no graphics module in {}", game_dir.display())?;
            }
        }
        RenderCommand::Log { game_dir } => match render::last_log(game_dir) {
            Some(text) => write!(out, "{text}")?,
            None => writeln!(
                out,
                "no log of the graphics module in {}",
                game_dir.display()
            )?,
        },
    }
    Ok(())
}
