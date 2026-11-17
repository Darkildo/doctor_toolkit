//! Exporting (stage 57): resources and the game's movies and streamed sounds
//! as files in the output folder the server was started with
//! (`--output-dir`). Nothing in the game or the workspace changes; the
//! answer lists the files written. The JSON files `export` writes for
//! string tables, SimGroups, metamodels, parts databases, navigation graphs
//! and dialogues are the ones `import_file` (and the command line) read
//! back.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::container::big::normalize_name;
use doctor_toolkit::game::GameAssets;
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::resource::audio::{Sample, parse_sbk, wav};
use doctor_toolkit::resource::graph::{Graph, TYPE_GRAPH};
use doctor_toolkit::resource::hkx::geometry::physics_mesh;
use doctor_toolkit::resource::hkx::physics::Physics;
use doctor_toolkit::resource::mesh::{Model, TYPE_MESH, TYPE_MESH_VOLATILE};
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::resource::texture::{TYPE_TG4D, TYPE_TG4H, Texture};
use doctor_toolkit::resource::{ffn, hkx, lh2, logic, lua, tg4, video};
use doctor_toolkit::scene::{self, ModelFormat};
use doctor_toolkit::workspace::{FileKey, ResourceKey, Workspace};

use super::Output;
use crate::state::{ToolError, ToolResult, Toolkit, fail};

