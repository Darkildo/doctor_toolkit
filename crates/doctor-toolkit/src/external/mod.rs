//! Editing resources in other programs ("Open in external editor").
//!
//! A resource is exported to a working file in a common format (PNG or DDS
//! for textures, CSV or JSON for string tables, JSON for game logic, Lua
//! source for scripts, GLB for models). The file is opened with the program
//! the user chose ([`launch`]) and watched ([`watch`]); every save is imported
//! back into the workspace with the same importers as the CLI. [`EditLink`]
//! ties the three together and turns rapid saves into one undoable operation.
//!
//! The export and import of each format are symmetric: exporting the
//! resource and importing the untouched file records nothing.

pub mod csv;
pub mod launch;
pub mod link;
pub mod watch;

use std::fmt;
use std::str::FromStr;

use serde_json::Value;

use crate::export::gltf::{AssetSource, NoAssets};
use crate::game::{GameAssets, GameInstance, ResourceIndex};
use crate::import::{gltf as gltf_import, model};
use crate::resource::lh2::StringTable;
use crate::resource::lua::{self, Chunk, TYPE_LUA};
use crate::resource::mesh::{self, Model, TYPE_MESH};
use crate::resource::metamodel::MetaModel;
use crate::resource::names::type_tags;
use crate::resource::texture::{self, TYPE_TG4D, TYPE_TG4H, Texture, TexturePair};
use crate::resource::tg4::Tg4Header;
use crate::resource::{logic, logic_import};
use crate::workspace::{ResourceKey, Workspace};
use crate::{Error, Result};

pub use launch::{launch, open_with_desktop, split_command};
pub use link::{EditLink, LinkEvent};
pub use watch::{ChangeDetector, FileWatcher, Poll};

const TYPE_LH2: u32 = crate::hash::sdbm(b"lh2");
const TYPE_METAMODEL: u32 = crate::hash::sdbm(b"metamodel");

/// The format of a working file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// A texture as PNG (one layer, the first mip level).
    Png,
    /// A texture as DDS with every mip level and layer.
    Dds,
    /// A string table as CSV: `hash` and one column per language.
    Csv,
    /// A string table (as `text-export`) or a logic resource (as `logic-export`).
    Json,
    /// A Lua script, decompiled.
    Lua,
    /// A model as binary glTF, with materials, textures and skeleton.
    Glb,
}

impl Format {
    pub const ALL: [Format; 6] = [
        Format::Png,
        Format::Dds,
        Format::Csv,
        Format::Json,
        Format::Lua,
        Format::Glb,
    ];

    /// File extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Dds => "dds",
            Self::Csv => "csv",
            Self::Json => "json",
            Self::Lua => "lua",
            Self::Glb => "glb",
        }
    }

    /// Which program of the settings (`[external_editors]`) opens the file.
    pub fn editor_role(self) -> EditorRole {
        match self {
            Self::Png | Self::Dds => EditorRole::Image,
            Self::Glb => EditorRole::Model,
            Self::Csv | Self::Json | Self::Lua => EditorRole::Text,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.extension())
    }
}

impl FromStr for Format {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let text = text.trim().trim_start_matches('.').to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|f| f.extension() == text || (text == "gltf" && *f == Self::Glb))
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "unknown format `{text}`; expected png, dds, csv, json, lua or glb"
                ))
            })
    }
}

/// The kind of program a working file is opened with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorRole {
    Image,
    Model,
    Text,
}

/// Formats a resource type can be edited in, the default first; empty when
/// the type has no external editing.
pub fn formats_for(type_id: u32) -> &'static [Format] {
    match type_id {
        TYPE_TG4H | TYPE_TG4D => &[Format::Png, Format::Dds],
        TYPE_LH2 => &[Format::Csv, Format::Json],
        TYPE_LUA => &[Format::Lua],
        TYPE_MESH => &[Format::Glb],
        TYPE_METAMODEL => &[Format::Glb, Format::Json],
        t if logic_import::is_supported(t) => &[Format::Json],
        _ => &[],
    }
}

/// A resource opened for external editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Index of the stream in the game.
    pub stream: usize,
    /// The resource; for textures, the header (`tg4h`) of the pair.
    pub resource: usize,
    /// Key of [`Self::resource`].
    pub key: ResourceKey,
    pub type_id: u32,
    pub format: Format,
    /// The data resource of a texture.
    pub texture_data: Option<usize>,
}

impl Target {
    /// Finds `key` in the game. `format` defaults to the first of
    /// [`formats_for`] the type.
    pub fn find(game: &GameInstance, key: &ResourceKey, format: Option<Format>) -> Result<Self> {
        let stream = game
            .find_stream(&key.stream)
            .ok_or_else(|| Error::InvalidArgument(format!("stream {} not found", key.stream)))?;
        let file = game.open_stream(stream)?;
        let index = key
            .find_in(file.layout())
            .ok_or_else(|| Error::InvalidArgument(format!("{key} is not in the game")))?;
        Self::at(game, stream, index, format)
    }

