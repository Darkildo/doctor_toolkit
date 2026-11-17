//! Starting the game straight in a level, with the game's own launch options.
//!
//! The PC build ignores its command line: it reads the options from
//! `cmd.txt` in the game folder (`0x0067BFA0`, see
//! `docs/analysis/launch-flags.md`). [`StartOptions`] builds that file;
//! [`CmdFile`] writes it before a launch and takes it away after the game
//! exits, so that the next ordinary start shows the menu again.
//!
//! - `-stream FOLDER NAME…` loads the streams `FOLDER\NAME.str` and goes to
//!   the game loop without the menus, a profile or a save (`0x0067CC50`).
//!   The names are given without `.str`: with it the game crashes.
//! - `-playerspawn {GUID}` picks the `PlayerSpawn` object where the player
//!   appears (`0x008111A0`); without it the first `PlayerSpawn` of kind 2
//!   that is loaded wins.
//! - `-windowed`, `-fullscreen`, `-resolution W H` set the window
//!   (`0x0068D010`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::game::GameInstance;
use crate::resource::simgroup::{Guid, SimGroup, Value};
use crate::{Error, Result};

/// The file the game reads its options from, in the game folder.
pub const CMD_FILE: &str = "cmd.txt";

/// Where the toolkit keeps what it wrote into [`CMD_FILE`] (and the file
/// it replaced), inside the game folder.
const STATE_DIR: &str = "_doctor_toolkit";
const WRITTEN: &str = "cmd.txt.written";
const PREVIOUS: &str = "cmd.txt.previous";

/// Class hash of `PlayerSpawn` (`sdbm("playerspawn")`).
pub const CLASS_PLAYER_SPAWN: u32 = 0x30AA_423A;

/// Launch options of the game, written to [`CMD_FILE`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartOptions {
    /// Streams to start in, as paths relative to the game folder without
    /// `.str`, all in one folder: `gf2/cuba`, `mp_mall/mp_mall`. Empty:
    /// the game starts with its menus.
    pub streams: Vec<String>,
    /// The `PlayerSpawn` object to appear at (its GUID as the toolkit prints
    /// it, 32 hexadecimal digits, or `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn: Option<String>,
    /// `Some(true)`: `-windowed`; `Some(false)`: `-fullscreen`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windowed: Option<bool>,
    /// `-resolution W H`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<(u32, u32)>,
}

