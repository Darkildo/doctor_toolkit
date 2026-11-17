//! Editing commands for game logic and Flash interfaces: `logic-import`,
//! `simgroup-set` and `apx-import`. Every edit is recorded in the workspace.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::apx_edit::{self, ApxEdit};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::resource::logic_import::{self, apply_json};
use doctor_toolkit::resource::names::{sim_classes, type_tags};
use doctor_toolkit::resource::simgroup::{Kind, SimGroup, Value};
use doctor_toolkit::resource::simgroup_edit::{self, Transform, describe};
use doctor_toolkit::workspace::Workspace;

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

const SIMGROUP: u32 = sdbm(b"simgroup");

/// Finds a stream by name; returns its index.
fn stream_index(game: &GameInstance, name: &str) -> Result<usize> {
    game.find_stream(name)
        .with_context(|| format!("stream `{name}` not found"))
}

/// Applies edited JSON files written by `logic-export`.
pub fn logic_import(game_dir: &Path, files: &[PathBuf], mode: &EditMode) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, mode)?;
    let mut touched = Vec::new();
    for path in files {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let doc: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", path.display()))?;
        let field = |name: &str| {
            doc.get(name).with_context(|| {
                format!(
                    "{}: missing `{name}`; export the file again with `logic-export`",
                    path.display()
                )
            })
        };
        let stream_name = field("stream")?
            .as_str()
            .context("`stream` must be a string")?;
        let index = field("resource")?
            .as_u64()
            .context("`resource` must be a number")? as usize;
        let id = field("id")?.as_str().context("`id` must be a string")?;
        let stream = stream_index(&game, stream_name)?;
        let header = game
            .open_stream(stream)?
            .layout()
            .resources
            .get(index)
            .map(|r| r.header);
        let header = header.with_context(|| format!("no resource {index} in {stream_name}"))?;
        if format!("{:08x}", header.ids[3]) != id {
            bail!(
                "{}: resource {index} of {stream_name} is not {id}",
                path.display()
            );
        }
        if !logic_import::is_supported(header.type_id) {
            bail!(
                "{}: {} resources cannot be imported; supported: simgroup, metamodel, variabledictionary, partsdb",
                path.display(),
                type_tags().display(header.type_id)
            );
        }
        let data = ws.current_data(&game, stream, index)?;
        let imported = apply_json(header.type_id, &data, &doc)
            .with_context(|| format!("{}", path.display()))?;
        if imported.changes.is_empty() {
            out!("{}: no changes", path.display());
            continue;
        }
        for change in &imported.changes {
            out!("  {change}");
        }
        let label = format!(
            "Import {} into {stream_name} {} {id}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            type_tags().display(header.type_id)
        );
        ws.set_resource(&game, stream, index, imported.data, &label)?;
        out!(
            "{}: {} value(s) changed",
            path.display(),
            imported.changes.len()
        );
        touched.push(stream_name.to_owned());
    }
    if touched.is_empty() {
        return Ok(());
    }
    touched.sort();
    touched.dedup();
    workspace_cmd::finish_edit(&game, &mut ws, mode, touched)
}

/// Arguments of `simgroup-set`.
#[derive(Args)]
pub struct SimGroupSet {
    pub game_dir: PathBuf,
    /// Stream name, e.g. `gf2/globals_global`
    pub stream: String,
    /// Index of the SimGroup resource in the stream (see `list`)
    pub resource: usize,
    /// Index of the object in the SimGroup (as in `logic-export` and the GUI)
    pub object: usize,
    /// Set an attribute: `Class.attribute=value`; class and attribute by name
    /// or as hexadecimal hash and index (`CSystemCommands.CMD_LoadMatrix`,
    /// `b390b11a.2=1`). An empty value resets the attribute to the default
    #[arg(long = "set", value_name = "CLASS.ATTR=VALUE")]
    pub set: Vec<String>,
    /// Move the object: `X,Y,Z` (Y is up)
    #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
    pub position: Option<String>,
    /// Rotate the object: degrees about X, then Y, then Z
    #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
    pub rotation: Option<String>,
    /// Scale the object along its axes
    #[arg(long, value_name = "X,Y,Z")]
    pub scale: Option<String>,
    /// Show another metamodel: its resource id (hexadecimal)
    #[arg(long, value_name = "ID")]
    pub model: Option<String>,
    /// Make the change even if the object's class cannot show its model (the
    /// game may crash, see docs/levels.md)
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub mode: EditMode,
}

