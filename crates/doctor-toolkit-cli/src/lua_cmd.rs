//! Lua scripts: decompile, compile, list, import into the workspace.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::lua::{self, Chunk, Comparison, GAME_NUMBER_SIZE, TYPE_LUA, opcodes};

use crate::workspace_cmd::{self, EditMode};

/// Writes a line to stdout and exits quietly when the pipe is closed (`| head`).
macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct Decompile {
    /// A `.luac` file, or a game directory to decompile every script
    pub input: PathBuf,
    /// Output `.lua` file (for a file; default: stdout) or directory (for a game)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Args)]
pub struct Compile {
    /// Lua 5.0 source file
    pub input: PathBuf,
    /// Output chunk [default: the input with the extension `.luac`]
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Source name stored in the chunk [default: the input file name]
    #[arg(long)]
    pub chunk_name: Option<String>,
    /// 8-byte double numbers (stock Lua) instead of the game's 4-byte floats
    #[arg(long)]
    pub double: bool,
}

#[derive(Args)]
pub struct Import {
    pub game_dir: PathBuf,
    /// Lua source files; each replaces the script with the same file name
    /// (`aud_utils.lua` replaces `x:\...\aud_utils.lua`)
    #[arg(required = true)]
    pub scripts: Vec<PathBuf>,
    /// Replace this script instead (file name of its source or resource id in hex);
    /// only with one input file
    #[arg(long)]
    pub resource: Option<String>,
    #[command(flatten)]
    pub mode: EditMode,
}

/// A `lua` resource of the game.
struct Script {
    stream: usize,
    stream_name: String,
    index: usize,
    id: u32,
    chunk: Chunk,
    data: Vec<u8>,
}

impl Script {
    fn file_name(&self) -> String {
        lua::file_name(&self.chunk.source()).to_owned()
    }
}

fn scripts(game: &GameInstance) -> Result<Vec<Script>> {
    let mut out = Vec::new();
    for (stream, file) in game.streams().iter().enumerate() {
        let opened = game.open_stream(stream)?;
        for (index, resource) in opened.layout().resources.iter().enumerate() {
            if resource.header.type_id != TYPE_LUA {
                continue;
            }
            let data = opened
                .resource_data(index)
                .with_context(|| format!("{} resource {index}", file.rel_path))?;
            let chunk = Chunk::parse(&data)
                .with_context(|| format!("{} resource {index}", file.rel_path))?;
            out.push(Script {
                stream,
                stream_name: file.rel_path.trim_end_matches(".str").to_owned(),
                index,
                id: resource.header.ids[3],
                chunk,
                data,
            });
        }
    }
    Ok(out)
}

fn read_chunk(path: &Path) -> Result<Chunk> {
    let data = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    Chunk::parse(&data).with_context(|| format!("{} is not a Lua 5.0 chunk", path.display()))
}

/// `lua-decompile`
pub fn decompile(args: &Decompile) -> Result<()> {
    if args.input.is_file() {
        let chunk = read_chunk(&args.input)?;
        let rt = lua::round_trip(&chunk);
        match &args.output {
            Some(path) => {
                fs::write(path, &rt.decompiled.source)
                    .with_context(|| format!("cannot write {}", path.display()))?;
                out!("{}: {}", path.display(), rt.comparison);
            }
            None => {
                print!("{}", rt.decompiled.source);
                eprintln!("round trip: {}", rt.comparison);
            }
        }
        return Ok(());
    }
    let output = args
        .output
        .as_ref()
        .context("give an output directory with -o for a game")?;
    let game = GameInstance::open(&args.input)?;
    let mut counts = [0usize; 3];
    for script in scripts(&game)? {
        let rt = lua::round_trip(&script.chunk);
        let dir = output.join(&script.stream_name);
        fs::create_dir_all(&dir)?;
        let name = match script.file_name().as_str() {
            "" => format!("{:05}_lua_{:08x}.lua", script.index, script.id),
            n => n.to_owned(),
        };
        let path = dir.join(&name);
        fs::write(&path, &rt.decompiled.source)?;
        counts[match rt.comparison {
            Comparison::Identical => 0,
            Comparison::SameCode => 1,
            Comparison::Different(_) => 2,
        }] += 1;
        out!("{}: {}", path.display(), rt.comparison);
    }
    out!(
        "{} script(s): {} byte-identical, {} same instructions, {} different",
        counts.iter().sum::<usize>(),
        counts[0],
        counts[1],
        counts[2]
    );
    Ok(())
}

/// `lua-compile`
pub fn compile(args: &Compile) -> Result<()> {
    let source =
        fs::read(&args.input).with_context(|| format!("cannot read {}", args.input.display()))?;
    let name = args.chunk_name.clone().unwrap_or_else(|| {
        args.input
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let number_size = if args.double { 8 } else { GAME_NUMBER_SIZE };
    let chunk = lua::compile(&source, name.as_bytes(), number_size)
        .map_err(|e| anyhow::anyhow!("{}:{}: {}", args.input.display(), e.line, e.message))?;
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| args.input.with_extension("luac"));
    fs::write(&output, chunk.to_bytes())
        .with_context(|| format!("cannot write {}", output.display()))?;
    out!(
        "{}: {} function(s), {} instruction(s)",
        output.display(),
        chunk.main.function_count(),
        chunk.main.instruction_count()
    );
    Ok(())
}

/// `lua-list`
pub fn list(input: &Path) -> Result<()> {
    let chunk = read_chunk(input)?;
    out!(
        "source {:?}, {}-byte numbers",
        chunk.source(),
        chunk.number_size
    );
    print!("{}", opcodes::listing(&chunk.main));
    Ok(())
}

/// `lua-import`
pub fn import(args: &Import) -> Result<()> {
    if args.resource.is_some() && args.scripts.len() != 1 {
        bail!("--resource takes exactly one script");
    }
    let game = GameInstance::open(&args.game_dir)?;
    let all = scripts(&game)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let mut touched = Vec::new();
    for path in &args.scripts {
        let wanted = match &args.resource {
            Some(r) => r.clone(),
            None => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let script = all
            .iter()
            .find(|s| {
                s.file_name().eq_ignore_ascii_case(&wanted)
                    || format!("{:08x}", s.id).eq_ignore_ascii_case(wanted.trim_start_matches("0x"))
            })
            .with_context(|| format!("no script `{wanted}` in the game"))?;
        let source = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        let chunk_name = script.chunk.main.source.clone().unwrap_or_default();
        let compiled = lua::compile(&source, &chunk_name, script.chunk.number_size)
            .map_err(|e| anyhow::anyhow!("{}:{}: {}", path.display(), e.line, e.message))?;
        let bytes = compiled.to_bytes();
        let note = if bytes == script.data {
            "same as the original"
        } else {
            match lua::compare(&script.chunk, &compiled) {
                Comparison::SameCode => "same code as the original, other lines",
                _ => "changed",
            }
        };
        let label = format!("Import {} ({})", script.file_name(), script.stream_name);
        let changed = ws.set_resource(&game, script.stream, script.index, bytes, &label)?;
        out!(
            "{} -> {} resource {} ({}): {}{}",
            path.display(),
            script.stream_name,
            script.index,
            script.file_name(),
            note,
            if changed {
                ""
            } else {
                "; the workspace already has it"
            }
        );
        if changed && !touched.contains(&script.stream_name) {
            touched.push(script.stream_name.clone());
        }
    }
    if touched.is_empty() {
        return Ok(());
    }
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, touched)
}
