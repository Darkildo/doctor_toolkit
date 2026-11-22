//! The textures a model draws with, for retexturing it.
//!
//! A model is a mesh, a metamodel (its meshes) or a character (the meshes
//! of the parts of a parts database set). Every draw record names a
//! material; the material names its textures by sampler, which gives each
//! texture its role ([`TextureRole`]). A texture can be in several streams:
//! [`TextureUse::copies`] lists every `tg4h` with its name hash, so that a
//! new image can replace all of them.

use std::collections::HashMap;

use crate::character::MeshLocator;
use crate::export::gltf::AssetSource;
use crate::game::{GameAssets, GameInstance, ResourceIndex};
use crate::resource::material::TextureRole;
use crate::resource::mesh::{self, TYPE_MESH};
use crate::resource::texture::{TYPE_TG4H, TexturePair, pair_of};
use crate::workspace::Workspace;
use crate::{Error, Result};

const TYPE_METAMODEL: u32 = crate::hash::sdbm(b"metamodel");

/// One texture of a model.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureUse {
    /// Name hash (the last id of its `tg4h` and `tg4d`).
    pub hash: u32,
    /// Name in the texture header; empty when the texture is not in the game.
    pub name: String,
    pub role: TextureRole,
    /// Sampler of the first material that uses it, e.g. `g_BaseSampler`.
    pub sampler: String,
    /// Names of the materials that use it.
    pub materials: Vec<String>,
    /// `(stream, tg4h resource)` of every copy in the game.
    pub copies: Vec<(usize, usize)>,
    /// Size of the first copy.
    pub size: (u32, u32),
}

impl TextureUse {
    /// The header and data resources of each copy.
    pub fn pairs(&self, game: &GameInstance) -> Result<Vec<(usize, TexturePair)>> {
        self.copies
            .iter()
            .map(|&(stream, resource)| {
                let file = game.open_stream(stream)?;
                let pair = pair_of(file.layout(), resource).ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "texture {:08x} in {} has no data resource",
                        self.hash,
                        game.streams()[stream].rel_path
                    ))
                })?;
                Ok((stream, pair))
            })
            .collect()
    }
}

/// A short English name of a role.
pub fn role_name(role: TextureRole) -> &'static str {
    match role {
        TextureRole::Diffuse => "colour",
        TextureRole::Normal => "normal map",
        TextureRole::Specular => "specular",
        TextureRole::Gloss => "gloss",
        TextureRole::Detail => "detail",
        TextureRole::Mask => "mask",
        TextureRole::Other => "other",
    }
}

/// The meshes of a mesh or metamodel resource `(stream, resource)`, found in
/// the stream itself first.
pub fn meshes_of(
    game: &GameInstance,
    locator: &MeshLocator,
    at: (usize, usize),
) -> Result<Vec<(usize, usize)>> {
    let file = game.open_stream(at.0)?;
    let header = file
        .layout()
        .resources
        .get(at.1)
        .ok_or_else(|| Error::InvalidArgument(format!("no resource {}", at.1)))?
        .header;
    match header.type_id {
        TYPE_MESH => Ok(vec![at]),
        TYPE_METAMODEL => {
            let refs = crate::import::collision::mesh_references(&file.resource_data(at.1)?)?;
            let mut out = Vec::new();
            for ids in refs {
                if let Some(found) = locator.find(ids[3], at.0)
                    && !out.contains(&found)
                {
                    out.push(found);
                }
            }
            Ok(out)
        }
        other => Err(Error::InvalidArgument(format!(
            "{} is neither a mesh nor a metamodel",
            crate::resource::names::type_tags().display(other)
        ))),
    }
}

/// Every `tg4h` of the game by texture name hash.
pub fn texture_copies(index: &ResourceIndex) -> HashMap<u32, Vec<(usize, usize)>> {
    let mut out: HashMap<u32, Vec<(usize, usize)>> = HashMap::new();
    for e in &index.entries {
        if e.header.type_id == TYPE_TG4H {
            out.entry(e.header.ids[3])
                .or_default()
                .push((e.stream as usize, e.resource as usize));
        }
    }
    out
}