const TYPE_SBK: u32 = sdbm(b"sbk");
const TYPE_METAMODEL: u32 = sdbm(b"metamodel");
const TYPE_PARTSDB: u32 = sdbm(b"partsdb");
const TYPE_APX: u32 = sdbm(b"apx");

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ExportParams {
    /// The resource: a key `stream:type:ids` (also of a resource the
    /// workspace added), `<stream>#<index>`, or a one-result search query.
    pub resource: String,
    /// Format: texture `png` (default) or `dds`; mesh and metamodel `glb`
    /// (default), `gltf`, `obj`, `fbx`, `usd`, `usda`; collision (hkx) `obj`
    /// (default) or `glb`, with its JSON; interface file (apx) `parts`
    /// (default: its movie `.apt`, constants `.const` and embedded textures
    /// as PNG, or as `dds`); parts database `glb` with `set`
    /// (one assembled character); sound bank (sbk) `wav` (one file
    /// per sound); Lua `lua` (decompiled source, default) or `luac`; font
    /// `png` (atlas and glyph JSON); `json` for every type `resource_json`
    /// shows (string tables, SimGroups, metamodels, parts databases,
    /// navigation graphs and dialogues in the form `import_file` reads
    /// back); `raw` for the data as stored, any type.
    #[serde(default)]
    pub format: Option<String>,
    /// File name (without extension, relative to the output folder)
    /// [default: `<stream>/<index>_<type>_<id>`].
    #[serde(default)]
    pub name: Option<String>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
    /// A parts database with format `glb`: assemble this set (name or
    /// number) into one character or vehicle and write it as one model.
    #[serde(default)]
    pub set: Option<String>,
    /// With `set`: choices as `SLOT=VARIANT`, `PRESET=ITEM` (names or
    /// numbers) or `tag=TAG` [default: the first variant of every slot].
    #[serde(default)]
    pub choices: Vec<String>,
    /// With `set`: level of detail (characters) or damage state (vehicles), from 0.
    #[serde(default)]
    pub lod: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct MediaParams {
    /// `movies` (default: the `.vp6` movies) or `sounds` (streamed sounds
    /// and speech of the `.viv` archives).
    #[serde(default)]
    pub kind: Option<String>,
    /// One movie (`mainmenu`, `movies/en/load.vp6`) or sound
    /// (`audiostreams\e_screen\title_screen_01.exa.snu`): its details; without
    /// it a list.
    #[serde(default)]
    pub name: Option<String>,
    /// List only names containing this text.
    #[serde(default)]
    pub filter: Option<String>,
    /// With `name`: write the sound, or the movie's audio track, as WAV
    /// into the output folder.
    #[serde(default)]
    pub wav: Option<bool>,
    /// With a movie `name`: write these frames as PNG into the output folder.
    #[serde(default)]
    pub frames: Vec<usize>,
    /// With a movie `name`: write every key frame as PNG (at most 200).
    #[serde(default)]
    pub key_frames: Option<bool>,
    /// Read the game files only, without the workspace's imported movies and sounds.
    #[serde(default)]
    pub original: Option<bool>,
    /// Skip this many names of the list (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many names (default 100, at most 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}

// ------------------------------------------------------------ resources

/// A resource to export: its key, data and, when it is in the game files,
/// where.
struct Source {
    key: ResourceKey,
    data: Vec<u8>,
    at: Option<(usize, usize)>,
}

fn source(kit: &Toolkit, ws: Option<&Workspace>, text: &str) -> ToolResult<Source> {
    let text = text.trim();
    if let (Some(ws), Ok(key)) = (ws, text.parse::<ResourceKey>())
        && ws.change(&key).is_some_and(|c| c.is_added())
    {
        let data = ws
            .data(&key)?
            .ok_or_else(|| ToolError(format!("{key} has no data in the workspace")))?;
        return Ok(Source {
            key,
            data,
            at: None,
        });
    }
    let at = super::create::game_resource(kit, text)?;
    let file = kit.game.open_stream(at.stream)?;
    let header = file
        .layout()
        .resources
        .get(at.resource)
        .ok_or_else(|| ToolError(format!("the stream has no resource {}", at.resource)))?
        .header;
    let key = ResourceKey::new(&kit.game.streams()[at.stream].stream_name(), &header);
    Ok(Source {
        key,
        data: kit.data(ws, at.stream, at.resource)?,
        at: Some((at.stream, at.resource)),
    })
}

/// Data of the resource of `type_id` and `ids` next to `of`: in the
/// workspace's additions to its stream, else in the game stream.
fn sibling(
    kit: &Toolkit,
    ws: Option<&Workspace>,
    of: &Source,
    type_id: u32,
    ids: [u32; 4],
) -> Option<Vec<u8>> {
    let key = ResourceKey {
        stream: of.key.stream.clone(),
        type_id,
        ids,
    };
    if let Some(ws) = ws
        && let Ok(Some(data)) = ws.data(&key)
    {
        return Some(data);
    }
    let s = kit.game.find_stream(&of.key.stream)?;
    let file = kit.game.open_stream(s).ok()?;
    let index = key.find_in(file.layout())?;
    kit.data(ws, s, index).ok()
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unnamed".into()
    } else {
        cleaned
    }
}

/// Writes `bytes` to `<base>.<ext>` in the output folder; returns the path.
fn write(kit: &Toolkit, base: &str, ext: &str, bytes: &[u8]) -> ToolResult<PathBuf> {
    let path = kit.output_path(&format!("{base}.{ext}"))?;
    std::fs::write(&path, bytes)
        .map_err(|e| ToolError(format!("cannot write {}: {e}", path.display())))?;
    Ok(path)
}

fn pretty(value: &Value) -> ToolResult<Vec<u8>> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(text.into_bytes())
}

/// A model of a mesh (and its meshvolatile next to it).
fn model(
    kit: &Toolkit,
    ws: Option<&Workspace>,
    of: &Source,
    mesh: &[u8],
    ids: [u32; 4],
) -> ToolResult<Model> {
    let volatile = sibling(kit, ws, of, TYPE_MESH_VOLATILE, ids.map(|i| !i)).unwrap_or_default();
    Ok(Model::parse(mesh, &volatile)?)
}

fn write_model(
    kit: &Toolkit,
    model: &Model,
    base: &str,
    format: ModelFormat,
) -> ToolResult<Vec<PathBuf>> {
    let mut assets = GameAssets::new(kit.game.clone(), kit.resources());
    let built = scene::game::model_scene(model, &mut assets)?;
    let path = kit.output_path(&format!("{base}.{}", format.extension()))?;
    Ok(scene::write_file(&built, &path, Some(format))?)
}

