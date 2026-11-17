//! Importing files and logic edits (`--allow-edits`, stage 57): a file into
//! an existing resource, sounds, movies and subtitles of the archives, and
//! wires of the logic graph. As the other writing tools, each call is one
//! undoable operation of the workspace with its diff; the game files are
//! not changed before `apply`.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::import::file::{self, FileOptions, FileTarget};
use doctor_toolkit::logic_graph::{BuildOptions, GraphBuilder, LogicSchema, PinRef, edit, scope};
use doctor_toolkit::resource::audio::parse_sbk;
use doctor_toolkit::resource::audio_encode::{encode_like, replace_sbk_sound};
use doctor_toolkit::resource::video::import::{AudioSource, build_from_files, read_audio_file};
use doctor_toolkit::resource::video::{self, vp6::EncoderConfig};
use doctor_toolkit::workspace::{FileKey, ResourceKey, Workspace};

use super::Output;
use super::create::{game_resource, input_file};
use super::edit::finish;
use crate::state::{ToolError, ToolResult, Toolkit, fail};

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ImportFileParams {
    /// The resource to replace: a key `stream:type:ids` (also of a resource
    /// the workspace added), `<stream>#<index>`, or a one-result query.
    pub resource: String,
    /// The file (an absolute path), read by the resource's type: string
    /// table (lh2) — JSON as `export` writes it (texts by entry hash);
    /// SimGroup, metamodel, variable dictionary, parts database — the JSON
    /// of `export` (values changed in it are taken); navigation graph —
    /// its JSON; dialogue (dlg) — its JSON (the `data` part); Lua — `.lua`
    /// source (compiled for the game) or `.luac`. Textures, models, sounds
    /// and movies have tools of their own.
    pub file: String,
    /// Take the file as the resource's data as stored, any type (checked
    /// only by `validate`).
    #[serde(default)]
    pub raw: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ImportSoundParams {
    /// The sound to replace: a sound of a bank as `<sbk key>#<number>` (the
    /// numbers `export` gives its WAV files), or a streamed sound of the
    /// archives (`audiostreams\e_screen\title_screen_01.exa.snu`; `media`
    /// with `kind: sounds` lists them).
    pub target: String,
    /// The new sound (an absolute path): WAV, or any format ffmpeg reads;
    /// converted to the channels, rate and loop of the sound it replaces.
    pub file: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ImportVideoParams {
    /// The movie to replace: `mainmenu`, `movies/en/load.vp6` (`media` lists them).
    pub movie: String,
    /// New picture (an absolute path): `.y4m` (encoded to VP6), a game
    /// `.vp6`, `.avi`/`.flv` with VP6, other formats through ffmpeg. Without
    /// it the picture stays.
    #[serde(default)]
    pub video: Option<String>,
    /// New sound track (an absolute path): WAV or what ffmpeg reads.
    #[serde(default)]
    pub audio: Option<String>,
    /// Drop the sound track.
    #[serde(default)]
    pub no_audio: Option<bool>,
    /// VP6 quantizer 0..63 (default 52; higher is better and larger).
    #[serde(default)]
    pub quantizer: Option<u8>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditSubtitleParams {
    /// The speech file of the archives (`audiostreams\b3_xxx_0\…exa.snu`;
    /// `media` with `kind: sounds` shows its subtitles), or the sound track
    /// of a cut-scene (`audiostreams\igc16\aud_igc16….exa.snu`; `cutscenes`
    /// with `stream` lists the tracks and their lines).
    pub entry: String,
    /// New texts of a speech file by language column, e.g. `{"en": "Hello"}`.
    #[serde(default)]
    pub texts: std::collections::BTreeMap<String, String>,
    /// A line of a cut-scene's subtitle table to retime (its number in the
    /// list of `cutscenes`); its text is a string of the cut-scene's string
    /// table (`edit_string`).
    #[serde(default)]
    pub line: Option<usize>,
    /// With `line`: the new start in milliseconds from the start of the sound.
    #[serde(default)]
    pub start_ms: Option<u32>,
    /// With `line`: the new length in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<u32>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct LogicConnectParams {
    /// The sending pin: `<stream>#<resource>:<object>:<pin>`, pin names as
    /// `logic_graph` shows them (`send_2`, `on_enter`).
    pub from: String,
    /// The listening pin, written the same way.
    pub to: String,
    /// Remove the wire between the two pins instead.
    #[serde(default)]
    pub disconnect: Option<bool>,
}

// ------------------------------------------------------------ import_file

/// The resource to write: in the game files, or added by the workspace.
fn target(kit: &Toolkit, ws: &Workspace, text: &str) -> ToolResult<FileTarget> {
    let text = text.trim();
    if let Ok(key) = text.parse::<ResourceKey>()
        && ws.change(&key).is_some_and(|c| c.is_added())
    {
        return Ok(FileTarget::Added(key));
    }
    let at = game_resource(kit, text)?;
    Ok(FileTarget::game(&kit.game, at.stream, at.resource)?)
}

fn current(kit: &Toolkit, ws: &Workspace, t: &FileTarget) -> ToolResult<Vec<u8>> {
    Ok(t.current(&kit.game, ws)?)
}

fn record(
    kit: &Toolkit,
    ws: &mut Workspace,
    t: &FileTarget,
    data: Vec<u8>,
    label: &str,
) -> ToolResult<()> {
    Ok(t.record(&kit.game, ws, data, label)?)
}

/// `import_file`: the reading by type is the library's
/// ([`doctor_toolkit::import::file::import_file`]), shared with the editor.
pub fn import_file(kit: &Toolkit, p: ImportFileParams) -> ToolResult<Output> {
    let path = input_file(&p.file)?;
    let mut access =
        kit.write_workspace(&format!("import_file: {} into {}", p.file, p.resource))?;
    let ws = &mut access.ws;
    let t = target(kit, ws, &p.resource)?;
    let since = ws.head();
    let options = FileOptions {
        raw: p.raw.unwrap_or(false),
    };
    let done = file::import_file(&kit.game, ws, &t, &path, &options).map_err(|e| {
        // The tools that do import what this one does not.
        let text = e.to_string();
        if text.contains("have no import from a file by type") {
            ToolError(format!(
                "{text} (textures: import_texture, models: import_model, sounds: import_sound; \
                 `raw: true` takes the data as stored)"
            ))
        } else {
            ToolError(text)
        }
    })?;
    if !done.changed {
        return Ok(Output::Json(json!({
            "changed": false,
            "note": "the file gives what the resource already holds",
        })));
    }
    finish(
        ws,
        since,
        &done.label,
        json!({ "resource": done.resource, "type": done.type_tag, "notes": done.notes }),
    )
}

// ------------------------------------------------------------ sound, movies, subtitles

pub fn import_sound(kit: &Toolkit, p: ImportSoundParams) -> ToolResult<Output> {
    let path = input_file(&p.file)?;
    let pcm = read_audio_file(&path)?;
    if pcm.frames() == 0 {
        return fail(format!("{} holds no audio", path.display()));
    }
    let mut access = kit.write_workspace(&format!("import_sound: {}", p.target))?;
    let ws = &mut access.ws;
    let since = ws.head();
    if let Some((bank, number)) = p.target.trim().rsplit_once('#')
        && let Ok(number) = number.parse::<u32>()
    {
        let t = target(kit, ws, bank)?;
        let data = current(kit, ws, &t)?;
        let sounds = parse_sbk(&data)?;
        let Some(position) = sounds.iter().position(|s| s.index == number) else {
            let have: Vec<String> = sounds.iter().map(|s| s.index.to_string()).collect();
            return fail(format!(
                "the bank has no sound {number} (sounds {})",
                have.join(", ")
            ));
        };
        let old = sounds[position].sample.header;
        let (fitted, sample) = encode_like(&pcm, &old)?;
        let new_bank = replace_sbk_sound(&data, position, &sample)?;
        let seconds = fitted.frames() as f64 / f64::from(old.sample_rate);
        let label = format!(
            "Import sound {number} of {} ({seconds:.2} s)",
            t.key().stream
        );
        record(kit, ws, &t, new_bank, &label)?;
        return finish(
            ws,
            since,
            &label,
            json!({
                "bank": t.key().to_string(),
                "sound": number,
                "channels": old.channels,
                "sample_rate": old.sample_rate,
                "seconds": seconds,
                "was_seconds": f64::from(old.sample_count) / f64::from(old.sample_rate),
            }),
        );
    }
    let key = FileKey::archive_entry(p.target.trim());
    let info = doctor_toolkit::sound::replace_archive_sound(&kit.game, ws, &p.target, &pcm)?;
    let label = format!("Import sound {}", key.name());
    finish(
        ws,
        since,
        &label,
        json!({
            "file": key.name(),
            "channels": info.channels,
            "sample_rate": info.sample_rate,
            "seconds": info.seconds,
        }),
    )
}

pub fn import_video(kit: &Toolkit, p: ImportVideoParams) -> ToolResult<Output> {
    let video_file = p.video.as_deref().map(input_file).transpose()?;
    let audio = match (&p.audio, p.no_audio.unwrap_or(false)) {
        (Some(_), true) => return fail("give `audio` or `no_audio`, not both"),
        (Some(a), false) => AudioSource::File(input_file(a)?),
        (None, true) => AudioSource::None,
        (None, false) => AudioSource::Keep,
    };
    if video_file.is_none() && matches!(audio, AudioSource::Keep) {
        return fail("give `video`, `audio` or `no_audio`");
    }
    let movie = video::find_movie(kit.game.root(), &p.movie)?;
    let key = FileKey::loose(&movie.rel_path);
    let mut access = kit.write_workspace(&format!("import_video: {}", movie.rel_path))?;
    let ws = &mut access.ws;
    let original = ws.current_file(&kit.game, &key)?;
    let mut opts = video::BuildOptions {
        rate: None,
        encoder: EncoderConfig {
            quantizer: p.quantizer.unwrap_or(52).min(63),
            ..EncoderConfig::default()
        },
        progress: None,
    };
    let imported = build_from_files(video_file.as_deref(), &audio, &original, &mut opts)?;
    let info = video::info(&imported.movie)?;
    let label = format!("Import movie {}", movie.rel_path);
    let since = ws.head();
    ws.set_file(&kit.game, &key, imported.movie, &label)?;
    finish(
        ws,
        since,
        &label,
        json!({
            "movie": movie.rel_path,
            "info": serde_json::to_value(&info)?,
            "warnings": imported.warnings,
        }),
    )
}

pub fn edit_subtitle(kit: &Toolkit, p: EditSubtitleParams) -> ToolResult<Output> {
    let key = FileKey::archive_entry(p.entry.trim());
    if let Some(line) = p.line {
        // The times of a line of a cut-scene's subtitle table.
        if !p.texts.is_empty() {
            return fail(
                "`line` retimes a subtitle of a cut-scene; its text is a string of the \
                 cut-scene's string table: change it with edit_string",
            );
        }
        let edit = doctor_toolkit::cutscene::ops::SubtitleEdit {
            line,
            start_ms: p.start_ms,
            duration_ms: p.duration_ms,
        };
        let mut access = kit.write_workspace(&format!("edit_subtitle: {}", key.name()))?;
        let ws = &mut access.ws;
        let since = ws.head();
        let what = key.name().rsplit('\\').next().unwrap_or("").to_owned();
        let done = doctor_toolkit::cutscene::ops::retime(&kit.game, ws, key.name(), &what, &edit)?;
        let label = done.label.clone();
        return finish(ws, since, &label, serde_json::to_value(&done)?);
    }
    if p.start_ms.is_some() || p.duration_ms.is_some() {
        return fail("`start_ms` and `duration_ms` need `line`: the subtitle to retime");
    }
    if p.texts.is_empty() {
        return fail(
            "give `texts` by language, e.g. {\"en\": \"Hello\"}, or `line` with `start_ms` \
             and (or) `duration_ms`",
        );
    }
    let mut access = kit.write_workspace(&format!("edit_subtitle: {}", key.name()))?;
    let ws = &mut access.ws;
    let texts: Vec<(String, String)> = p
        .texts
        .iter()
        .map(|(lang, text)| (lang.clone(), text.clone()))
        .collect();
    let label = format!(
        "Subtitle of {}",
        key.name().rsplit('\\').next().unwrap_or("")
    );
    let since = ws.head();
    doctor_toolkit::sound::set_subtitles(&kit.game, ws, &p.entry, &texts)?;
    finish(
        ws,
        since,
        &label,
        json!({ "file": key.name(), "texts": p.texts }),
    )
}

// ------------------------------------------------------------ logic

/// `<stream>#<resource>:<object>:<pin>`.
fn pin(kit: &Toolkit, text: &str) -> ToolResult<(doctor_toolkit::level::Location, usize, String)> {
    let (object, pin) = text.trim().rsplit_once(':').ok_or_else(|| {
        ToolError(format!(
            "`{text}`: expected <stream>#<resource>:<object>:<pin>"
        ))
    })?;
    let (group, object) = super::parse_object_id(kit, object)?.ok_or_else(|| {
        ToolError(format!(
            "`{text}`: expected <stream>#<resource>:<object>:<pin>"
        ))
    })?;
    Ok((group, object, pin.to_owned()))
}

pub fn logic_connect(kit: &Toolkit, p: LogicConnectParams) -> ToolResult<Output> {
    let (from_group, from_object, from_pin) = pin(kit, &p.from)?;
    let (to_group, to_object, to_pin) = pin(kit, &p.to)?;
    let disconnect = p.disconnect.unwrap_or(false);
    let mut access = kit.write_workspace(&format!("logic_connect: {} -> {}", p.from, p.to))?;
    let ws = &mut access.ws;
    let mut groups = vec![from_group];
    if to_group != from_group {
        groups.push(to_group);
    }
    let (loaded, errors) = scope::load(&kit.game, Some(ws), &groups);
    if let Some((_, e)) = errors.into_iter().next() {
        return Err(ToolError(e.to_string()));
    }
    let mut builder = GraphBuilder::new(LogicSchema::gf2());
    for g in &loaded {
        builder.add_group(g.location, &g.stream, &g.group);
    }
    let graph = builder.build(BuildOptions {
        hub_threshold: 0,
        keep_unwired: true,
        ..BuildOptions::default()
    });
    let find = |group, object, pin: &str| -> ToolResult<PinRef> {
        let node = graph
            .node_of(group, object)
            .ok_or_else(|| ToolError(format!("no object {object} in that SimGroup")))?;
        let pins = &graph.nodes[node].pins;
        let p = pins.iter().position(|p| p.name == pin).ok_or_else(|| {
            let names: Vec<&str> = pins.iter().map(|p| p.name.as_str()).collect();
            ToolError(format!(
                "{} has no pin `{pin}`; pins: {}",
                graph.nodes[node].class_name,
                names.join(", ")
            ))
        })?;
        Ok(PinRef { node, pin: p })
    };
    let a = find(from_group, from_object, &from_pin)?;
    let b = find(to_group, to_object, &to_pin)?;
    let changes = if disconnect {
        let w = graph
            .wires
            .iter()
            .position(|w| w.from == a && w.to == b)
            .ok_or_else(|| ToolError("no wire between these pins".into()))?;
        edit::plan_disconnect(&graph, w)?
    } else {
        edit::plan_connect(&graph, a, b, || {
            let base = format!("dtk_{}_{from_pin}", graph.nodes[a.node].class_name);
            edit::fresh_message(&graph, &base)
        })?
    };
    if changes.is_empty() {
        return Ok(Output::Json(json!({
            "changed": false,
            "note": if disconnect { "not connected" } else { "already connected" },
        })));
    }
    let set: Vec<Value> = changes
        .iter()
        .map(|c| {
            json!({
                "object": super::object_id(kit, c.at.group, c.at.object),
                "attribute": format!(
                    "{}.{}",
                    LogicSchema::gf2().class_name(c.at.class).unwrap_or("?"),
                    c.at.index
                ),
                "value": c.value.as_ref().map(ToString::to_string),
            })
        })
        .collect();
    let label = if disconnect {
        "Disconnect logic"
    } else {
        "Connect logic"
    };
    let since = ws.head();
    edit::apply(&kit.game, kit.resources(), ws, &changes, label)?;
    finish(
        ws,
        since,
        label,
        json!({ "from": p.from, "to": p.to, "set": set }),
    )
}