fn parse_floats<const N: usize>(text: &str) -> Result<[f32; N]> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    if parts.len() != N {
        bail!("`{text}`: {N} comma-separated numbers expected");
    }
    let mut out = [0.0; N];
    for (o, p) in out.iter_mut().zip(parts) {
        *o = p
            .parse()
            .with_context(|| format!("`{p}` is not a number"))?;
    }
    Ok(out)
}

fn parse_hex(text: &str) -> Result<u32> {
    u32::from_str_radix(text.trim_start_matches("0x"), 16)
        .with_context(|| format!("`{text}` is not a hexadecimal number"))
}

/// Parses `text` for an attribute of `kind`; an unknown kind follows the
/// stored scalar value.
fn parse_value(kind: Option<Kind>, current: Option<&Value>, text: &str) -> Result<Option<Value>> {
    let kind = kind.or(match current {
        Some(Value::Int(_)) => Some(Kind::Int),
        Some(Value::Float(_)) => Some(Kind::Float),
        Some(Value::Hash(_)) => Some(Kind::Hash),
        _ => None,
    });
    match kind {
        Some(kind) => Ok(simgroup_edit::parse_value(kind, text)?),
        None if text.is_empty() => Ok(None),
        None => bail!("the kind of this attribute is not known; use `logic-import`"),
    }
}

/// Sets attributes, the transform or the model of one SimGroup object.
pub fn simgroup_set(args: &SimGroupSet) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let stream = stream_index(&game, &args.stream)?;
    let header = game
        .open_stream(stream)?
        .layout()
        .resources
        .get(args.resource)
        .map(|r| r.header)
        .with_context(|| format!("no resource {} in {}", args.resource, args.stream))?;
    if header.type_id != SIMGROUP {
        bail!(
            "resource {} of {} is not a SimGroup",
            args.resource,
            args.stream
        );
    }
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let data = ws.current_data(&game, stream, args.resource)?;
    let mut group = SimGroup::parse(&data)?;
    let original = group.clone();
    let o = args.object;
    let object = group
        .objects
        .get(o)
        .with_context(|| format!("the SimGroup has {} object(s)", group.objects.len()))?
        .clone();
    let mut changed = 0;
    for item in &args.set {
        let (target, text) = item
            .split_once('=')
            .with_context(|| format!("`{item}`: CLASS.ATTR=VALUE expected"))?;
        let (class, attr) = target
            .split_once('.')
            .with_context(|| format!("`{target}`: CLASS.ATTR expected"))?;
        // The packets of the object as they are now: setting an attribute
        // of a packet shared with other objects gives the object its own.
        let packets = group.objects[o].packets.clone();
        let slot = packets
            .iter()
            .position(|&p| {
                let packet = &group.packets[p];
                packet
                    .class_name
                    .is_some_and(|n| n.eq_ignore_ascii_case(class))
                    || parse_hex(class).is_ok_and(|h| h == packet.class.0)
            })
            .with_context(|| format!("object {o} has no packet of class `{class}`"))?;
        let packet = &group.packets[packets[slot]];
        let class_hash = packet.class.0;
        let schema = doctor_toolkit::resource::simgroup::Schema::gf2();
        let index = match attr.parse::<u16>() {
            Ok(index) => index,
            Err(_) => (0..packet.attr_count)
                .find(|&i| {
                    schema
                        .get(class_hash, i)
                        .and_then(|info| info.name)
                        .is_some_and(|n| n.eq_ignore_ascii_case(attr))
                })
                .with_context(|| format!("class `{class}` has no attribute named `{attr}`"))?,
        };
        let current = packet
            .attrs
            .iter()
            .find(|a| a.index == index)
            .map(|a| a.value.clone());
        let value = parse_value(group.attr_kind(class_hash, index), current.as_ref(), text)
            .with_context(|| format!("`{item}`"))?;
        if group.set_attr(o, slot, index, value.clone())? {
            out!(
                "{target}: {} -> {}",
                describe(current.as_ref()),
                describe(value.as_ref())
            );
            changed += 1;
        }
    }
    if args.position.is_some() || args.rotation.is_some() || args.scale.is_some() {
        let current = group
            .transform(&object)
            .with_context(|| format!("object {o} has no transform (CMD_LoadMatrix)"))?;
        let mut m = current;
        if args.rotation.is_some() || args.scale.is_some() {
            let mut t = Transform::from_matrix(&current);
            if let Some(r) = &args.rotation {
                t.rotation = parse_floats(r)?;
            }
            if let Some(s) = &args.scale {
                t.scale = parse_floats(s)?;
            }
            m = t.to_matrix();
            m[3] = current[3];
        }
        if let Some(p) = &args.position {
            let p: [f32; 3] = parse_floats(p)?;
            m[3][..3].copy_from_slice(&p);
        }
        if group.set_transform(o, m)? {
            out!(
                "transform: {} -> {}",
                describe(Some(&Value::Matrix(current))),
                describe(Some(&Value::Matrix(m)))
            );
            changed += 1;
        }
    }
    if let Some(model) = &args.model {
        let id = parse_hex(model)?;
        let before = group.model(&object);
        if group.set_model(o, Some(id))? {
            out!(
                "model: {} -> {id:08x}",
                before.map_or_else(|| "none".to_owned(), |m| format!("{m:08x}"))
            );
            changed += 1;
        }
    }
    if changed == 0 {
        out!("no change");
        return Ok(());
    }
    if !args.force {
        let (index, _) = game.build_index(|_| {});
        let problems = doctor_toolkit::level::compat::check_edit(
            &game,
            &index,
            Some(&ws),
            Some(&original),
            &group,
        )?;
        if let Some(problem) = problems.first() {
            bail!("{problem} (`--force` makes the change anyway)");
        }
    }
    let class = sim_classes()
        .get(object.class)
        .map_or_else(|| format!("{:08x}", object.class), str::to_owned);
    let label = format!(
        "Edit object {o} ({class}) of SimGroup {:08x} in {}",
        header.ids[3], args.stream
    );
    ws.set_resource(&game, stream, args.resource, group.encode(), &label)?;
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![args.stream.clone()])
}

