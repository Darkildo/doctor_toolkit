//! `live object …`: moving, turning, hiding and showing SimGroup objects
//! in the running game, undo there, and the moves written into the
//! workspace (stage 47, protocol 6).

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::json;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::live::Client;
use doctor_toolkit::live::commit;
use doctor_toolkit::live::names::LiveNames;
use doctor_toolkit::live::proto::{self, EditValue, ObjectEdit, ObjectState, edit};
use doctor_toolkit::workspace::Workspace;

use crate::live_cmd::{LinkArgs, confirm, object_label};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// `live object …`
#[derive(Subcommand)]
pub enum ObjectCommand {
    /// The state of an object: class, matrix, in the world or not, whether
    /// its logic takes messages, whether the link can move it
    Get {
        /// GUID ({…} or 32 hex digits) or a name (needs --game)
        object: String,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Move an object in the running game; its model and collision follow
    /// (a write: needs --yes)
    Move {
        object: String,
        /// New position X,Y,Z
        #[arg(
            long,
            value_name = "X,Y,Z",
            allow_hyphen_values = true,
            conflicts_with = "by"
        )]
        to: Option<String>,
        /// Offset DX,DY,DZ in metres
        #[arg(long, value_name = "DX,DY,DZ", allow_hyphen_values = true)]
        by: Option<String>,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Turn an object about the up axis (a write: needs --yes)
    Rotate {
        object: String,
        /// Facing in degrees (0: +Z, 90: +X), keeping its tilt
        #[arg(long, allow_hyphen_values = true, conflicts_with = "by")]
        yaw: Option<f32>,
        /// Degrees to turn by
        #[arg(long, allow_hyphen_values = true)]
        by: Option<f32>,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Take an object out of the world — model and collision — or with
    /// --logic stop its logic (its listener takes no messages); a write:
    /// needs --yes
    #[command(alias = "hide")]
    Disable {
        object: String,
        #[arg(long)]
        logic: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Bring back what `disable` took out (the link shows only what it
    /// hid), or with --logic start its logic again; a write: needs --yes
    #[command(alias = "show")]
    Enable {
        object: String,
        #[arg(long)]
        logic: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// Undo the latest edit of the link in the game (of OBJECT when given);
    /// a write: needs --yes
    Undo {
        object: Option<String>,
        /// Undo every edit of the link, newest first
        #[arg(long, conflicts_with = "object")]
        all: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
    /// The edits of the link not undone, oldest first
    Edits(LinkArgs),
    /// Write the objects moved in the game into the workspace of --game:
    /// the matrix of each in its SimGroup, one operation (undo with
    /// `workspace undo`); hiding stays in the running game only
    #[command(alias = "to-workspace")]
    Commit {
        /// Workspace folder [default: the game's]
        #[arg(long, value_name = "DIR")]
        workspace_dir: Option<PathBuf>,
        /// Then drop those moves from the module's journal (undo in the game
        /// no longer reaches them)
        #[arg(long)]
        forget: bool,
        #[command(flatten)]
        link: LinkArgs,
    },
}

fn vec3(text: &str) -> Result<[f32; 3]> {
    let parts: Vec<f32> = text
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("`{text}` is not X,Y,Z"))?;
    match parts.as_slice() {
        [x, y, z] => Ok([*x, *y, *z]),
        _ => bail!("`{text}` is not X,Y,Z"),
    }
}

/// The GUID of `text`: a GUID, or the one object whose name contains it.
fn resolve(text: &str, link: &LinkArgs) -> Result<(proto::Guid, Option<LiveNames>)> {
    if let Some(g) = proto::parse_guid(text) {
        return Ok((g, link.names(false)?));
    }
    let names = link
        .names(true)?
        .context("names come from the SimGroups of --game")?;
    let found = names.search(text);
    let exact: Vec<_> = found
        .iter()
        .filter(|(_, o)| o.name.eq_ignore_ascii_case(text))
        .collect();
    let guid = match (exact.as_slice(), found.as_slice()) {
        ([(g, _)], _) | ([], [(g, _)]) => *g,
        ([], []) => bail!("no object of the game's SimGroups is called like `{text}`"),
        _ => bail!(
            "`{text}` names {} objects: give a GUID (`live find {text} --game …` lists them)",
            found.len()
        ),
    };
    Ok((guid, Some(names)))
}

fn need_edits(c: &Client) -> Result<()> {
    if !c.has_edits() {
        bail!(
            "the module in the game speaks protocol {} and does not edit the world: install the module of this toolkit",
            c.version()
        );
    }
    Ok(())
}

fn fmt_matrix(m: &proto::Matrix) -> String {
    format!(
        "({:.3}, {:.3}, {:.3}) yaw {:.1}°",
        m[12],
        m[13],
        m[14],
        edit::yaw(m).to_degrees()
    )
}

fn value_json(v: &EditValue) -> serde_json::Value {
    match v {
        EditValue::Matrix(m) => json!({"matrix": m, "position": [m[12], m[13], m[14]]}),
        EditValue::Flag(f) => json!(f),
    }
}

pub fn edit_json(e: &ObjectEdit, names: Option<&LiveNames>) -> serde_json::Value {
    json!({
        "id": e.id,
        "guid": proto::guid_hex(&e.guid),
        "object": object_label(names, &e.guid),
        "kind": e.kind.name(),
        "before": value_json(&e.before),
        "after": value_json(&e.after),
        "note": e.note,
    })
}

fn print_edit(e: &ObjectEdit, names: Option<&LiveNames>, json_out: bool) {
    if json_out {
        out!("{}", edit_json(e, names));
    } else {
        out!("#{} {}: {}", e.id, object_label(names, &e.guid), e.note);
    }
}

pub fn state_json(s: &ObjectState, names: Option<&LiveNames>) -> serde_json::Value {
    json!({
        "guid": proto::guid_hex(&s.guid),
        "object": object_label(names, &s.guid),
        "class": s.class_name,
        "matrix": s.matrix,
        "position": s.position(),
        "yaw_degrees": s.matrix.map(|m| edit::yaw(&m).to_degrees()),
        "in_world": s.shown(),
        "inactive_count": s.inactive,
        "has_model": s.has_model,
        "logic_on": s.listening,
        "hidden_by_link": s.hidden_by_link,
        "supported": s.supported(),
        "unsupported": (!s.supported()).then_some(&s.unsupported),
    })
}

fn print_state(s: &ObjectState, names: Option<&LiveNames>, json_out: bool) {
    if json_out {
        out!("{}", state_json(s, names));
        return;
    }
    out!("{}", object_label(names, &s.guid));
    out!(
        "  class   {}",
        if s.class_name.is_empty() {
            "(no As of its own)"
        } else {
            &s.class_name
        }
    );
    if let Some(m) = &s.matrix {
        out!("  matrix  {}", fmt_matrix(m));
    }
    match s.inactive {
        Some(0) => out!("  world   in the world (seen, colliding)"),
        Some(n) => out!("  world   out of the world (count {n})"),
        None => {}
    }
    out!("  model   {}", if s.has_model { "yes" } else { "none" });
    out!(
        "  logic   {}",
        if s.listening { "takes messages" } else { "off" }
    );
    if s.hidden_by_link {
        out!("  hidden by the link");
    }
    if !s.supported() {
        out!("  the link does not move or hide it: {}", s.unsupported);
    }
}

pub fn run(command: ObjectCommand) -> Result<()> {
    match command {
        ObjectCommand::Get { object, link } => {
            let (guid, names) = resolve(&object, &link)?;
            let mut c = link.connect()?;
            need_edits(&c)?;
            print_state(&c.object(guid)?, names.as_ref(), link.json);
        }
        ObjectCommand::Move {
            object,
            to,
            by,
            yes,
            link,
        } => {
            if to.is_none() && by.is_none() {
                bail!("give --to X,Y,Z or --by DX,DY,DZ");
            }
            let (guid, names) = resolve(&object, &link)?;
            confirm(yes, "object move")?;
            let mut c = link.connect()?;
            need_edits(&c)?;
            let s = c.object(guid)?;
            let m = s
                .matrix
                .with_context(|| format!("no matrix: {}", s.unsupported))?;
            let target = match (to, by) {
                (Some(t), _) => vec3(&t)?,
                (None, Some(d)) => {
                    let d = vec3(&d)?;
                    [m[12] + d[0], m[13] + d[1], m[14] + d[2]]
                }
                _ => unreachable!(),
            };
            let e = c.object_transform(guid, edit::with_position(&m, target))?;
            print_edit(&e, names.as_ref(), link.json);
        }
        ObjectCommand::Rotate {
            object,
            yaw,
            by,
            yes,
            link,
        } => {
            let (guid, names) = resolve(&object, &link)?;
            confirm(yes, "object rotate")?;
            let mut c = link.connect()?;
            need_edits(&c)?;
            let s = c.object(guid)?;
            let m = s
                .matrix
                .with_context(|| format!("no matrix: {}", s.unsupported))?;
            let turn = match (yaw, by) {
                (Some(y), _) => y.to_radians() - edit::yaw(&m),
                (None, Some(d)) => d.to_radians(),
                _ => bail!("give --yaw DEGREES or --by DEGREES"),
            };
            let e = c.object_transform(guid, edit::rotate_y(&m, turn))?;
            print_edit(&e, names.as_ref(), link.json);
        }
        ObjectCommand::Disable {
            object,
            logic,
            yes,
            link,
        } => switch(&object, false, logic, yes, &link)?,
        ObjectCommand::Enable {
            object,
            logic,
            yes,
            link,
        } => switch(&object, true, logic, yes, &link)?,
        ObjectCommand::Undo {
            object,
            all,
            yes,
            link,
        } => {
            let (target, names) = match object.as_deref() {
                Some(o) => {
                    let (g, n) = resolve(o, &link)?;
                    (Some(g), n)
                }
                None => (None, link.names(false)?),
            };
            confirm(yes, "object undo")?;
            let mut c = link.connect()?;
            need_edits(&c)?;
            loop {
                let e = c.object_undo(target)?;
                print_edit(&e, names.as_ref(), link.json);
                if !all || c.object_edits()?.is_empty() {
                    break;
                }
            }
        }
        ObjectCommand::Edits(link) => {
            let names = link.names(false)?;
            let mut c = link.connect()?;
            need_edits(&c)?;
            let list = c.object_edits()?;
            if link.json {
                out!(
                    "{}",
                    serde_json::Value::Array(
                        list.iter().map(|e| edit_json(e, names.as_ref())).collect()
                    )
                );
            } else if list.is_empty() {
                out!("the link changed nothing in the game (or it was undone)");
            } else {
                for e in &list {
                    print_edit(e, names.as_ref(), false);
                }
            }
        }
        ObjectCommand::Commit {
            workspace_dir,
            forget,
            link,
        } => {
            let dir = link
                .game_dir()
                .context("the workspace belongs to a game folder: add --game DIR")?
                .to_path_buf();
            let mut c = link.connect()?;
            need_edits(&c)?;
            let moves = edit::moved(&c.object_edits()?);
            if moves.is_empty() {
                out!("no object was moved through the link: nothing to write");
                return Ok(());
            }
            let game = GameInstance::open(&dir)?;
            let ws_dir = workspace_dir.unwrap_or_else(|| Workspace::default_dir(game.root()));
            if !Workspace::exists(&ws_dir) {
                bail!(
                    "no workspace in {}: make one with `workspace init`",
                    ws_dir.display()
                );
            }
            crate::workspace_cmd::lock(&ws_dir)?;
            let mut ws = Workspace::open(&ws_dir)?;
            let names = LiveNames::open(&dir, false)
                .with_context(|| format!("reading the SimGroups of {}", dir.display()))?;
            let done = commit::commit_moves(&game, &mut ws, &names, &moves)?;
            if forget {
                for (g, ..) in &done.written {
                    c.object_forget(Some(*g))?;
                }
            }
            if link.json {
                out!(
                    "{}",
                    json!({
                        "written": done.written.iter().map(|(g, l, o, changed)| json!({
                            "guid": proto::guid_hex(g),
                            "object": object_label(Some(&names), g),
                            "stream": game.streams()[l.stream].stream_name(),
                            "resource": l.resource,
                            "index": o,
                            "changed": changed,
                        })).collect::<Vec<_>>(),
                        "skipped": done.skipped.iter().map(|(g, why)| json!({
                            "guid": proto::guid_hex(g),
                            "why": why,
                        })).collect::<Vec<_>>(),
                        "operation": done.operation,
                        "forgot": forget,
                    })
                );
            } else {
                for (g, l, o, changed) in &done.written {
                    out!(
                        "{} -> {}#{}:{}{}",
                        object_label(Some(&names), g),
                        game.streams()[l.stream].stream_name(),
                        l.resource,
                        o,
                        if *changed { "" } else { " (already so)" }
                    );
                }
                for (g, why) in &done.skipped {
                    eprintln!("skipped {}: {why}", object_label(Some(&names), g));
                }
                match &done.operation {
                    Some(op) => out!("workspace: {op} (undo: workspace undo)"),
                    None => out!("workspace: nothing changed"),
                }
            }
        }
    }
    Ok(())
}

fn switch(object: &str, enabled: bool, logic: bool, yes: bool, link: &LinkArgs) -> Result<()> {
    let (guid, names) = resolve(object, link)?;
    confirm(
        yes,
        if enabled {
            "object enable"
        } else {
            "object disable"
        },
    )?;
    let mut c = link.connect()?;
    need_edits(&c)?;
    let e = c.object_enable(guid, enabled, logic)?;
    print_edit(&e, names.as_ref(), link.json);
    Ok(())
}
