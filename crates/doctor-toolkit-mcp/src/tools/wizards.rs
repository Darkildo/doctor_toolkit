//! What the editor's wizards do, as tools (stage 104): game parameters of
//! the curated list, a new voice line, a new part of clothes and the sets
//! that wear it, parameters of a material. Each tool is a thin wrapper: the
//! operation itself is a function of the library that the wizard and the
//! command line call too (`doctor_toolkit::{tunables, voice, outfit,
//! retexture}`).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::outfit::{self, AllowVariant, NewPart};
use doctor_toolkit::resource::video::import::read_audio_file;
use doctor_toolkit::tunables::{self, SetTunable};
use doctor_toolkit::voice::{self, NewVoiceLine};
use doctor_toolkit::workspace::{ResourceKey, Workspace};

use super::create::{game_resource, input_file};
use super::edit::finish;
use super::{Output, clamp};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct TunablesParams {
    /// Only this category: weapons, health, player, crew, economy, police,
    /// vehicles, world.
    #[serde(default)]
    pub category: Option<String>,
    /// Only parameters whose id, name or description contains this text.
    #[serde(default)]
    pub filter: Option<String>,
    /// One parameter by id: also the value of each of its objects.
    #[serde(default)]
    pub id: Option<String>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
    /// Skip this many parameters (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many parameters (default 60, at most 300).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SetTunablesParams {
    /// The parameters to change, each with `value` or `multiply`.
    pub changes: Vec<SetTunable>,
    /// Only say what would change.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct VoiceAddParams {
    /// The chatter set (`chv`) of the speaker, as a resource key
    /// (`speech` with `show: "speakers"` lists them).
    pub speaker: String,
    /// The recording (an absolute path): WAV, or any format ffmpeg reads;
    /// converted to the format of the speaker's lines (mono, 32 kHz).
    pub file: String,
    /// Speech event the line belongs to (`F2F_ENJOY_FLIGHT`); a new name
    /// makes a new event with its record in the event table.
    pub event: String,
    /// Line name: lower-case letters, digits and `_` (`doc1_xxx_0000001`);
    /// the sound becomes `d_<name>.exa.snu`.
    pub name: String,
    /// Subtitles by language code (`en`, `cs`, `nl`, `pl`, `ru`, `es`);
    /// other languages get the English text.
    #[serde(default)]
    pub subtitles: BTreeMap<String, String>,
    /// Weight among the lines of the event (default 1).
    #[serde(default)]
    pub weight: Option<f32>,
    /// Remove the other lines of the event: it always says the new one.
    #[serde(default)]
    pub only: Option<bool>,
    /// Mix category by name (`DIALOG`, `CHATTER`) [default: that of the
    /// speaker's lines].
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PartsdbAddPartParams {
    /// The parts database: a resource key, `<stream>#<index>` or a
    /// one-result search query (`type:partsdb player`).
    pub database: String,
    #[serde(flatten)]
    pub part: NewPart,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PartsdbAllowParams {
    /// The parts database: a resource key, `<stream>#<index>` or a
    /// one-result search query (`type:partsdb player`).
    pub database: String,
    #[serde(flatten)]
    pub allow: AllowVariant,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SetMaterialParams {
    /// The material: a resource key (also of a material the workspace
    /// added, as `import_model` lists them), `<stream>#<index>` or a
    /// one-result search query.
    pub material: String,
    /// New values of float parameters by name, as many numbers as the
    /// parameter holds (`resource_json` of the material shows them):
    /// `{"g_TintColor0": [0.8, 0.1, 0.1, 1.0]}`.
    pub params: BTreeMap<String, Vec<f32>>,
}

// ------------------------------------------------------------ helpers

/// The key of a resource: one the workspace added (given by key), or one of
/// the game files.
pub(crate) fn resource_key(kit: &Toolkit, ws: &Workspace, text: &str) -> ToolResult<ResourceKey> {
    if let Ok(key) = text.trim().parse::<ResourceKey>()
        && ws.change(&key).is_some_and(|c| c.is_added())
    {
        return Ok(key);
    }
    let at = game_resource(kit, text)?;
    let file = kit.game.open_stream(at.stream)?;
    let header = file
        .layout()
        .resources
        .get(at.resource)
        .ok_or_else(|| ToolError(format!("the stream has no resource {}", at.resource)))?
        .header;
    Ok(ResourceKey::new(
        &kit.game.streams()[at.stream].stream_name(),
        &header,
    ))
}

// ------------------------------------------------------------ tunables

pub fn tunables(kit: &Toolkit, p: TunablesParams) -> ToolResult<Output> {
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    if let Some(id) = &p.id {
        let t = tunables::by_id(id)?;
        let reading = t.read(&kit.game, ws.as_ref())?;
        return Ok(Output::Json(json!({
            "id": t.id,
            "category": t.category,
            "label": t.label,
            "description": t.description,
            "resource": t.resource.to_string(),
            "class": t.class_name(),
            "attribute": t.attribute,
            "kind": format!("{:?}", t.kind).to_lowercase(),
            "min": t.min,
            "max": t.max,
            "objects": reading
                .objects
                .iter()
                .map(|o| json!({ "object": o.object, "name": o.label, "value": o.value }))
                .collect::<Vec<_>>(),
        })));
    }
    let filter = p.filter.as_deref().map(str::to_lowercase);
    let listed: Vec<&tunables::Tunable> = tunables::gf2()
        .iter()
        .filter(|t| {
            p.category
                .as_deref()
                .is_none_or(|c| t.category.eq_ignore_ascii_case(c.trim()))
        })
        .filter(|t| {
            filter.as_deref().is_none_or(|f| {
                t.id.to_lowercase().contains(f)
                    || t.label.to_lowercase().contains(f)
                    || t.description.to_lowercase().contains(f)
            })
        })
        .collect();
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 60, 300);
    let rows: Vec<Value> = listed
        .iter()
        .skip(offset)
        .take(limit)
        .map(|t| {
            // A resource the game does not have (another edition) is said, not fatal.
            let values = match t.read(&kit.game, ws.as_ref()) {
                Ok(reading) => json!({
                    "values": reading
                        .distinct()
                        .iter()
                        .map(|&v| t.format(v))
                        .collect::<Vec<_>>(),
                    "objects": reading.objects.len(),
                }),
                Err(e) => json!({ "unavailable": e.to_string() }),
            };
            json!({
                "id": t.id,
                "category": t.category,
                "label": t.label,
                "kind": format!("{:?}", t.kind).to_lowercase(),
                "min": t.min,
                "max": t.max,
                "now": values,
            })
        })
        .collect();
    Ok(Output::Json(json!({
        "total": listed.len(),
        "offset": offset,
        "categories": tunables::CATEGORIES,
        "tunables": rows,
        "next": "set_tunables changes them (--allow-edits); `id` shows one with every object",
    })))
}

pub fn set_tunables(kit: &Toolkit, p: SetTunablesParams) -> ToolResult<Output> {
    if p.changes.is_empty() {
        return fail("give at least one change: `{id, value}` or `{id, multiply}`");
    }
    if p.dry_run.unwrap_or(false) {
        let ws = kit.workspace()?;
        let changes = tunables::changes(&kit.game, ws.as_ref(), &p.changes)?;
        let (_, diff) = tunables::plan(&kit.game, ws.as_ref(), &changes)?;
        return Ok(Output::Json(
            json!({ "dry_run": true, "would_change": diff }),
        ));
    }
    let mut access = kit.write_workspace("set_tunables")?;
    let ws = &mut access.ws;
    let since = ws.head();
    let (diff, label) = tunables::set(&kit.game, ws, &p.changes)?;
    finish(
        ws,
        since,
        label.as_deref().unwrap_or_default(),
        json!({ "changed_values": diff }),
    )
}

// ------------------------------------------------------------ voice

pub fn voice_add(kit: &Toolkit, p: VoiceAddParams) -> ToolResult<Output> {
    let path = input_file(&p.file)?;
    let chv: ResourceKey = p.speaker.trim().parse().map_err(|e| {
        ToolError(format!(
            "`speaker`: {e}; give the key of a chatter set (`speech` with `show: \"speakers\"`)"
        ))
    })?;
    let pcm = read_audio_file(&path)
        .map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))?;
    let line = NewVoiceLine {
        chv,
        event: p.event.trim().to_owned(),
        name: p.name.trim().to_owned(),
        subtitles: p.subtitles.into_iter().collect(),
        weight: p.weight.unwrap_or(1.0),
        only: p.only.unwrap_or(false),
        category: p.category,
    };
    let mut access = kit.write_workspace(&format!("voice_add: {}", line.name))?;
    let ws = &mut access.ws;
    let since = ws.head();
    let label = format!("New voice line {} ({})", line.name, line.event);
    let report = voice::add_line(&kit.game, ws, &pcm, &line, &label)?;
    finish(ws, since, &label, serde_json::to_value(&report)?)
}