/// Arguments of `apx-import`.
#[derive(Args)]
pub struct ApxImport {
    pub game_dir: PathBuf,
    /// Stream name, e.g. `gf2/frontend`
    pub stream: String,
    /// The apx file: its title (as written by `apx-export`) or its resource index
    pub apx: String,
    /// Replace the embedded texture with this name (or position, from 0)
    #[arg(long, requires = "image")]
    pub texture: Option<String>,
    /// The new texture image (.png or .dds)
    #[arg(long, requires = "texture")]
    pub image: Option<PathBuf>,
    /// Replace the Apt movie with this `.apt` file
    #[arg(long, value_name = "FILE")]
    pub apt: Option<PathBuf>,
    /// Replace the constant table with this `.const` file
    #[arg(long = "const", value_name = "FILE")]
    pub constants: Option<PathBuf>,
    #[command(flatten)]
    pub mode: EditMode,
}

/// Replaces an embedded texture, the movie or the constants of an apx file.
pub fn apx_import(args: &ApxImport) -> Result<()> {
    if args.texture.is_none() && args.apt.is_none() && args.constants.is_none() {
        bail!("nothing to import: give --texture and --image, --apt or --const");
    }
    let game = GameInstance::open(&args.game_dir)?;
    let stream = stream_index(&game, &args.stream)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let (index, _) = apx_edit::find(&game, &ws, stream, &args.apx)?;
    let done = apx_edit::edit(
        &game,
        &mut ws,
        stream,
        index,
        &ApxEdit {
            texture: args.texture.clone(),
            image: args.image.clone(),
            apt: args.apt.clone(),
            constants: args.constants.clone(),
            text_refs: Default::default(),
        },
    )?;
    for change in &done.changes {
        out!("{}: {change}", done.title);
    }
    if done.operation.is_none() {
        out!("no change: the workspace already has this data");
        return Ok(());
    }
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![args.stream.clone()])
}

/// Data of a resource for export: the workspace version when there is one.
pub fn export_data(
    ws: Option<&Workspace>,
    stream_name: &str,
    file: &doctor_toolkit::game::StreamFile,
    index: usize,
) -> doctor_toolkit::Result<Vec<u8>> {
    if let Some(ws) = ws {
        let key = doctor_toolkit::workspace::ResourceKey::new(
            stream_name,
            &file.layout().resources[index].header,
        );
        if let Some(data) = ws.data(&key)? {
            return Ok(data);
        }
    }
    file.resource_data(index)
}