/// The textures of `meshes`, through their materials (after the material
/// replacements `swaps`, as a character set makes them), in order of first use.
pub fn textures_of(
    game: &GameInstance,
    index: &ResourceIndex,
    assets: &mut GameAssets,
    meshes: &[(usize, usize)],
    swaps: &HashMap<u32, u32>,
) -> Result<Vec<TextureUse>> {
    let copies = texture_copies(index);
    let mut materials: Vec<u32> = Vec::new();
    for &(stream, resource) in meshes {
        let file = game.open_stream(stream)?;
        let model = mesh::load(&file, resource)?;
        for record in &model.records {
            let hash = swaps
                .get(&record.material_hash)
                .copied()
                .unwrap_or(record.material_hash);
            if !materials.contains(&hash) {
                materials.push(hash);
            }
        }
    }
    let mut out: Vec<TextureUse> = Vec::new();
    for hash in materials {
        let Some(material) = assets.material(hash) else {
            continue;
        };
        for (role, texture, param) in material.textures() {
            if let Some(known) = out.iter_mut().find(|t| t.hash == texture) {
                if !known.materials.contains(&material.name) {
                    known.materials.push(material.name.clone());
                }
                continue;
            }
            let found = copies.get(&texture).cloned().unwrap_or_default();
            let (name, size) = match assets.texture(texture) {
                Some(t) => (
                    t.header.name.clone(),
                    (t.header.width.into(), t.header.height.into()),
                ),
                None => (String::new(), (0, 0)),
            };
            out.push(TextureUse {
                hash: texture,
                name,
                role,
                sampler: param.name.clone(),
                materials: vec![material.name.clone()],
                copies: found,
                size,
            });
        }
    }
    Ok(out)
}

impl TextureUse {
    /// Replaces the pixels of every copy with `image` (PNG or DDS), one
    /// operation of the workspace per copy; the caller joins them
    /// ([`Workspace::squash`]). Returns the number of copies replaced.
    pub fn replace(
        &self,
        game: &GameInstance,
        ws: &mut Workspace,
        image: &[u8],
        is_png: bool,
    ) -> Result<usize> {
        let pairs = self.pairs(game)?;
        for &(stream, pair) in &pairs {
            ws.import_texture(game, stream, pair, image, is_png)?;
        }
        Ok(pairs.len())
    }
}

/// What [`replace_all_copies`] did.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Replaced {
    /// Name of the texture.
    pub name: String,
    /// Name hash of the texture.
    pub hash: u32,
    /// Streams whose copy was replaced.
    pub streams: Vec<String>,
    /// Size and format of the texture before and after.
    pub before: String,
    pub after: String,
}

/// Replaces the pixels of every copy of the texture with name hash `hash`
/// (every `tg4h` of the game with that id) with `image`, as one undoable
/// operation. What the Retexture wizard does for each texture it changes.
pub fn replace_all_copies(
    game: &GameInstance,
    ws: &mut Workspace,
    index: &ResourceIndex,
    hash: u32,
    image: &[u8],
    is_png: bool,
) -> Result<Replaced> {
    let copies = texture_copies(index).remove(&hash).unwrap_or_default();
    if copies.is_empty() {
        return Err(Error::InvalidArgument(format!(
            "no texture {hash:08x} in the game"
        )));
    }
    let usage = TextureUse {
        hash,
        name: String::new(),
        role: TextureRole::Other,
        sampler: String::new(),
        materials: Vec::new(),
        copies,
        size: (0, 0),
    };
    let pairs = usage.pairs(game)?;
    let since = ws.head();
    let mut out: Option<Replaced> = None;
    for &(stream, pair) in &pairs {
        let before = {
            let file = game.open_stream(stream)?;
            crate::resource::texture::Texture::load(file.data(), file.layout(), pair)?.header
        };
        let after = ws.import_texture(game, stream, pair, image, is_png)?.header;
        let name = game.streams()[stream].stream_name();
        match &mut out {
            Some(done) => done.streams.push(name),
            None => {
                out = Some(Replaced {
                    name: after.name.clone(),
                    hash,
                    streams: vec![name],
                    before: format!(
                        "{}×{} {}",
                        before.width,
                        before.height,
                        before.format().name()
                    ),
                    after: format!(
                        "{}×{} {}, {} mip level(s)",
                        after.width,
                        after.height,
                        after.format().name(),
                        after.mip_count()
                    ),
                });
            }
        }
    }
    let done = out.expect("there is at least one copy");
    if ws.head() > since + 1 {
        let label = format!("Retexture {} ({} copies)", done.name, done.streams.len());
        ws.squash(since, &label)?;
    }
    Ok(done)
}