pub fn export(kit: &Toolkit, p: ExportParams) -> ToolResult<Output> {
    // Refuse early, before reading anything.
    kit.output_path("probe")?;
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    let src = source(kit, ws.as_ref(), &p.resource)?;
    let type_id = src.key.type_id;
    let tag = type_tags().display(type_id).to_string();
    let base = match &p.name {
        Some(name) => name.trim().to_owned(),
        None => format!(
            "{}/{}_{}_{:08x}",
            src.key.stream,
            src.at
                .map_or_else(|| "new".to_owned(), |a| format!("{:05}", a.1)),
            sanitize(&tag),
            src.key.ids[3]
        ),
    };
    let format = p.format.as_deref().map(str::to_ascii_lowercase);
    let format = format.as_deref();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    match (type_id, format) {
        (_, Some("raw")) => files.push(write(kit, &base, "bin", &src.data)?),
        (TYPE_TG4H | TYPE_TG4D, None | Some("png" | "dds")) => {
            let other = if type_id == TYPE_TG4H {
                TYPE_TG4D
            } else {
                TYPE_TG4H
            };
            let mut partner_ids = src.key.ids;
            partner_ids[1] = other;
            let partner = sibling(kit, ws.as_ref(), &src, other, partner_ids).ok_or_else(|| {
                ToolError("the other half of the texture is not in its stream".into())
            })?;
            let (h, d) = if type_id == TYPE_TG4H {
                (&src.data, &partner)
            } else {
                (&partner, &src.data)
            };
            let texture = Texture {
                header: tg4::Tg4Header::parse(h)?,
                data: d.clone(),
            };
            notes.push(format!(
                "{}: {}×{} {}",
                texture.header.name,
                texture.header.width,
                texture.header.height,
                texture.header.format().name()
            ));
            if format == Some("dds") {
                files.push(write(kit, &base, "dds", &texture.to_dds()?)?);
            } else {
                let layers = if texture.header.kind() == tg4::Kind::Flat {
                    1
                } else {
                    texture.header.layer_count()
                };
                for layer in 0..layers {
                    let name = if layers == 1 {
                        base.clone()
                    } else {
                        format!("{base}_{layer}")
                    };
                    files.push(write(kit, &name, "png", &texture.to_png(layer)?)?);
                }
            }
        }
        (TYPE_MESH, f) if f != Some("json") => {
            let format = model_format(f)?;
            let m = model(kit, ws.as_ref(), &src, &src.data, src.key.ids)?;
            notes.push(format!("{}: {} draw record(s)", m.name, m.records.len()));
            files.extend(write_model(kit, &m, &base, format)?);
        }
        (TYPE_METAMODEL, f) if f != Some("json") => {
            let format = model_format(f)?;
            let meshes = doctor_toolkit::import::collision::mesh_references(&src.data)?;
            for ids in meshes {
                let Some(data) = sibling(kit, ws.as_ref(), &src, TYPE_MESH, ids) else {
                    notes.push(format!(
                        "mesh {:08x} is not in the metamodel's stream",
                        ids[3]
                    ));
                    continue;
                };
                let m = model(kit, ws.as_ref(), &src, &data, ids)?;
                let name = format!("{base}_{}", sanitize(&m.name));
                files.extend(write_model(kit, &m, &name, format)?);
            }
            if files.is_empty() {
                return fail(
                    "the metamodel shows no mesh of its stream; `format: json` exports its parts",
                );
            }
        }
        (TYPE_PARTSDB, Some("glb")) => {
            use doctor_toolkit::character::{Character, MeshLocator, parse_selection};
            let db = doctor_toolkit::resource::partsdb::PartsDb::parse(&src.data)?;
            let Some(set) = p.set.as_deref() else {
                let names: Vec<&str> = db.sets.iter().map(|s| s.name.as_str()).collect();
                return fail(format!(
                    "give `set`: the set of `{}` to assemble. Sets: {}",
                    db.name,
                    names.join(", ")
                ));
            };
            let set = db
                .set_index(set)
                .or_else(|| set.parse().ok().filter(|&i: &usize| i < db.sets.len()))
                .ok_or_else(|| ToolError(format!("no set `{set}` in `{}`", db.name)))?;
            let selection = parse_selection(&db, set, &p.choices)?;
            let near = kit.game.find_stream(&src.key.stream).unwrap_or(0);
            let meshes = MeshLocator::new(kit.resources());
            let character = Character::load(
                &kit.game,
                &meshes,
                near,
                &db,
                set,
                &selection,
                p.lod.unwrap_or(0),
            )?;
            for part in &character.parts {
                notes.push(format!(
                    "{} -> {} ({:08x})",
                    part.name, part.model.name, part.mesh_id
                ));
            }
            for &part in &character.assembly.culled {
                notes.push(format!("{} hidden by a culling tag", db.parts[part].name));
            }
            for (name, id) in &character.missing {
                notes.push(format!("{name}: mesh {id:08x} not found"));
            }
            if character.parts.is_empty() {
                return fail(format!(
                    "set `{}` shows no mesh of the game files; {}",
                    db.sets[set].name,
                    notes.join("; ")
                ));
            }
            let mut assets = GameAssets::new(kit.game.clone(), kit.resources());
            files.push(write(kit, &base, "glb", &character.to_glb(&mut assets)?)?);
        }
        (TYPE_APX, None | Some("parts" | "png" | "dds")) => {
            use doctor_toolkit::resource::apx::{AptFile, TAG_APT, TAG_CONSTANTS};
            let apx = AptFile::parse(&src.data)?;
            if let Some(block) = apx.block(TAG_APT) {
                files.push(write(kit, &base, "apt", &block.data)?);
            }
            if let Some(block) = apx.block(TAG_CONSTANTS) {
                files.push(write(kit, &base, "const", &block.data)?);
            }
            for (position, texture) in apx.textures()?.iter().enumerate() {
                let name = format!("{base}_{position}_{}", sanitize(&texture.header.name));
                if format == Some("dds") {
                    files.push(write(kit, &name, "dds", &texture.to_dds()?)?);
                } else {
                    files.push(write(kit, &name, "png", &texture.to_png(0)?)?);
                }
            }
            notes.push(format!(
                "{}: the movie (.apt), the constants (.const) and the embedded textures; \
                 apx_edit puts an edited part back",
                apx.title().unwrap_or_default()
            ));
        }
        (hkx::TYPE_HKX, None | Some("obj" | "glb")) => {
            let physics = Physics::parse(&src.data)?;
            let mesh = physics_mesh(&physics);
            let name = format!("{:08x}", src.key.ids[3]);
            if format == Some("glb") {
                files.push(write(kit, &base, "glb", &mesh.to_glb(&name))?);
            } else {
                files.push(write(kit, &base, "obj", mesh.to_obj(&name).as_bytes())?);
            }
            files.push(write(
                kit,
                &base,
                "json",
                &pretty(&serde_json::to_value(&physics)?)?,
            )?);
            notes.push(format!("{} triangle(s), model space", mesh.triangles.len()));
        }
        (TYPE_SBK, None | Some("wav")) => {
            for sound in parse_sbk(&src.data)? {
                let name = format!("{base}/{:04}", sound.index);
                files.push(write(kit, &name, "wav", &sound.sample.to_wav()?)?);
            }
            notes.push(format!(
                "{} sound(s); `import_sound` takes `<key>#<number>`",
                files.len()
            ));
        }
        (lua::TYPE_LUA, None | Some("lua")) => {
            let chunk = lua::Chunk::parse(&src.data)?;
            let source = lua::decompile(&chunk).source;
            files.push(write(kit, &base, "lua", source.as_bytes())?);
            notes.push("decompiled Lua 5.0; import_file compiles it back".into());
        }
        (lua::TYPE_LUA, Some("luac")) => files.push(write(kit, &base, "luac", &src.data)?),
        (ffn::TYPE_FFN, None | Some("png")) => {
            let font = ffn::Font::parse(&src.data)?;
            if let Some((s, _)) = src.at {
                let file = kit.game.open_stream(s)?;
                match font.find_atlas(file.data(), file.layout()) {
                    Some(atlas) => files.push(write(kit, &base, "png", &atlas.to_png(0)?)?),
                    None => notes.push("no atlas texture named like the font in its stream".into()),
                }
            }
            files.push(write(
                kit,
                &base,
                "json",
                &pretty(&serde_json::to_value(&font)?)?,
            )?);
        }
        (TYPE_LH2_TEXT, None | Some("json")) => {
            let table = lh2::StringTable::parse(&src.data)?;
            let doc = doctor_toolkit::text::export_json(
                &table,
                &src.key.stream,
                src.at.map_or(0, |a| a.1),
                src.key.ids[3],
            );
            files.push(write(kit, &base, "json", &pretty(&doc)?)?);
        }
        // A cut-scene shot: the camera and the actors as one animation.
        (doctor_toolkit::resource::trinity::TYPE_TRINITY_SEQ_MASTER, Some("glb" | "gltf")) => {
            let Some((stream, resource)) = src.at else {
                return fail("a shot the workspace added has no stream of the game to read");
            };
            let (glb, info) = super::cutscene::shot_glb(kit, ws.as_ref(), stream, resource)?;
            files.push(write(kit, &base, "glb", &glb)?);
            notes.push(info.to_string());
        }
        (TYPE_GRAPH, None | Some("json")) => {
            let graph = Graph::parse(&src.data)?;
            files.push(write(
                kit,
                &base,
                "json",
                &pretty(&serde_json::to_value(&graph)?)?,
            )?);
        }
        (t, None | Some("json")) if logic::is_supported(t) => {
            let mut doc = logic::to_json(t, &src.data).expect("supported")?;
            // Where it is, as `logic-export` writes it.
            if let Value::Object(map) = &mut doc {
                let fields = [
                    ("stream", json!(src.key.stream)),
                    ("resource", json!(src.at.map_or(0, |a| a.1))),
                    ("type", json!(tag)),
                    ("id", json!(format!("{:08x}", src.key.ids[3]))),
                ];
                for (i, (k, v)) in fields.into_iter().enumerate() {
                    map.shift_insert(i, k.to_owned(), v);
                }
            }
            files.push(write(kit, &base, "json", &pretty(&doc)?)?);
        }
        (t, None | Some("json")) if super::json::view_name(t).is_some() => {
            let value = super::json::view(kit, ws.as_ref(), src.at, t, &src.data)?;
            match value {
                super::json::View::Json(v) => files.push(write(kit, &base, "json", &pretty(&v)?)?),
                super::json::View::Source(s) => files.push(write(kit, &base, "lua", s.as_bytes())?),
            }
        }
        (_, None) => {
            files.push(write(kit, &base, "bin", &src.data)?);
            notes.push(format!("{tag} has no other export: its data as stored"));
        }
        (_, Some(f)) => {
            return fail(format!(
                "{tag} resources do not export as `{f}`; see the formats in the description, or `raw`"
            ));
        }
    }
    Ok(Output::Json(json!({
        "resource": src.key.to_string(),
        "type": tag,
        "files": files,
        "notes": notes,
    })))
}