impl StartOptions {
    /// Whether there is nothing to write: the game starts as usual.
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
            && self.spawn.is_none()
            && self.windowed.is_none()
            && self.resolution.is_none()
    }

    /// The options as the game reads them, `-stream` last: it takes every
    /// word after it.
    pub fn args(&self) -> Result<Vec<String>> {
        let mut args = Vec::new();
        match self.windowed {
            Some(true) => args.push("-windowed".to_owned()),
            Some(false) => args.push("-fullscreen".to_owned()),
            None => {}
        }
        if let Some((w, h)) = self.resolution {
            if w == 0 || h == 0 {
                return Err(Error::InvalidArgument(format!(
                    "resolution {w}x{h}: both sides must be positive"
                )));
            }
            args.extend(["-resolution".to_owned(), w.to_string(), h.to_string()]);
        }
        if let Some(spawn) = &self.spawn {
            if self.streams.is_empty() {
                return Err(Error::InvalidArgument(
                    "a spawn point needs the stream to start in".into(),
                ));
            }
            args.extend(["-playerspawn".to_owned(), spawn_arg(spawn)?]);
        }
        if !self.streams.is_empty() {
            let (folder, names) = split_streams(&self.streams)?;
            args.push("-stream".to_owned());
            args.push(folder);
            args.extend(names);
        }
        Ok(args)
    }

    /// The contents of [`CMD_FILE`]: the options on one line.
    pub fn cmd_txt(&self) -> Result<String> {
        Ok(format!("{}\r\n", self.args()?.join(" ")))
    }
}

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` for a GUID given as the toolkit
/// prints it (16 bytes in file order) or already in that form.
pub fn spawn_arg(text: &str) -> Result<String> {
    let text = text.trim();
    if text.starts_with('{') {
        let guid = parse_braced(text)?;
        return Ok(braced(&guid));
    }
    let hex = text.trim_start_matches("0x");
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidArgument(format!(
            "spawn point `{text}`: expected 32 hexadecimal digits or {{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}}"
        )));
    }
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex digits");
    }
    Ok(braced(&Guid(bytes)))
}

/// A GUID as the game parses it (`0x004D9500`): the first three groups are
/// little-endian numbers (4, 2 and 2 bytes), the last two the bytes in order.
pub fn braced(guid: &Guid) -> String {
    let b = guid.0;
    format!(
        "{{{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        b[3],
        b[2],
        b[1],
        b[0],
        b[5],
        b[4],
        b[7],
        b[6],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

/// The reverse of [`braced`].
pub fn parse_braced(text: &str) -> Result<Guid> {
    let bad = || {
        Error::InvalidArgument(format!(
            "`{text}` is not {{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}}"
        ))
    };
    let inner = text
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .ok_or_else(bad)?;
    let groups: Vec<&str> = inner.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    if groups.len() != 5
        || groups
            .iter()
            .zip(lengths)
            .any(|(g, n)| g.len() != n || !g.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(bad());
    }
    let digits: String = groups.concat();
    let byte = |i: usize| u8::from_str_radix(&digits[2 * i..2 * i + 2], 16).expect("hex digits");
    let mut b = [0u8; 16];
    // Digits in text order 0..16 map to bytes 3 2 1 0, 5 4, 7 6, 8..16.
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    for (i, &at) in order.iter().enumerate() {
        b[at] = byte(i);
    }
    Ok(Guid(b))
}

/// Splits `gf2/cuba`, `gf2/cuba/local` into the folder they share and the
/// names the game wants after it (`gf2`; `cuba`, `cuba\local`), with
/// backslashes and without `.str`. The game loads `FOLDER\NAME.str`.
fn split_streams(streams: &[String]) -> Result<(String, Vec<String>)> {
    let mut paths: Vec<Vec<String>> = Vec::new();
    for stream in streams {
        let clean = stream.trim().replace('\\', "/");
        let clean = clean.trim_matches('/');
        let lower = clean.to_ascii_lowercase();
        let clean = if lower.ends_with(".str") {
            &clean[..clean.len() - 4]
        } else {
            clean
        };
        if clean.is_empty()
            || clean
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '*' | '"' | '\'' | '#'))
            || clean
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(Error::InvalidArgument(format!(
                "stream `{stream}`: expected a path like gf2/cuba, without spaces, quotes, `*` or `#`"
            )));
        }
        if !clean.contains('/') {
            return Err(Error::InvalidArgument(format!(
                "stream `{stream}`: give its folder too, e.g. gf2/cuba or mp_mall/mp_mall"
            )));
        }
        paths.push(clean.split('/').map(str::to_owned).collect());
    }
    let first = paths
        .first()
        .ok_or_else(|| Error::InvalidArgument("no stream to start in".into()))?;
    // The folder: the leading parts every stream has, leaving each a name.
    let mut shared = first.len() - 1;
    for path in &paths[1..] {
        shared = shared.min(path.len() - 1);
        shared = (0..shared)
            .find(|&i| !path[i].eq_ignore_ascii_case(&first[i]))
            .unwrap_or(shared);
    }
    if shared == 0 {
        return Err(Error::InvalidArgument(format!(
            "the streams of one start must share a folder: {}",
            streams.join(", ")
        )));
    }
    let folder = first[..shared].join("\\");
    let names = paths.iter().map(|p| p[shared..].join("\\")).collect();
    Ok((folder, names))
}

/// A `PlayerSpawn` object: where the player can appear.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpawnPoint {
    /// The stream that holds it (`gf2/cuba`).
    pub stream: String,
    pub guid: Guid,
    /// `PlayerSpawn` attribute 0: 2 marks the default point of a stream
    /// (the game takes the first one it loads when none is chosen).
    pub kind: Option<i32>,
    /// Attribute 1, the message the point sends, e.g. `checkpoint_001_load`.
    pub message: Option<String>,
    pub position: Option<[f32; 3]>,
    /// The direction the player faces (x, z).
    pub facing: Option<[f32; 2]>,
    /// Index of the SimGroup resource in the stream that holds the point.
    pub resource: usize,
    /// Index of the point among the objects of that SimGroup.
    pub object: usize,
}