    /// The resource `index` of stream `stream`.
    pub fn at(
        game: &GameInstance,
        stream: usize,
        index: usize,
        format: Option<Format>,
    ) -> Result<Self> {
        let file = game.open_stream(stream)?;
        let layout = file.layout();
        let header = layout
            .resources
            .get(index)
            .ok_or_else(|| Error::InvalidArgument(format!("no resource {index}")))?
            .header;
        let name = game.streams()[stream].stream_name();
        let formats = formats_for(header.type_id);
        if formats.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "{} resources cannot be edited in other programs; textures, string \
                 tables, logic, scripts and models can",
                type_tags().display(header.type_id)
            )));
        }
        let format = format.unwrap_or(formats[0]);
        if !formats.contains(&format) {
            let list: Vec<&str> = formats.iter().map(|f| f.extension()).collect();
            return Err(Error::InvalidArgument(format!(
                "{} resources are edited as {}, not {format}",
                type_tags().display(header.type_id),
                list.join(" or ")
            )));
        }
        let (resource, texture_data, type_id) = if header.type_id == TYPE_TG4H
            || header.type_id == TYPE_TG4D
        {
            let pair = texture::pair_of(layout, index).ok_or_else(|| {
                Error::InvalidArgument(format!("resource {index} of {name} has no texture pair"))
            })?;
            (pair.header, Some(pair.data), TYPE_TG4H)
        } else {
            (index, None, header.type_id)
        };
        let key = ResourceKey::new(&name, &layout.resources[resource].header);
        Ok(Self {
            stream,
            resource,
            key,
            type_id,
            format,
            texture_data,
        })
    }

    /// Whether the working file is a model.
    pub fn is_model(&self) -> bool {
        self.format == Format::Glb
    }
}

/// Lookups across the game that models need: the resource index (skeletons,
/// materials, ids in use) and the materials and textures for the export.
pub struct ModelContext<'a> {
    pub index: &'a ResourceIndex,
    pub assets: &'a mut GameAssets,
}

/// Options of a model import, as `model-import`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelOptions {
    /// Import as a new model set with this name instead of replacing the model.
    pub new_name: Option<String>,
    /// Resources of the stream to point at the new set.
    pub retarget: Vec<usize>,
}

/// Data of resource `index` as the workspace has it, or as the game has it.
fn current(
    game: &GameInstance,
    ws: Option<&Workspace>,
    stream: usize,
    index: usize,
) -> Result<Vec<u8>> {
    match ws {
        Some(ws) => ws.current_data(game, stream, index),
        None => game.open_stream(stream)?.resource_data(index),
    }
}

/// A file name for the working file of `target`: the resource's name when it
/// has one, with the extension of the format.
pub fn file_name(game: &GameInstance, ws: Option<&Workspace>, target: &Target) -> String {
    let id = format!("{:08x}", target.key.ids[3]);
    let tag = type_tags().display(target.type_id).to_string();
    let named = match target.type_id {
        TYPE_TG4H => current(game, ws, target.stream, target.resource)
            .ok()
            .and_then(|d| Tg4Header::parse(&d).ok())
            .map(|h| h.name),
        TYPE_LUA => current(game, ws, target.stream, target.resource)
            .ok()
            .and_then(|d| Chunk::parse(&d).ok())
            .map(|c| {
                lua::file_name(&c.source())
                    .trim_end_matches(".lua")
                    .to_owned()
            }),
        TYPE_MESH | TYPE_METAMODEL if target.is_model() => {
            model_mesh(game, target).ok().and_then(|(m, v)| {
                let data = current(game, ws, target.stream, m).ok()?;
                let vol = match v {
                    Some(v) => current(game, ws, target.stream, v).ok()?,
                    None => Vec::new(),
                };
                Model::parse(&data, &vol).ok().map(|m| m.name)
            })
        }
        _ => None,
    };
    let base = named
        .and_then(|n| sanitize(&n))
        .map_or_else(|| format!("{tag}_{id}"), |n| format!("{n}_{id}"));
    format!("{base}.{}", target.format.extension())
}