/// Sets float parameters of the material `key` (a tint colour
/// `g_TintColor0` holds four values): a material of the game files, or one
/// the workspace added (a new material of an imported model). With
/// `all_copies` the edit goes to every copy of the material
/// ([`material::copies`]): the game uses the copy it loads first. One
/// undoable operation; returns what changed in `key` (parameter, old
/// values, new values), the label of the operation (`None` when nothing
/// changed) and the copies.
pub fn set_material_params(
    game: &GameInstance,
    ws: &mut Workspace,
    key: &crate::workspace::ResourceKey,
    params: &std::collections::BTreeMap<String, Vec<f32>>,
    all_copies: bool,
) -> Result<MaterialEdited> {
    use crate::import::file::FileTarget;
    use crate::resource::material::{self, Material, ParamEdit, ParamEditValue};

    if key.type_id != material::TYPE_MATERIAL {
        return Err(Error::InvalidArgument(format!("{key} is not a material")));
    }
    let target = if ws.change(key).is_some_and(|c| c.is_added()) {
        FileTarget::Added(key.clone())
    } else {
        let (stream, resource) = crate::tunables::locate(game, key)?;
        FileTarget::game(game, stream, resource)?
    };
    // What changes, for the answer and the label; the writing itself is
    // `material::apply_params`, shared with the editor and the command line.
    let before = Material::parse(&target.current(game, ws)?)?;
    let mut changes = Vec::new();
    for (name, values) in params {
        let old = before.floats(name).map(<[f32]>::to_vec);
        if old.as_deref() != Some(values.as_slice()) {
            changes.push(MaterialChange {
                param: name.clone(),
                before: old.unwrap_or_default(),
                after: values.clone(),
            });
        }
    }
    let edits: Vec<ParamEdit> = params
        .iter()
        .map(|(name, values)| ParamEdit {
            name: name.clone(),
            value: ParamEditValue::Floats(values.clone()),
        })
        .collect();
    let targets = material::edit_targets(game, ws, target, all_copies)?;
    let names: Vec<String> = if changes.is_empty() {
        params.keys().cloned().collect()
    } else {
        changes.iter().map(|c| c.param.clone()).collect()
    };
    let applied = material::apply_params(game, ws, &targets, &edits, |m, changed| {
        material::copies_label(
            format!("Material {}: {}", m.name, names.join(", ")),
            changed,
        )
    })?;
    Ok(MaterialEdited {
        changes,
        label: (!applied.changed.is_empty()).then_some(applied.label),
        copies: targets.iter().map(|t| t.key().to_string()).collect(),
        changed_copies: applied.changed,
        skipped: applied.skipped,
    })
}

/// What [`set_material_params`] did.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MaterialEdited {
    /// The parameters that change in the given material.
    pub changes: Vec<MaterialChange>,
    /// Label of the recorded operation; `None` when nothing changed.
    pub label: Option<String>,
    /// Every copy the edit went to (workspace keys), the given one first.
    pub copies: Vec<String>,
    /// The copies whose data changed.
    pub changed_copies: Vec<String>,
    /// Copies the edit does not fit, with the reason; they keep their data.
    pub skipped: Vec<String>,
}

/// One parameter [`set_material_params`] changed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MaterialChange {
    pub param: String,
    pub before: Vec<f32>,
    pub after: Vec<f32>,
}