impl SpawnPoint {
    /// The argument of `-playerspawn`.
    pub fn arg(&self) -> String {
        braced(&self.guid)
    }
}

/// The `PlayerSpawn` objects of stream `stream` (a stream name or path,
/// e.g. `gf2/cuba`), in file order.
pub fn spawn_points(game: &GameInstance, stream: &str) -> Result<Vec<SpawnPoint>> {
    const SIMGROUP: u32 = crate::hash::sdbm(b"simgroup");
    let index = game
        .find_stream(stream)
        .ok_or_else(|| Error::InvalidArgument(format!("no stream `{stream}`")))?;
    let name = game.streams()[index].stream_name().replace('\\', "/");
    let file = game.open_stream(index)?;
    let mut points = Vec::new();
    for (i, entry) in file.layout().resources.iter().enumerate() {
        if entry.header.type_id != SIMGROUP {
            continue;
        }
        let group = SimGroup::parse(&file.resource_data(i)?)?;
        for (number, object) in group
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.class == CLASS_PLAYER_SPAWN)
        {
            let kind = match group.attr(object, CLASS_PLAYER_SPAWN, 0) {
                Some(Value::Int(k)) => Some(*k),
                _ => None,
            };
            let message = match group.attr(object, CLASS_PLAYER_SPAWN, 1) {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            };
            let matrix = group.transform(object);
            points.push(SpawnPoint {
                stream: name.clone(),
                guid: object.guid,
                kind,
                message,
                position: matrix.map(|m| [m[3][0], m[3][1], m[3][2]]),
                facing: matrix.map(|m| [m[2][0], m[2][2]]),
                resource: i,
                object: number,
            });
        }
    }
    Ok(points)
}

/// Streams the game can start in with `-stream`: the top-level stream of
/// every level, `FOLDER/NAME` next to its table of contents (`gf2/cuba`,
/// `mp_mall/mp_mall`), without the `_global` streams and the menus
/// (`gf2/frontend`). The cities of the story come first.
pub fn level_streams(game: &GameInstance) -> Vec<String> {
    let mut names: Vec<String> = game
        .streams()
        .iter()
        .map(|s| s.stream_name().replace('\\', "/"))
        .filter(|name| {
            let parts: Vec<&str> = name.split('/').collect();
            parts.len() == 2 && !parts[1].ends_with("_global") && parts[1] != "frontend"
        })
        .collect();
    let story = |n: &str| match n {
        "gf2/cuba" => 0,
        "gf2/miami" => 1,
        "gf2/queens" => 2,
        _ if n.starts_with("gf2/") => 3,
        _ => 4,
    };
    names.sort_by(|a, b| story(a).cmp(&story(b)).then_with(|| a.cmp(b)));
    names.dedup();
    names
}

/// The top-level stream a zone belongs to: `gf2/cuba` for
/// `gf2/cuba/local/high/business_019` (the first two parts of the path).
/// `-stream` loads only top-level streams; their zones come with them.
pub fn top_level_stream(stream: &str) -> Option<String> {
    let clean = stream.trim().replace('\\', "/").to_ascii_lowercase();
    let clean = clean.strip_suffix(".str").unwrap_or(&clean);
    let mut parts = clean.split('/').filter(|p| !p.is_empty());
    let (folder, name) = (parts.next()?, parts.next()?);
    Some(format!("{folder}/{name}"))
}

/// The point a new one is copied from: the default point of the stream
/// that sends no message, else any point without a message (a point with a
/// message would start its checkpoint again).
pub fn spawn_template(points: &[SpawnPoint]) -> Option<&SpawnPoint> {
    points
        .iter()
        .find(|p| p.kind == Some(2) && p.message.is_none())
        .or_else(|| points.iter().find(|p| p.message.is_none()))
}

