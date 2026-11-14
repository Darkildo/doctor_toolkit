//! Importing files and logic edits (`--allow-edits`, stage 57): a file into
//! an existing resource, sounds, movies and subtitles of the archives, and
//! wires of the logic graph. As the other writing tools, each call is one
//! undoable operation of the workspace with its diff; the game files are
//! not changed before `apply`.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::level::navgraph::{GraphRef, save_graph};
use doctor_toolkit::logic_graph::{BuildOptions, GraphBuilder, LogicSchema, PinRef, edit, scope};
use doctor_toolkit::resource::audio::{Sample, parse_sbk};
use doctor_toolkit::resource::audio_encode::{encode_like, encode_snu_like, replace_sbk_sound};
use doctor_toolkit::resource::dlg::Dialogue;
use doctor_toolkit::resource::graph::{Graph, TYPE_GRAPH};
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::logic_import;
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::resource::video::import::{AudioSource, build_from_files, read_audio_file};
use doctor_toolkit::resource::video::{self, vp6::EncoderConfig};
use doctor_toolkit::resource::{dlg, lua, speech};
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
    /// `media` with `kind: sounds` shows its subtitles).
    pub entry: String,
    /// New texts by language column, e.g. `{"en": "Hello"}`.
    pub texts: std::collections::BTreeMap<String, String>,
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
enum Target {
    Game(usize, usize, ResourceKey),
    Added(ResourceKey),
}

impl Target {
    fn key(&self) -> &ResourceKey {
        match self {
            Self::Game(_, _, k) | Self::Added(k) => k,
        }
    }
}

fn target(kit: &Toolkit, ws: &Workspace, text: &str) -> ToolResult<Target> {
    let text = text.trim();
    if let Ok(key) = text.parse::<ResourceKey>()
        && ws.change(&key).is_some_and(|c| c.is_added())
    {
        return Ok(Target::Added(key));
    }
    let at = game_resource(kit, text)?;
    let file = kit.game.open_stream(at.stream)?;
    let header = file
        .layout()
        .resources
        .get(at.resource)
        .ok_or_else(|| ToolError(format!("the stream has no resource {}", at.resource)))?
        .header;
    let key = ResourceKey::new(&kit.game.streams()[at.stream].stream_name(), &header);
    Ok(Target::Game(at.stream, at.resource, key))
}

fn current(kit: &Toolkit, ws: &Workspace, t: &Target) -> ToolResult<Vec<u8>> {
    Ok(match t {
        Target::Game(s, r, _) => ws.current_data(&kit.game, *s, *r)?,
        Target::Added(k) => ws
            .data(k)?
            .ok_or_else(|| ToolError(format!("{k} has no data in the workspace")))?,
    })
}

fn record(
    kit: &Toolkit,
    ws: &mut Workspace,
    t: &Target,
    data: Vec<u8>,
    label: &str,
) -> ToolResult<()> {
    match t {
        Target::Game(s, r, _) => {
            ws.set_resource(&kit.game, *s, *r, data, label)?;
        }
        Target::Added(k) => {
            ws.change_resources_and_added(&kit.game, label, &[], &[], &[(k.clone(), Some(data))])?;
        }
    }
    Ok(())
}

fn read_json(path: &std::path::Path) -> ToolResult<Value> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| ToolError(format!("{} is not JSON: {e}", path.display())))
}