// ------------------------------------------------------------ clothes

pub fn partsdb_add_part(kit: &Toolkit, p: PartsdbAddPartParams) -> ToolResult<Output> {
    let at = game_resource(kit, &p.database)?;
    let mut access = kit.write_workspace(&format!("partsdb_add_part: {}", p.part.name))?;
    let ws = &mut access.ws;
    let since = ws.head();
    let done = outfit::record_part(&kit.game, ws, (at.stream, at.resource), &p.part)?;
    let label = done.operation.clone().unwrap_or_default();
    finish(ws, since, &label, serde_json::to_value(&done)?)
}

pub fn partsdb_allow(kit: &Toolkit, p: PartsdbAllowParams) -> ToolResult<Output> {
    let at = game_resource(kit, &p.database)?;
    let mut access = kit.write_workspace(&format!("partsdb_allow: {}", p.allow.variant))?;
    let ws = &mut access.ws;
    let since = ws.head();
    let done = outfit::record_allow(&kit.game, ws, (at.stream, at.resource), &p.allow)?;
    let label = done.operation.clone().unwrap_or_default();
    finish(ws, since, &label, serde_json::to_value(&done)?)
}

// ------------------------------------------------------------ material

pub fn set_material(kit: &Toolkit, p: SetMaterialParams) -> ToolResult<Output> {
    let mut access = kit.write_workspace(&format!("set_material: {}", p.material))?;
    let ws = &mut access.ws;
    let key = resource_key(kit, ws, &p.material)?;
    let since = ws.head();
    let (changes, label) =
        doctor_toolkit::retexture::set_material_params(&kit.game, ws, &key, &p.params)?;
    finish(
        ws,
        since,
        label.as_deref().unwrap_or_default(),
        json!({ "material": key.to_string(), "changed": changes }),
    )
}