/// Letters, digits, `.`, `-` and `_` of `name`.
pub fn sanitize(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let cleaned = cleaned.trim_matches(['.', '_']).to_owned();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The mesh (and its `meshvolatile`) a model target exports: the mesh
/// itself, or the first mesh of a metamodel in the same stream.
fn model_mesh(game: &GameInstance, target: &Target) -> Result<(usize, Option<usize>)> {
    let file = game.open_stream(target.stream)?;
    let layout = file.layout();
    let index = if target.type_id == TYPE_MESH {
        target.resource
    } else {
        let meta = MetaModel::parse(&file.resource_data(target.resource)?)?;
        meta.resources
            .iter()
            .filter(|r| r.type_id.0 == TYPE_MESH)
            .find_map(|r| {
                let ids = r.ids.map(|h| h.0);
                layout
                    .resources
                    .iter()
                    .position(|x| x.header.type_id == TYPE_MESH && x.header.ids == ids)
            })
            .ok_or_else(|| {
                Error::InvalidArgument(format!("{} has no mesh in its stream", target.key))
            })?
    };
    Ok((index, mesh::volatile_for(layout, index)))
}

/// Exports the resource of `target` as its working file: the workspace
/// version when the workspace changes it. A model takes its materials,
/// textures and skeleton from `assets`; without it, only the geometry.
pub fn export(
    game: &GameInstance,
    ws: Option<&Workspace>,
    target: &Target,
    assets: Option<&mut dyn AssetSource>,
) -> Result<Vec<u8>> {
    let data = |index: usize| current(game, ws, target.stream, index);
    match target.format {
        Format::Png | Format::Dds => {
            let texture = Texture {
                header: Tg4Header::parse(&data(target.resource)?)?,
                data: data(target.texture_data.ok_or_else(|| {
                    Error::InvalidArgument(format!("{} is not a texture", target.key))
                })?)?,
            };
            if target.format == Format::Dds {
                return texture.to_dds();
            }
            if texture.header.layer_count() > 1 {
                return Err(Error::InvalidArgument(format!(
                    "{} has {} layers; edit it as DDS",
                    texture.header.name,
                    texture.header.layer_count()
                )));
            }
            texture.to_png(0)
        }
        Format::Csv => {
            let table = StringTable::parse(&data(target.resource)?)?;
            Ok(csv::write_table(&table).into_bytes())
        }
        Format::Json if target.type_id == TYPE_LH2 => crate::navigation::unpack::strings_json(
            &target.key.stream,
            target.resource,
            &target.key.ids,
            &data(target.resource)?,
        ),
        Format::Json => {
            let mut json = logic::to_json(target.type_id, &data(target.resource)?)
                .ok_or_else(|| Error::InvalidArgument(format!("no JSON for {}", target.key)))??;
            // Where the resource is, as `logic-export` writes it.
            if let Value::Object(map) = json {
                let mut located = serde_json::Map::new();
                located.insert("stream".into(), target.key.stream.clone().into());
                located.insert("resource".into(), target.resource.into());
                located.insert(
                    "type".into(),
                    type_tags().display(target.type_id).to_string().into(),
                );
                located.insert("id".into(), format!("{:08x}", target.key.ids[3]).into());
                located.extend(map);
                json = Value::Object(located);
            }
            let text = serde_json::to_string_pretty(&json)
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
            Ok((text + "\n").into_bytes())
        }
        Format::Lua => {
            let chunk = Chunk::parse(&data(target.resource)?)?;
            Ok(lua::round_trip(&chunk).decompiled.source.into_bytes())
        }
        Format::Glb => {
            let (m, v) = model_mesh(game, target)?;
            let volatile = match v {
                Some(v) => data(v)?,
                None => Vec::new(),
            };
            let model = Model::parse(&data(m)?, &volatile)?;
            match (assets, ws) {
                // Materials and textures the workspace adds or changes too.
                (Some(assets), Some(ws)) => crate::export::gltf::model_to_glb(
                    &model,
                    &mut crate::workspace::EditedAssets::new(ws, assets),
                ),
                (Some(assets), None) => crate::export::gltf::model_to_glb(&model, assets),
                (None, _) => crate::export::gltf::model_to_glb(&model, &mut NoAssets),
            }
        }
    }
}

/// What an import recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    /// Label of the workspace operation.
    pub label: String,
    /// Sequence number of the operation; `None` when nothing changed.
    pub seq: Option<u64>,
    /// What changed and warnings, one line each.
    pub notes: Vec<String>,
}