pub fn import_file(kit: &Toolkit, p: ImportFileParams) -> ToolResult<Output> {
    let path = input_file(&p.file)?;
    let mut access =
        kit.write_workspace(&format!("import_file: {} into {}", p.file, p.resource))?;
    let ws = &mut access.ws;
    let t = target(kit, ws, &p.resource)?;
    let key = t.key().clone();
    let tag = type_tags().display(key.type_id).to_string();
    let label = format!(
        "Import {} into {key}",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    let data = current(kit, ws, &t)?;
    let since = ws.head();
    let mut notes: Vec<String> = Vec::new();
    let new = if p.raw.unwrap_or(false) {
        std::fs::read(&path)
            .map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))?
    } else {
        match key.type_id {
            doctor_toolkit::text::TYPE_LH2 => {
                let mut table = StringTable::parse(&data)?;
                let changed = doctor_toolkit::text::import_json(&mut table, &read_json(&path)?)?;
                notes.push(format!("{changed} string(s) changed"));
                table.to_bytes()?
            }
            TYPE_GRAPH => {
                let mut graph: Graph = serde_json::from_value(read_json(&path)?)
                    .map_err(|e| ToolError(format!("{} is not a graph: {e}", path.display())))?;
                let old = Graph::parse(&data)?;
                if graph.guid != old.guid {
                    return fail(format!(
                        "the file is graph {}, not {} ({key})",
                        graph.guid, old.guid
                    ));
                }
                notes.extend(graph.repair_derived());
                if let Target::Game(s, r, _) = &t {
                    // Links of other graphs into this one follow it.
                    let g = GraphRef {
                        stream: *s,
                        resource: *r,
                        graph,
                    };
                    save_graph(&kit.game, ws, &g, None, &label)?;
                    return finish(
                        ws,
                        since,
                        &label,
                        json!({ "resource": key.to_string(), "notes": notes }),
                    );
                }
                graph.to_bytes()?
            }
            lua::TYPE_LUA => {
                let bytes = std::fs::read(&path)
                    .map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))?;
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("luac"))
                {
                    lua::Chunk::parse(&bytes)?;
                    bytes
                } else {
                    let old = lua::Chunk::parse(&data)?;
                    let name = old.main.source.clone().unwrap_or_default();
                    let chunk = lua::compile(&bytes, &name, old.number_size).map_err(|e| {
                        ToolError(format!("{}:{}: {}", path.display(), e.line, e.message))
                    })?;
                    notes.push(format!(
                        "{} function(s), {} instruction(s)",
                        chunk.main.function_count(),
                        chunk.main.instruction_count()
                    ));
                    chunk.to_bytes()
                }
            }
            dlg::TYPE_DLG => {
                let doc = read_json(&path)?;
                let part = doc.get("data").cloned().unwrap_or(doc);
                let d: Dialogue = serde_json::from_value(part)
                    .map_err(|e| ToolError(format!("{}: not a dialogue: {e}", path.display())))?;
                d.to_bytes()?
            }
            t if logic_import::is_supported(t) => {
                let imported = logic_import::apply_json(t, &data, &read_json(&path)?)?;
                notes.extend(imported.changes.iter().map(ToString::to_string));
                imported.data
            }
            _ => {
                return fail(format!(
                    "{tag} resources have no import from a file here; textures: import_texture, \
                     models: import_model, sounds: import_sound; `raw: true` takes the data as stored"
                ));
            }
        }
    };
    if new == data {
        return Ok(Output::Json(json!({
            "changed": false,
            "note": "the file gives what the resource already holds",
        })));
    }
    record(kit, ws, &t, new, &label)?;
    finish(
        ws,
        since,
        &label,
        json!({ "resource": key.to_string(), "type": tag, "notes": notes }),
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
    let original = ws.current_file(&kit.game, &key)?;
    let data = encode_snu_like(&pcm, &original)?;
    let header = Sample::parse_snu(&data)?.header;
    let label = format!("Import sound {}", key.name());
    ws.set_file(&kit.game, &key, data, &label)?;
    finish(
        ws,
        since,
        &label,
        json!({
            "file": key.name(),
            "channels": header.channels,
            "sample_rate": header.sample_rate,
            "seconds": f64::from(header.sample_count) / f64::from(header.sample_rate),
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
    if p.texts.is_empty() {
        return fail("give `texts` by language, e.g. {\"en\": \"Hello\"}");
    }
    let key = FileKey::archive_entry(p.entry.trim());
    let mut access = kit.write_workspace(&format!("edit_subtitle: {}", key.name()))?;
    let ws = &mut access.ws;
    let data = ws.current_file(&kit.game, &key)?;
    let mut table = speech::subtitles(&data)?
        .ok_or_else(|| ToolError(format!("{} has no subtitles", key.name())))?;
    for (lang, text) in &p.texts {
        let col = speech::language_column(&table, lang).ok_or_else(|| {
            ToolError(format!(
                "no `{lang}` column ({})",
                table.languages.join(" ")
            ))
        })?;
        table.strings[col][0] = text.clone();
    }
    let new = speech::with_subtitles(&data, &table)?;
    let label = format!(
        "Subtitle of {}",
        key.name().rsplit('\\').next().unwrap_or("")
    );
    let since = ws.head();
    ws.set_file(&kit.game, &key, new, &label)?;
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