const TYPE_LH2_TEXT: u32 = doctor_toolkit::text::TYPE_LH2;

fn model_format(f: Option<&str>) -> ToolResult<ModelFormat> {
    match f {
        None => Ok(ModelFormat::Glb),
        Some(text) => text
            .parse()
            .map_err(|e| ToolError(format!("`format`: {e}"))),
    }
}

// ------------------------------------------------------------ movies and sounds

pub fn media(kit: &Toolkit, p: MediaParams) -> ToolResult<Output> {
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    let kind = p.kind.as_deref().unwrap_or("movies");
    let filter = p.filter.as_deref().map(str::to_ascii_lowercase);
    let wanted = |name: &str| {
        filter
            .as_ref()
            .is_none_or(|f| name.to_ascii_lowercase().contains(f))
    };
    let offset = p.offset.unwrap_or(0);
    let limit = super::clamp(p.limit, 100, 1000);
    match (kind, &p.name) {
        ("movies", None) => {
            let movies = video::game_movies(kit.game.root())?;
            let list: Vec<Value> = movies
                .iter()
                .filter(|m| wanted(&m.rel_path))
                .map(|m| json!({ "movie": m.rel_path, "bytes": m.len }))
                .collect();
            let total = list.len();
            let (items, more) = crate::output::window(list, offset, limit);
            Ok(Output::Json(
                json!({ "total": total, "more": more, "movies": items }),
            ))
        }
        ("movies", Some(name)) => {
            let movie = video::find_movie(kit.game.root(), name)?;
            let key = FileKey::loose(&movie.rel_path);
            let data = match &ws {
                Some(ws) => ws.current_file(&kit.game, &key)?,
                None => std::fs::read(&movie.path)
                    .map_err(|e| ToolError(format!("cannot read {}: {e}", movie.rel_path)))?,
            };
            let info = video::info(&data)?;
            let stem = sanitize(movie.rel_path.trim_end_matches(".vp6"));
            let mut files = Vec::new();
            if p.wav.unwrap_or(false) {
                match video::decode_audio(&data)? {
                    Some(pcm) => files.push(write(
                        kit,
                        &format!("movies/{stem}"),
                        "wav",
                        &wav(&pcm.samples, pcm.channels, pcm.sample_rate),
                    )?),
                    None => return fail("the movie has no audio track"),
                }
            }
            let mut frames = p.frames.clone();
            if p.key_frames.unwrap_or(false) {
                let parsed = video::Movie::parse(&data)?;
                frames.extend(
                    parsed
                        .frames()
                        .enumerate()
                        .filter(|(_, (k, _))| *k)
                        .map(|(i, _)| i)
                        .take(200),
                );
            }
            if !frames.is_empty() {
                let mut decoded = video::MovieFrames::new(&data[..])?;
                for index in frames {
                    let shown = index.min(decoded.len().saturating_sub(1));
                    let picture = decoded.display(shown)?;
                    files.push(write(
                        kit,
                        &format!("movies/{stem}_{shown:05}"),
                        "png",
                        &picture.to_png()?,
                    )?);
                }
            }
            Ok(Output::Json(json!({
                "movie": movie.rel_path,
                "info": serde_json::to_value(&info)?,
                "files": files,
                "note": "import_video replaces a movie in the workspace",
            })))
        }
        ("sounds", name) => {
            let all = doctor_toolkit::sound::archive_sounds(kit.game.root(), None)?;
            let found: Vec<(String, String)> = all
                .into_iter()
                .filter(|e| match name {
                    Some(n) => normalize_name(&e.name) == normalize_name(&n.replace('/', "\\")),
                    None => wanted(&e.name),
                })
                .map(|e| (e.name, e.archive))
                .collect();
            let Some(name) = name else {
                let total = found.len();
                let list: Vec<String> = found.into_iter().map(|(n, _)| n).collect();
                let (items, more) = crate::output::window(list, offset, limit);
                return Ok(Output::Json(
                    json!({ "total": total, "more": more, "sounds": items }),
                ));
            };
            let Some((entry, _)) = found.into_iter().next() else {
                return fail(format!(
                    "no sound `{name}` in the archives; list them without `name`"
                ));
            };
            let data = doctor_toolkit::sound::archive_sound_data(&kit.game, ws.as_ref(), &entry)?;
            let info = doctor_toolkit::sound::sound_info(&data)?;
            let mut files = Vec::new();
            if p.wav.unwrap_or(false) {
                let stem = sanitize(entry.trim_end_matches(".exa.snu"));
                files.push(write(
                    kit,
                    &format!("sounds/{stem}"),
                    "wav",
                    &Sample::parse_snu(&data)?.to_wav()?,
                )?);
            }
            let subtitles = info.subtitles.as_ref().map(|texts| {
                texts
                    .iter()
                    .map(|(l, t)| json!({ "language": l, "text": t }))
                    .collect::<Vec<_>>()
            });
            Ok(Output::Json(json!({
                "sound": entry,
                "channels": info.channels,
                "sample_rate": info.sample_rate,
                "seconds": info.seconds,
                "subtitles": subtitles,
                "files": files,
                "note": "import_sound replaces it, edit_subtitle changes its subtitles",
            })))
        }
        (other, _) => fail(format!("`kind` is `movies` or `sounds`, not `{other}`")),
    }
}
