//! Making new levels (`--allow-edits`, stage 57): new streams, SimGroups in
//! them, new and cloned resources, imported models, textures and collision.
//! As the other writing tools, every call is one undoable operation of the
//! workspace answered with its diff; nothing reaches the game files before
//! `apply`. Files are read from the paths the client gives.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::container::ustream::{ResourceHeader, Storage};
use doctor_toolkit::create::{self, CloneOptions, IdForm};
use doctor_toolkit::game::GameAssets;
use doctor_toolkit::import::{collision, model};
use doctor_toolkit::level::Location;
use doctor_toolkit::level::edit as level_edit;
use doctor_toolkit::resource::hkx::collision::CollisionKind;
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::resource::simgroup::Guid;
use doctor_toolkit::resource::texture::{self, Texture};
use doctor_toolkit::workspace::key::normalize_stream;
use doctor_toolkit::workspace::{NewResourceEdit, ResourceEdit, ResourceKey, StorageChoice};

use super::edit::{finish, group_ref};
use super::{Output, hex, resolve};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AddStreamParams {
    /// Name of the new stream, e.g. `gf2/doctor/props` (file `gf2/doctor/props.str`).
    pub name: String,
    /// The stream it depends on, loaded before it, e.g. `gf2/cuba`.
    pub parent: String,
    /// Make it a parented stream of `parent` (flags `0,0,5,0`, as mission
    /// streams): the game loads it only after an `EnableDisableStreams`
    /// object enables it. Otherwise (default) it loads when game data asks
    /// for its hash (`StreamSet`) or when the live link loads it
    /// (`stream_load`).
    #[serde(default)]
    pub with_parent: Option<bool>,
    /// With `with_parent`: leave the stream enabled (flags `0,0,1,0`), so
    /// the game opens it with every load of `parent` and unloads it with
    /// the parent. With a world zone as the parent the stream comes and
    /// goes as the player approaches and leaves.
    #[serde(default)]
    pub auto_load: Option<bool>,
    /// Tables of contents to list it in (relative paths), optionally with
    /// another parent there: `gf2/globals_global.toc=gf2/globals_global`
    /// [default: every table that lists the parent].
    #[serde(default)]
    pub tocs: Vec<String>,
    /// 32 hexadecimal digits [default: derived from the name, with an unused hash].
    #[serde(default)]
    pub guid: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AddZoneCellParams {
    /// Root stream of the level whose world grid gets the cell, e.g. `gf2/queens`.
    pub level: String,
    /// The stream to load by the position of the player: one made with
    /// `add_stream` (or any stream when `guid` is given).
    pub stream: String,
    /// Lowest corner of the box of the zone, `[x, y, z]` in metres (Y is up).
    pub min: [f32; 3],
    /// Highest corner of the box.
    pub max: [f32; 3],
    /// Grid to add it to: `high` (default; loaded within about 180 m of the
    /// centre of the box while the memory budget of the grid lasts),
    /// `medium` (450 m), `low` (1800 m) or `map` (the grid of the Don's View map).
    #[serde(default)]
    pub lod: Option<String>,
    /// Name `ZoneStreamEnable` objects find the cell by (their attribute 0).
    #[serde(default)]
    pub name: Option<String>,
    /// Leave the cell disabled until the player is inside the box of a
    /// `ZoneStreamEnable` object with this `name` (as the game's interiors).
    #[serde(default)]
    pub on_enable: Option<bool>,
    /// GUID of the stream, 32 hexadecimal digits [default: that of the new stream].
    #[serde(default)]
    pub guid: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RemoveStreamParams {
    /// A stream the workspace created (`add_stream`); its resources go too.
    pub name: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AddSimGroupParams {
    /// The stream to add it to: a new stream (`add_stream`) or a game stream.
    pub stream: String,
    /// Copy objects of this SimGroup (`<stream>#<resource>` or a resource
    /// key): every one, or those of `objects`. The copies get new GUIDs, and
    /// GUID attributes and links between them follow. Without it the new
    /// SimGroup is empty.
    #[serde(default)]
    pub copy_of: Option<String>,
    /// With `copy_of`: only these objects (their numbers in that SimGroup).
    #[serde(default)]
    pub objects: Option<Vec<usize>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AddResourceParams {
    /// Stream name: a game stream or a new one (`add_stream`).
    pub stream: String,
    /// Resource type: a tag (`lh2`, `tg4h`, `metamodel`, ...) or a hexadecimal id.
    #[serde(rename = "type")]
    pub type_tag: String,
    /// A file with the raw data of the resource (an absolute path).
    pub file: String,
    /// Ids: one hexadecimal id (repeated four times) or four separated by
    /// dots [default: a new unused id].
    #[serde(default)]
    pub ids: Option<String>,
    /// In a game stream: place it before this resource index [default: at the end].
    #[serde(default)]
    pub before: Option<usize>,
    /// Memory policy name or hexadecimal hash [default: the one of the type].
    #[serde(default)]
    pub policy: Option<String>,
    /// Load alignment [default: the one of the type].
    #[serde(default)]
    pub alignment: Option<u32>,
    /// Store RefPack-compressed.
    #[serde(default)]
    pub compress: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CloneResourceParams {
    /// The game stream that holds the resources.
    pub stream: String,
    /// Indices of the resources to clone. A texture header and its data,
    /// a mesh and its meshvolatile are always cloned together.
    pub indices: Vec<usize>,
    /// Put the clones into this stream (a game stream or a new one)
    /// [default: the same stream, after the sources].
    #[serde(default)]
    pub to: Option<String>,
    /// Text the new ids are derived from [default: the target stream name].
    #[serde(default)]
    pub seed: Option<String>,
    /// Point references between the clones at the clones (default true);
    /// false keeps them pointing at the originals.
    #[serde(default)]
    pub rewrite_references: Option<bool>,
    /// Resources of `stream` (indices) to point at the clones: a SimGroup or
    /// metamodel at cloned models, a mesh at cloned materials, a material
    /// at cloned textures.
    #[serde(default)]
    pub retarget: Vec<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CheckRefsParams {
    /// Streams to check, or prefixes of streams (`gf2/cuba`); at most 400 streams.
    pub streams: Vec<String>,
    /// At most this many problems listed (default 100, at most 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ImportModelParams {
    /// The model file (an absolute path): .glb, .gltf, .obj, .fbx, .usd,
    /// .usda, .usdc or .usdz. Y is up, metres.
    pub file: String,
    /// The game model it is built on: a metamodel or mesh as
    /// `<stream>#<resource>`, a resource key, or a one-result search query.
    /// Its vertex formats, materials and collision part are the template.
    pub template: String,
    /// Make a new model set with this name (clones of the template's
    /// metamodel and meshes under new ids). Without it the template's own
    /// meshes are replaced, and everything that shows them changes.
    #[serde(default)]
    pub name: Option<String>,
    /// With `name`: put the new model set into this stream (a new one from
    /// `add_stream`, or a game stream) instead of the template's. Take a
    /// template from a stream loaded before it (its parent chain): the new
    /// model shares materials and textures with the template.
    #[serde(default)]
    pub stream: Option<String>,
    /// Also build collision from the imported geometry: `mesh` (triangles,
    /// static), `hull` (convex hull) or `box`. The template must be a
    /// metamodel with a collision part.
    #[serde(default)]
    pub collision: Option<String>,
    /// Vertex format of every draw record (0–12) instead of the template's.
    #[serde(default)]
    pub format: Option<u8>,
    /// With `name`, in the template's stream: resources of that stream
    /// (indices: SimGroup, metamodel) to point at the new model.
    #[serde(default)]
    pub retarget: Vec<usize>,
    /// Without `name`: replace the model only in the template's stream. By
    /// default every stream with a copy of a replaced mesh (the same ids)
    /// gets the new one too, since the game takes the mesh from whichever
    /// of them it loaded; the answer lists them (`copies`).
    #[serde(default)]
    pub this_stream_only: bool,
    /// Float parameters of the new materials the import makes (it makes one
    /// for each material of the file with a texture of its own), by
    /// material name and parameter name: tint colours of new clothes,
    /// `{"mod_coat": {"g_TintColor0": [0.8, 0.1, 0.1, 1.0]}}`. As many
    /// numbers as the parameter holds.
    #[serde(default)]
    pub material_params:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, Vec<f32>>>,
    /// Only say what would be done.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ImportTextureParams {
    /// The texture: a resource key of its `tg4h` or `tg4d` (from `search`),
    /// or a one-result search query.
    pub texture: String,
    /// The new image (an absolute path): .png or .dds. It is encoded in the
    /// texture's format with mip levels.
    pub file: String,
    /// Replace every copy of the texture: the `tg4h` with the same name
    /// hash in every stream of the game (a model's textures are copied into
    /// the streams that show it), as the Retexture wizard does. One
    /// operation.
    #[serde(default)]
    pub all_copies: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CollisionBuildParams {
    /// The metamodel whose collision is rebuilt: `<stream>#<resource>`, a
    /// resource key or a one-result query. It must have a collision part.
    pub metamodel: String,
    /// `mesh` (default: triangles, static), `hull` or `box`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Geometry from a model file (an absolute path) [default: the first
    /// mesh the metamodel shows, as the workspace has it].
    #[serde(default)]
    pub from: Option<String>,
    /// Only say what would be done.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

// ------------------------------------------------------------ helpers

/// A file the client names: it must exist.
pub(crate) fn input_file(text: &str) -> ToolResult<PathBuf> {
    let path = PathBuf::from(text.trim());
    if !path.is_file() {
        return fail(format!(
            "no file {}: give the absolute path of a file the server can read",
            path.display()
        ));
    }
    Ok(path)
}

fn read_input(path: &Path) -> ToolResult<Vec<u8>> {
    std::fs::read(path).map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))
}

/// A stream name of the game or of the workspace's new streams.
fn any_stream(
    kit: &Toolkit,
    ws: &doctor_toolkit::workspace::Workspace,
    name: &str,
) -> ToolResult<String> {
    let name = normalize_stream(name.trim().trim_end_matches(".str"));
    if name.contains("..") || name.is_empty() {
        return fail(format!("`{name}` is not a stream name"));
    }
    if kit.game.find_stream(&name).is_some() || ws.new_stream(&name).is_some() {
        return Ok(name);
    }
    fail(format!(
        "no stream `{name}` in the game or among the workspace's new streams; \
         list_streams lists the game's, add_stream makes one"
    ))
}

/// A game resource: `<stream>#<resource>`, a key or a one-result query.
pub(crate) fn game_resource(kit: &Toolkit, text: &str) -> ToolResult<Location> {
    let text = text.trim();
    if let Some((stream, resource)) = text.rsplit_once('#')
        && let Ok(resource) = resource.parse()
    {
        return Ok(Location {
            stream: kit.stream(stream)?,
            resource,
        });
    }
    let id = resolve(kit, text)?;
    let index = kit.index();
    Ok(Location {
        stream: kit.stream(index.stream_name(index.stream_of(id)))?,
        resource: index.record(id).resource as usize,
    })
}

fn collision_kind(text: Option<&str>) -> ToolResult<Option<CollisionKind>> {
    text.map(|k| {
        k.parse()
            .map_err(|e| ToolError(format!("`collision`: {e} (mesh, hull or box)")))
    })
    .transpose()
}

pub(crate) fn last_label(ws: &doctor_toolkit::workspace::Workspace) -> String {
    ws.operations()
        .get(ws.head().wrapping_sub(1))
        .map(|op| op.label.clone())
        .unwrap_or_default()
}

// ------------------------------------------------------------ streams

pub fn add_stream(kit: &Toolkit, p: AddStreamParams) -> ToolResult<Output> {
    let mut access = kit.write_workspace(&format!("add_stream: {}", p.name))?;
    let ws = &mut access.ws;
    let parent = normalize_stream(&p.parent);
    kit.stream(&parent)?;
    if p.auto_load == Some(true) && p.with_parent != Some(true) {
        return fail("`auto_load` goes with `with_parent`");
    }
    let mut stream = create::new_stream(
        &kit.game,
        ws,
        &p.name,
        &parent,
        p.with_parent.unwrap_or(false),
        &p.tocs,
        p.guid.as_deref(),
    )?;
    stream.auto_load = p.auto_load.unwrap_or(false);
    let done = json!({
        "stream": stream.name,
        "file": stream.rel_path(),
        "parent": stream.parent,
        "guid": stream.guid,
        "toc_hash": hex(stream.hash),
        "tocs": stream.tocs,
        "loaded": if stream.with_parent && stream.auto_load {
            "with every load of its parent, unloaded with it"
        } else if stream.with_parent {
            "with its parent, once an EnableDisableStreams object enables its hash"
        } else {
            "when game data asks for its hash (StreamSet) or by stream_load"
        },
        "next": "add_simgroup puts a SimGroup into it; import_model with `stream` a model; \
                 add_zone_cell makes the game load it when the player comes near",
    });
    let since = ws.head();
    ws.add_stream(&kit.game, stream)?;
    let label = last_label(ws);
    finish(ws, since, &label, done)
}

pub fn add_zone_cell(kit: &Toolkit, p: AddZoneCellParams) -> ToolResult<Output> {
    use doctor_toolkit::level::world_grid::{self, ZoneCell};
    use doctor_toolkit::resource::wgi;
    let lod_mask = match p.lod.as_deref().unwrap_or("high") {
        "high" => wgi::lod::HIGH,
        "medium" => wgi::lod::MEDIUM,
        "low" => wgi::lod::LOW,
        "map" => wgi::lod::MAP,
        other => return fail(format!("`lod` is high, medium, low or map, not {other}")),
    };
    let on_enable = p.on_enable.unwrap_or(false);
    if on_enable && p.name.is_none() {
        return fail("`on_enable` needs `name`: the name a ZoneStreamEnable object carries");
    }
    let mut access = kit.write_workspace(&format!("add_zone_cell: {}", p.stream))?;
    let ws = &mut access.ws;
    let level = normalize_stream(&p.level);
    kit.stream(&level)?;
    let since = ws.head();
    let added = world_grid::add_zone_cell(
        &kit.game,
        ws,
        &level,
        &ZoneCell {
            stream: p.stream.clone(),
            guid: p.guid.clone(),
            box_min: p.min,
            box_max: p.max,
            lod_mask,
            kind: if on_enable { 5 } else { wgi::KIND_WORLD },
            flags: 0,
            name: p.name.clone(),
        },
    )?;
    let label = last_label(ws);
    finish(
        ws,
        since,
        &label,
        json!({
            "level": level,
            "stream": normalize_stream(&p.stream),
            "toc_hash": hex(added.hash),
            "world_grid_resource": added.resource,
            "cell": added.cell,
            "needs": added.needs,
            "loaded": if on_enable {
                "while the player is inside the box of a ZoneStreamEnable object with this name \
                 and near the cell"
            } else {
                "when the player comes near the centre of the box, unloaded when he leaves"
            },
        }),
    )
}

pub fn remove_stream(kit: &Toolkit, p: RemoveStreamParams) -> ToolResult<Output> {
    let mut access = kit.write_workspace(&format!("remove_stream: {}", p.name))?;
    let ws = &mut access.ws;
    let name = normalize_stream(&p.name);
    let resources = ws.new_stream_resources(&name).len();
    let since = ws.head();
    if !ws.remove_stream(&name)? {
        return fail(format!(
            "{name} is not a stream the workspace created; workspace_status lists them"
        ));
    }
    let label = last_label(ws);
    finish(
        ws,
        since,
        &label,
        json!({ "removed": name, "resources_removed": resources }),
    )
}

pub fn add_simgroup(kit: &Toolkit, p: AddSimGroupParams) -> ToolResult<Output> {
    if p.objects.is_some() && p.copy_of.is_none() {
        return fail("`objects` goes with `copy_of`");
    }
    let taken = kit.object_guids();
    let mut access = kit.write_workspace(&format!("add_simgroup: {}", p.stream))?;
    let ws = &mut access.ws;
    let stream = any_stream(kit, ws, &p.stream)?;
    let from = p
        .copy_of
        .as_deref()
        .map(|g| group_ref(kit, ws, g))
        .transpose()?;
    let since = ws.head();
    let made = level_edit::add_group(
        &kit.game,
        kit.resources(),
        ws,
        &stream,
        from.as_ref().map(|g| (g, p.objects.as_deref())),
        &|g: &Guid| taken.contains(g),
    )?;
    let label = last_label(ws);
    let key = made.key.to_string();
    finish(
        ws,
        since,
        &label,
        json!({
            "simgroup": key,
            "objects": made.objects,
            "object_ids": (0..made.objects).map(|i| format!("{key}:{i}")).collect::<Vec<_>>(),
            "references_rewired": made.rewired,
            "next": "add_object with `group` set to this key places objects in it; \
                     level_objects with the stream lists them",
        }),
    )
}

// ------------------------------------------------------------ resources

pub fn add_resource(kit: &Toolkit, p: AddResourceParams) -> ToolResult<Output> {
    let path = input_file(&p.file)?;
    let data = read_input(&path)?;
    let type_id = create::parse_type(&p.type_tag)?;
    let mut access = kit.write_workspace(&format!("add_resource: {} into {}", p.file, p.stream))?;
    let ws = &mut access.ws;
    let stream = any_stream(kit, ws, &p.stream)?;
    let ids = match &p.ids {
        Some(text) => create::parse_ids(text)?,
        None => {
            let mut used = ws.used_ids(kit.resources());
            let seed = format!("{stream}:{type_id:08x}:{}", path.display());
            used.new_ids(IdForm::for_type(type_id), type_id, &[0; 4], &seed)
        }
    };
    let defaults = create::type_defaults(type_id);
    let mem_policy = match &p.policy {
        Some(text) => create::parse_mem_policy(text)?,
        None => defaults.mem_policy,
    };
    let before = match p.before {
        None => None,
        Some(index) => {
            let Some(s) = kit.game.find_stream(&stream) else {
                return fail(
                    "`before` is for game streams: a new stream holds its resources in the order they were added",
                );
            };
            let file = kit.game.open_stream(s)?;
            let r = file.layout().resources.get(index).ok_or_else(|| {
                ToolError(format!(
                    "{stream} has {} resources",
                    file.layout().resources.len()
                ))
            })?;
            Some(ResourceKey::new(&stream, &r.header))
        }
    };
    let header = ResourceHeader {
        mem_policy,
        alignment: p.alignment.unwrap_or(defaults.alignment),
        type_id,
        ids,
        data_size: 0,
    };
    let key = ResourceKey::new(&stream, &header);
    let label = format!("Add {key} from {}", path.display());
    let since = ws.head();
    ws.add_resources(
        &kit.game,
        &label,
        &[NewResourceEdit {
            stream,
            header,
            data,
            storage: if p.compress.unwrap_or(false) {
                StorageChoice::Refpack
            } else {
                StorageChoice::Raw
            },
            before,
        }],
    )?;
    finish(ws, since, &label, json!({ "added": key.to_string() }))
}

pub fn clone_resource(kit: &Toolkit, p: CloneResourceParams) -> ToolResult<Output> {
    if p.indices.is_empty() {
        return fail("give `indices` of resources to clone");
    }
    let source_index = kit.stream(&p.stream)?;
    let source = kit.game.streams()[source_index].stream_name();
    let mut access =
        kit.write_workspace(&format!("clone_resource: {} of {source}", p.indices.len()))?;
    let ws = &mut access.ws;
    let target = match &p.to {
        Some(to) => any_stream(kit, ws, to)?,
        None => source.clone(),
    };
    let file = kit.game.open_stream(source_index)?;
    let mut used = ws.used_ids(kit.resources());
    let options = CloneOptions {
        seed: p.seed.clone().unwrap_or_else(|| target.clone()),
        rewrite_references: p.rewrite_references.unwrap_or(true),
        storage: None,
    };
    let (clones, remap) = create::clone_resources(&file, &p.indices, &mut used, &options)?;
    // In the same stream the clones follow their sources; in a new stream
    // they go in the order they are added.
    let before = if target == source {
        let last = clones.iter().filter_map(|c| c.source).max().unwrap_or(0);
        file.layout()
            .resources
            .get(last + 1)
            .map(|r| ResourceKey::new(&source, &r.header))
    } else {
        None
    };
    let additions: Vec<NewResourceEdit> = clones
        .iter()
        .map(|c| NewResourceEdit {
            stream: target.clone(),
            header: c.header,
            data: c.data.clone(),
            storage: match c.storage {
                Storage::Raw => StorageChoice::Raw,
                Storage::RefPack => StorageChoice::Refpack,
            },
            before: before.clone(),
        })
        .collect();
    let mut retargeted = Vec::new();
    let mut edits = Vec::new();
    for &index in &p.retarget {
        let r =
            file.layout().resources.get(index).ok_or_else(|| {
                ToolError(format!("{source} has no resource {index} to retarget"))
            })?;
        let key = ResourceKey::new(&source, &r.header);
        let mut data = ws.current_data(&kit.game, source_index, index)?;
        let changed = create::rewrite_references(r.header.type_id, &mut data, &remap)?;
        retargeted.push(json!({ "resource": key.to_string(), "references": changed }));
        if changed > 0 {
            edits.push(ResourceEdit {
                index,
                data,
                storage: StorageChoice::Keep,
            });
        }
    }
    let clone_list: Vec<Value> = clones
        .iter()
        .map(|c| {
            json!({
                "from": c.source.map(|i| ResourceKey::new(&source, &file.layout().resources[i].header).to_string()),
                "clone": ResourceKey::new(&target, &c.header).to_string(),
            })
        })
        .collect();
    drop(file);
    let label = format!(
        "Clone {} resource(s) of {source} into {target}",
        additions.len()
    );
    let since = ws.head();
    let edits = if edits.is_empty() {
        Vec::new()
    } else {
        vec![(source_index, edits)]
    };
    ws.change_resources(&kit.game, &label, &edits, &additions)?;
    finish(
        ws,
        since,
        &label,
        json!({
            "clones": clone_list,
            "renamed_hashes": remap.names.iter().map(|(a, b)| json!([hex(*a), hex(*b)])).collect::<Vec<_>>(),
            "retargeted": retargeted,
        }),
    )
}

pub fn check_refs(kit: &Toolkit, p: CheckRefsParams) -> ToolResult<Output> {
    if p.streams.is_empty() {
        return fail("give `streams`: names or prefixes (the whole game takes minutes)");
    }
    let mut indices = Vec::new();
    for want in &p.streams {
        let want = normalize_stream(want.trim().trim_end_matches(".str"));
        let found: Vec<usize> = kit
            .game
            .streams()
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let name = s.stream_name();
                name == want || name.starts_with(&format!("{want}/"))
            })
            .map(|(i, _)| i)
            .collect();
        if found.is_empty() {
            return fail(format!("no stream `{want}` or below it in the game"));
        }
        indices.extend(found);
    }
    indices.sort_unstable();
    indices.dedup();
    if indices.len() > 400 {
        return fail(format!(
            "{} streams: narrow `streams` to at most 400",
            indices.len()
        ));
    }
    let limit = super::clamp(p.limit, 100, 1000);
    let mut checker = create::ReferenceChecker::new(&kit.game)?;
    let mut problems = Vec::new();
    let mut total = 0;
    for &i in &indices {
        for problem in checker.check(i)? {
            total += 1;
            if problems.len() < limit {
                problems.push(json!({
                    "stream": problem.stream,
                    "resource": problem.resource,
                    "type": type_tags().display(problem.type_id),
                    "unresolved": format!("{:?}", problem.target),
                }));
            }
        }
    }
    Ok(Output::Json(json!({
        "streams_checked": indices.len(),
        "unresolved": total,
        "problems": problems,
        "note": "references of the game files: a resource must be in the stream or in a stream \
                 loaded before it; `validate` checks the workspace's edits and new streams",
    })))
}

// ------------------------------------------------------------ import

pub fn import_model(kit: &Toolkit, p: ImportModelParams) -> ToolResult<Output> {
    let path = input_file(&p.file)?;
    let at = game_resource(kit, &p.template)?;
    let collision = collision_kind(p.collision.as_deref())?;
    if p.stream.is_some() && p.name.is_none() {
        return fail("`stream` goes with `name`: only a new model set goes into another stream");
    }
    let scene = doctor_toolkit::scene::read_file(&path)
        .map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))?;
    let mut access = kit.write_workspace(&format!("import_model: {}", path.display()))?;
    let ws = &mut access.ws;
    let into = p
        .stream
        .as_deref()
        .map(|s| any_stream(kit, ws, s))
        .transpose()?;
    let mut assets = GameAssets::new(kit.game.clone(), kit.resources());
    let mut plan = model::plan(
        &kit.game,
        ws,
        kit.resources(),
        &mut assets,
        at.stream,
        at.resource,
        &scene,
        &model::ImportOptions {
            new_name: p.name.clone(),
            retarget: p.retarget.clone(),
            format: p.format,
            collision,
            this_stream_only: p.this_stream_only,
        },
    )?;
    if let Some(into) = &into {
        plan.put_into(&kit.game, ws, into)?;
    }
    for (material, params) in &p.material_params {
        let new = plan.new_materials();
        let hash = new
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(material.trim()))
            .map(|m| m.name_hash)
            .ok_or_else(|| {
                let names: Vec<&str> = new.iter().map(|m| m.name.as_str()).collect();
                ToolError(format!(
                    "`material_params`: the import makes no material `{material}`; it makes: {}",
                    if names.is_empty() {
                        "none (the file's materials are the game's own)".to_owned()
                    } else {
                        names.join(", ")
                    }
                ))
            })?;
        for (param, values) in params {
            plan.set_material_floats(hash, param, values)?;
        }
    }
    let summary = serde_json::to_value(&plan.summary)?;
    // The new metamodel: what add_object takes as `model`.
    let new_model = plan
        .summary
        .added
        .iter()
        .filter_map(|k| k.parse::<ResourceKey>().ok())
        .find(|k| k.type_id == doctor_toolkit::level::TYPE_METAMODEL)
        .map(|k| hex(k.ids[3]));
    let done = json!({
        "label": plan.label,
        "summary": summary,
        "model": new_model,
        "next": if new_model.is_some() {
            "add_object with `model` set to this id places it"
        } else {
            "everything that shows the template's meshes shows the import"
        },
    });
    if p.dry_run.unwrap_or(false) {
        return Ok(Output::Json(json!({ "dry_run": true, "would": done })));
    }
    let since = ws.head();
    plan.commit(ws, &kit.game)?;
    let label = plan.label.clone();
    finish(ws, since, &label, done)
}

pub fn import_texture(kit: &Toolkit, p: ImportTextureParams) -> ToolResult<Output> {
    let path = input_file(&p.file)?;
    let is_png = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    let is_dds = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dds"));
    if !is_png && !is_dds {
        return fail("the image must be a .png or .dds file");
    }
    let at = game_resource(kit, &p.texture)?;
    let file = kit.game.open_stream(at.stream)?;
    let pair = texture::pairs(file.layout())
        .into_iter()
        .find(|pair| pair.header == at.resource || pair.data == at.resource)
        .ok_or_else(|| ToolError(format!("{} is not a texture (tg4h/tg4d)", p.texture)))?;
    let before = Texture::load(file.data(), file.layout(), pair)?.header;
    drop(file);
    let image = read_input(&path)?;
    let mut access = kit.write_workspace(&format!("import_texture: {}", before.name))?;
    let ws = &mut access.ws;
    let since = ws.head();
    if p.all_copies.unwrap_or(false) {
        let done = doctor_toolkit::retexture::replace_all_copies(
            &kit.game,
            ws,
            kit.resources(),
            before.name_hash,
            &image,
            is_png,
        )?;
        let label = last_label(ws);
        return finish(ws, since, &label, serde_json::to_value(&done)?);
    }
    let after = ws.import_texture(&kit.game, at.stream, pair, &image, is_png)?;
    let h = &after.header;
    let label = last_label(ws);
    finish(
        ws,
        since,
        &label,
        json!({
            "texture": h.name,
            "before": format!("{}×{} {}", before.width, before.height, before.format().name()),
            "after": format!("{}×{} {}, {} mip level(s)", h.width, h.height, h.format().name(), h.mip_count()),
        }),
    )
}

pub fn collision_build(kit: &Toolkit, p: CollisionBuildParams) -> ToolResult<Output> {
    let at = game_resource(kit, &p.metamodel)?;
    let kind = collision_kind(p.kind.as_deref())?.unwrap_or(CollisionKind::Mesh);
    let mut access = kit.write_workspace(&format!("collision_build: {}", p.metamodel))?;
    let ws = &mut access.ws;
    let mesh = match &p.from {
        Some(file) => {
            let path = input_file(file)?;
            let scene = doctor_toolkit::scene::read_file(&path)
                .map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))?;
            collision::mesh_from_gltf(&scene)
        }
        None => collision::mesh_from_metamodel(&kit.game, ws, at.stream, at.resource)?,
    };
    if mesh.triangles.is_empty() {
        return fail("the geometry has no triangles");
    }
    let plan = collision::plan(
        &kit.game,
        ws,
        kit.resources(),
        at.stream,
        at.resource,
        &mesh,
        kind,
    )?;
    let done = json!({ "label": plan.label, "report": serde_json::to_value(&plan.report)? });
    if p.dry_run.unwrap_or(false) {
        return Ok(Output::Json(json!({ "dry_run": true, "would": done })));
    }
    let since = ws.head();
    plan.commit(ws, &kit.game)?;
    let label = plan.label.clone();
    finish(ws, since, &label, done)
}