/// Imports a saved working file into the workspace as one operation.
pub fn import(
    game: &GameInstance,
    ws: &mut Workspace,
    target: &Target,
    bytes: &[u8],
    models: Option<ModelContext<'_>>,
    options: &ModelOptions,
) -> Result<Imported> {
    let key = &target.key;
    let mut notes = Vec::new();
    let (label, seq) = match target.format {
        Format::Png | Format::Dds => {
            let pair = TexturePair {
                header: target.resource,
                data: target
                    .texture_data
                    .ok_or_else(|| Error::InvalidArgument(format!("{key} is not a texture")))?,
            };
            let before = ws.head();
            let texture = ws.import_texture(
                game,
                target.stream,
                pair,
                bytes,
                target.format == Format::Png,
            )?;
            let label = format!("Import texture {}", texture.header.name);
            notes.push(format!(
                "{}×{} {}, {} mip level(s)",
                texture.header.width,
                texture.header.height,
                texture.header.format().name(),
                texture.header.mip_count()
            ));
            (label, last_seq(ws, before))
        }
        Format::Csv | Format::Json if target.type_id == TYPE_LH2 => {
            let mut table =
                StringTable::parse(&ws.current_data(game, target.stream, target.resource)?)?;
            let changed = if target.format == Format::Csv {
                csv::apply_table(&mut table, bytes)?
            } else {
                let document: Value = serde_json::from_slice(bytes)
                    .map_err(|e| Error::InvalidArgument(format!("not valid JSON: {e}")))?;
                check_location(&document, target)?;
                crate::navigation::unpack::apply_strings(&mut table, &document)?
            };
            notes.push(format!("{changed} string(s) changed"));
            let label = format!("Edit strings in {}", key.stream);
            let before = ws.head();
            ws.set_string_table(game, target.stream, target.resource, &table)?;
            (label, last_seq(ws, before))
        }
        Format::Csv => {
            return Err(Error::InvalidArgument(format!(
                "{key} is not a string table; CSV is for string tables"
            )));
        }
        Format::Json => {
            let document: Value = serde_json::from_slice(bytes)
                .map_err(|e| Error::InvalidArgument(format!("not valid JSON: {e}")))?;
            check_location(&document, target)?;
            if !logic_import::is_supported(target.type_id) {
                return Err(Error::InvalidArgument(format!(
                    "{} resources cannot be imported from JSON",
                    type_tags().display(target.type_id)
                )));
            }
            let data = ws.current_data(game, target.stream, target.resource)?;
            let imported = logic_import::apply_json(target.type_id, &data, &document)?;
            notes.extend(imported.changes.iter().map(ToString::to_string));
            let label = format!(
                "Edit {} {:08x} in {}",
                type_tags().display(target.type_id),
                key.ids[3],
                key.stream
            );
            let before = ws.head();
            ws.set_resource(game, target.stream, target.resource, imported.data, &label)?;
            (label, last_seq(ws, before))
        }
        Format::Lua => {
            let data = ws.current_data(game, target.stream, target.resource)?;
            let chunk = Chunk::parse(&data)?;
            let name = chunk.main.source.clone().unwrap_or_default();
            let compiled = lua::compile(bytes, &name, chunk.number_size)?;
            let file = lua::file_name(&chunk.source()).to_owned();
            notes.push(format!(
                "{} function(s), {} instruction(s)",
                compiled.main.function_count(),
                compiled.main.instruction_count()
            ));
            let label = format!("Edit script {file}");
            let before = ws.head();
            ws.set_resource(
                game,
                target.stream,
                target.resource,
                compiled.to_bytes(),
                &label,
            )?;
            (label, last_seq(ws, before))
        }
        Format::Glb => {
            let cx = models.ok_or_else(|| {
                Error::InvalidArgument("a model import needs the game index".into())
            })?;
            let gltf = gltf_import::read(bytes, None)?;
            let plan = model::plan(
                game,
                ws,
                cx.index,
                cx.assets,
                target.stream,
                target.resource,
                &gltf,
                &model::ImportOptions {
                    new_name: options.new_name.clone(),
                    retarget: options.retarget.clone(),
                    format: None,
                    collision: None,
                    this_stream_only: false,
                    keep_copies: Vec::new(),
                },
            )?;
            let s = &plan.summary;
            notes.push(format!(
                "{} part(s), {} draw record(s); materials {} reused, {} new; {} texture(s)",
                s.parts.len(),
                s.records,
                s.materials_reused,
                s.materials_new,
                s.textures_imported
            ));
            if let Some(skeleton) = &s.skeleton {
                notes.push(format!("skeleton {skeleton}, {} bone(s)", s.bones_mapped));
            }
            notes.extend(s.warnings.iter().map(|w| format!("warning: {w}")));
            let seq = plan.commit(ws, game)?;
            (plan.label.clone(), seq)
        }
    };
    Ok(Imported { label, seq, notes })
}

/// The sequence number of the operation recorded since the log had `before`
/// operations in effect.
fn last_seq(ws: &Workspace, before: usize) -> Option<u64> {
    let head = ws.head();
    (head > before).then(|| ws.operations()[head - 1].seq)
}

/// A JSON file of another resource is refused.
fn check_location(document: &Value, target: &Target) -> Result<()> {
    if let Some(id) = document.get("id").and_then(Value::as_str)
        && !id.eq_ignore_ascii_case(&format!("{:08x}", target.key.ids[3]))
    {
        return Err(Error::InvalidArgument(format!(
            "the file is for resource {id}, not {}",
            target.key
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