/// A new `PlayerSpawn` at `at`: a copy of `template` recorded in the
/// workspace (one operation). The game has it once the workspace is given
/// to it, so a launch at the point applies the workspace first. Returns the
/// index of the copy in its SimGroup and its GUID.
pub fn copy_spawn(
    game: &GameInstance,
    ws: &mut crate::workspace::Workspace,
    template: &SpawnPoint,
    at: [f32; 3],
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<(usize, Guid)> {
    let stream = game
        .find_stream(&template.stream)
        .ok_or_else(|| Error::InvalidArgument(format!("no stream {}", template.stream)))?;
    let from = template.position.unwrap_or(at);
    let offset = [at[0] - from[0], at[1] - from[1], at[2] - from[2]];
    let group = crate::level::Location {
        stream,
        resource: template.resource,
    };
    crate::level::edit::duplicate(game, ws, group, template.object, offset, taken)
}

/// The point of `points` nearest to `at` (with its distance in metres):
/// where "Play from here" starts.
pub fn nearest_spawn(points: &[SpawnPoint], at: [f32; 3]) -> Option<(&SpawnPoint, f32)> {
    points
        .iter()
        .filter(|p| p.message.is_none())
        .filter_map(|p| {
            let pos = p.position?;
            let d = (0..3).map(|i| (pos[i] - at[i]).powi(2)).sum::<f32>().sqrt();
            Some((p, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// [`CMD_FILE`] written for one launch.
///
/// [`CmdFile::write`] keeps a copy of what it wrote (and of a `cmd.txt` that
/// was there before) under `_doctor_toolkit/`; [`CmdFile::restore`] — or
/// [`restore_stale`] after the toolkit was stopped while the game ran —
/// removes the file again or puts the earlier one back, unless someone
/// changed it in between.
#[derive(Debug)]
pub struct CmdFile {
    root: PathBuf,
    restored: bool,
}

impl CmdFile {
    /// Writes `options` to `root/cmd.txt`.
    pub fn write(root: &Path, options: &StartOptions) -> Result<Self> {
        let text = options.cmd_txt()?;
        restore_stale(root)?;
        let state = root.join(STATE_DIR);
        std::fs::create_dir_all(&state).map_err(|e| Error::io(&state, e))?;
        let target = root.join(CMD_FILE);
        if let Ok(previous) = std::fs::read(&target) {
            crate::atomic::write(&state.join(PREVIOUS), &previous)?;
        }
        crate::atomic::write(&state.join(WRITTEN), text.as_bytes())?;
        crate::atomic::write(&target, text.as_bytes())?;
        Ok(Self {
            root: root.to_path_buf(),
            restored: false,
        })
    }

    /// The written file.
    pub fn path(&self) -> PathBuf {
        self.root.join(CMD_FILE)
    }

    /// Removes the file (or puts back the one it replaced). Returns whether
    /// anything was done; see [`restore_stale`].
    pub fn restore(mut self) -> Result<bool> {
        self.restored = true;
        restore_stale(&self.root)
    }
}

impl Drop for CmdFile {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore_stale(&self.root);
        }
    }
}

/// Takes away a [`CMD_FILE`] the toolkit wrote for an earlier launch: puts
/// back the file it replaced, or removes it. A `cmd.txt` that differs from
/// what the toolkit wrote is someone else's and stays. Returns whether the
/// game folder changed.
pub fn restore_stale(root: &Path) -> Result<bool> {
    let state = root.join(STATE_DIR);
    let written_path = state.join(WRITTEN);
    let Ok(written) = std::fs::read(&written_path) else {
        return Ok(false);
    };
    let target = root.join(CMD_FILE);
    let previous_path = state.join(PREVIOUS);
    let mut changed = false;
    if std::fs::read(&target).ok().as_deref() == Some(written.as_slice()) {
        match std::fs::read(&previous_path) {
            Ok(previous) => crate::atomic::write(&target, &previous)?,
            Err(_) => std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?,
        }
        changed = true;
    }
    for path in [&written_path, &previous_path] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(path, e)),
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests;
