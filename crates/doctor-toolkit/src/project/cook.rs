//! Building one entry: its files into the workspace, with the importers of
//! the library.
//!
//! A key of an entry is a resource of the game, which gets new data, or a
//! resource the project adds ([`super::manifest::AddedSource`]), which is
//! recorded as an added resource with the place and the order its record
//! gives ([`Workspace::put_added_resources`]). The data is made the same
//! way for both; only what it is made over differs: the game's resource, or
//! the data kept with the sources (`base`, `template`).

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use crate::game::{GameAssets, GameInstance, ResourceIndex};
use crate::import::model::{self, ImportOptions};
use crate::resource::anim::{AnimBank, ClipEvent};
use crate::resource::anim_image::BankImage;
use crate::resource::lh2::StringTable;
use crate::resource::logic::TYPE_SIMGROUP;
use crate::resource::logic_import;
use crate::resource::lua::{self, Chunk};
use crate::resource::simgroup::SimGroup;
use crate::resource::texture::{self, TexturePair};
use crate::resource::tg4::Tg4Header;
use crate::scene;
use crate::workspace::{
    AddedResourceEdit, NewResourceEdit, ResourceEdit, ResourceKey, StorageChoice, Workspace,
};
use crate::{Error, Result};

use super::cache;
use super::manifest::{AddedSource, Entry, Kind};
use super::patch::SimGroupPatch;

/// One build: the game and what models need, made when the first model is built.
pub(super) struct Cook<'a> {
    pub game: &'a GameInstance,
    pub root: &'a Path,
    /// Mod id of the project, for the GUIDs of new objects.
    pub project_id: &'a str,
    models: Option<(ResourceIndex, GameAssets)>,
}

/// Where one key of an entry is built into.
enum Slot<'e> {
    /// A resource of the game: its position in the stream, and the storage
    /// the entry asks for it.
    Game(usize, StorageChoice),
    /// A resource the project adds.
    Added(&'e AddedSource),
}

/// The keys of an entry in one stream.
struct Target<'e> {
    /// The stream of the game; `None` for a stream the project creates.
    stream: Option<usize>,
    slots: Vec<(ResourceKey, Slot<'e>)>,
}

impl Target<'_> {
    /// The stream and the positions of the resources of a target that is
    /// all the game's.
    fn game(&self) -> Result<(usize, Vec<usize>)> {
        let resources = self
            .slots
            .iter()
            .map(|(key, slot)| match slot {
                Slot::Game(r, _) => Ok(*r),
                Slot::Added(_) => Err(Error::InvalidArgument(format!(
                    "{key} is added by the project, the other resources of the entry are the \
                     game's: this kind of entry is built into one or the other"
                ))),
            })
            .collect::<Result<Vec<_>>>()?;
        let stream = self
            .stream
            .ok_or_else(|| Error::InvalidArgument("the stream is not in the game".into()))?;
        Ok((stream, resources))
    }

    /// Whether the project adds the main resource.
    fn is_added(&self) -> bool {
        matches!(self.slots.first(), Some((_, Slot::Added(_))))
    }
}

impl<'a> Cook<'a> {
    pub fn new(game: &'a GameInstance, root: &'a Path, project_id: &'a str) -> Self {
        Self {
            game,
            root,
            project_id,
            models: None,
        }
    }

    /// The targets of `entry`, one per stream: where each key is built into.
    fn locate<'e>(&self, entry: &'e Entry) -> Result<Vec<Target<'e>>> {
        entry
            .targets()
            .into_iter()
            .map(|(stream, keys)| {
                let all_added = keys.iter().all(|k| entry.added_record(k).is_some());
                let stream_index = self.game.find_stream(&stream);
                let file = match stream_index {
                    Some(s) if !all_added => Some(self.game.open_stream(s)?),
                    None if !all_added => {
                        return Err(Error::InvalidArgument(format!(
                            "stream {stream} is not in the game"
                        )));
                    }
                    _ => None,
                };
                let slots = keys
                    .into_iter()
                    .map(|key| {
                        let slot = match entry.added_record(&key) {
                            Some(added) => Slot::Added(added),
                            None => Slot::Game(
                                file.as_ref()
                                    .and_then(|f| key.find_in(f.layout()))
                                    .ok_or_else(|| {
                                        Error::InvalidArgument(format!("{key} is not in the game"))
                                    })?,
                                entry.storage_of(&key),
                            ),
                        };
                        Ok((key, slot))
                    })
                    .collect::<Result<_>>()?;
                Ok(Target {
                    stream: stream_index,
                    slots,
                })
            })
            .collect()
    }

    /// Records `data` (one per key of the target) in `ws`: new data of the
    /// game's resources, and the resources the project adds as their
    /// records describe them.
    fn put(
        &self,
        ws: &mut Workspace,
        label: &str,
        target: &Target<'_>,
        data: Vec<Vec<u8>>,
    ) -> Result<()> {
        if target.slots.len() != data.len() {
            return Err(Error::InvalidArgument(format!(
                "{} file(s) for {} resource(s)",
                data.len(),
                target.slots.len()
            )));
        }
        let mut edits = Vec::new();
        let mut additions = Vec::new();
        for ((key, slot), data) in target.slots.iter().zip(data) {
            match slot {
                Slot::Game(index, storage) => edits.push(ResourceEdit {
                    index: *index,
                    data,
                    storage: *storage,
                }),
                Slot::Added(record) => {
                    let added = record.to_added()?;
                    additions.push(AddedResourceEdit {
                        order: added.order,
                        resource: NewResourceEdit {
                            stream: key.stream.clone(),
                            header: added.header(key, data.len() as u64),
                            data,
                            storage: record.storage,
                            before: added.before,
                        },
                    });
                }
            }
        }
        if !edits.is_empty() {
            let stream = target
                .stream
                .ok_or_else(|| Error::InvalidArgument("the stream is not in the game".into()))?;
            ws.set_resources(self.game, label, stream, &edits)?;
        }
        if !additions.is_empty() {
            ws.put_added_resources(self.game, label, &additions)?;
        }
        Ok(())
    }

    /// Builds `entry` of folder `dir` (relative to the root) into `ws`.
    /// `patch` is the path of its patch from the root; `unchanged` tells
    /// that its files are as they were written from the workspace (a
    /// picture or a model the workspace made then builds the data it was
    /// written from, not an encoding of the file). Each target becomes one
    /// or more operations. Returns notes (warnings of importers).
    pub fn entry(
        &mut self,
        ws: &mut Workspace,
        dir: &str,
        entry: &Entry,
        patch: Option<&str>,
        unchanged: bool,
    ) -> Result<Vec<String>> {
        let folder = self.root.join(dir);
        let read = |name: &str| -> Result<Vec<u8>> {
            let path = folder.join(name);
            std::fs::read(&path).map_err(|e| Error::io(&path, e))
        };
        let json = |name: &str| -> Result<Value> {
            serde_json::from_slice(&read(name)?)
                .map_err(|e| Error::InvalidArgument(format!("{name}: not valid JSON: {e}")))
        };
        let label = format!("Build {dir}/{}", entry.files[0]);
        let targets = self.locate(entry)?;
        let mut notes = Vec::new();
        let game = self.game;
        match entry.kind {
            Kind::Texture => {
                let bytes = read(&entry.files[0])?;
                let is_png = entry.files[0].to_ascii_lowercase().ends_with(".png");
                for t in &targets {
                    if t.slots.len() != 2 {
                        return Err(Error::InvalidArgument(
                            "a texture needs its tg4h and tg4d".into(),
                        ));
                    }
                    if unchanged && entry.template.len() == 2 {
                        // The picture was not touched: the texture it was
                        // written from, not a second encoding of it.
                        let data = vec![self.template(entry, 0)?, self.template(entry, 1)?];
                        self.put(ws, &label, t, data)?;
                        continue;
                    }
                    if t.is_added() {
                        // The format, the name and the mip levels of a
                        // texture the game does not have: its header as it
                        // was when the picture was written.
                        let header = Tg4Header::parse(&self.template(entry, 0)?)?;
                        let made = if is_png {
                            texture::from_png(&header, &bytes)?
                        } else {
                            texture::from_dds(&header, &bytes)?
                        };
                        self.put(ws, &label, t, vec![made.header.to_bytes()?, made.data])?;
                        continue;
                    }
                    let (stream, resources) = t.game()?;
                    ws.import_texture(
                        game,
                        stream,
                        TexturePair {
                            header: resources[0],
                            data: resources[1],
                        },
                        &bytes,
                        is_png,
                    )?;
                }
            }
            Kind::Strings => {
                let document = json(&entry.files[0])?;
                let mut table = StringTable::parse(&self.base(entry)?)?;
                let changed = crate::navigation::unpack::apply_strings(&mut table, &document)?;
                notes.push(format!("{changed} string(s) differ from the game"));
                for t in &targets {
                    self.put(ws, &label, t, vec![table.to_bytes()?])?;
                }
            }
            Kind::Logic => {
                let type_id = entry.keys[0].type_id;
                let document = json(&entry.files[0])?;
                if let Some(id) = document.get("id").and_then(Value::as_str)
                    && !id.eq_ignore_ascii_case(&format!("{:08x}", entry.keys[0].ids[3]))
                {
                    return Err(Error::InvalidArgument(format!(
                        "the file is for resource {id}, not {}",
                        entry.keys[0]
                    )));
                }
                let imported = logic_import::apply_json(type_id, &self.base(entry)?, &document)?;
                let mut data = imported.data;
                if let Some(patch) = patch {
                    if type_id != TYPE_SIMGROUP {
                        return Err(Error::InvalidArgument(format!(
                            "{patch}: patches are for SimGroups only"
                        )));
                    }
                    let path = self.root.join(patch);
                    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
                    let parsed = SimGroupPatch::parse(&text).map_err(|e| super::at(patch, e))?;
                    let mut group = SimGroup::parse(&data)?;
                    let scope = format!("{}:{}:{}", self.project_id, entry.type_tag, entry.ids);
                    let report = parsed
                        .apply(&mut group, &scope)
                        .map_err(|e| super::at(patch, e))?;
                    notes.extend(report.warnings.iter().map(|w| format!("{patch}: {w}")));
                    for (name, guid) in &report.added {
                        let text = super::guid::text(guid).unwrap_or_default();
                        notes.push(format!("{patch}: new object {name}: {guid} ({text})"));
                    }
                    data = group.encode();
                }
                for t in &targets {
                    self.put(ws, &label, t, vec![data.clone()])?;
                }
            }
            Kind::Document => {
                let data =
                    super::create::document_bytes(entry.keys[0].type_id, &json(&entry.files[0])?)?;
                for t in &targets {
                    self.put(ws, &label, t, vec![data.clone()])?;
                }
            }
            Kind::Script => {
                let source = read(&entry.files[0])?;
                for t in &targets {
                    let current = match t.slots.first() {
                        Some((_, Slot::Game(index, _))) => {
                            let stream = t.game()?.0;
                            ws.current_data(game, stream, *index)?
                        }
                        // A script the project adds: as the workspace has
                        // it, else as it was when the source was written.
                        Some((key, Slot::Added(_))) => match ws.data(key)? {
                            Some(data) => data,
                            None => self.base(entry)?,
                        },
                        None => continue,
                    };
                    let chunk = Chunk::parse(&current)?;
                    let name = chunk.main.source.clone().unwrap_or_default();
                    let compiled = lua::compile(&source, &name, chunk.number_size)
                        .map_err(|e| Error::InvalidArgument(e.to_string()))?;
                    self.put(ws, &label, t, vec![compiled.to_bytes()])?;
                }
            }
            Kind::Model => {
                if !entry.template.is_empty() {
                    // A model the workspace made (imported over a mesh of
                    // the game or as a new set): the geometry of the file
                    // over the mesh as it was. Its materials and textures
                    // are resources of their own, with their own entries.
                    let mesh = self.template(entry, 0)?;
                    let volatile = match entry.template.len() {
                        1 => Vec::new(),
                        _ => self.template(entry, 1)?,
                    };
                    // The file was not touched: the mesh it was written from.
                    let (mesh, volatile) = if unchanged {
                        (mesh, volatile)
                    } else {
                        let model = scene::read_file(&folder.join(&entry.files[0]))?;
                        let (_, assets) = self.models();
                        let (mesh, volatile, summary) =
                            model::rebuild_added(ws, assets, (&mesh, &volatile), &model)?;
                        notes.extend(summary.warnings);
                        (mesh, volatile)
                    };
                    for t in &targets {
                        let data = match t.slots.len() {
                            1 => vec![mesh.clone()],
                            _ => vec![mesh.clone(), volatile.clone()],
                        };
                        self.put(ws, &label, t, data)?;
                    }
                    return Ok(notes);
                }
                let model = scene::read_file(&folder.join(&entry.files[0]))?;
                let (index, assets) = self.models();
                // Every target is planned before anything is recorded.
                let mut plans = Vec::new();
                for t in &targets {
                    // The mesh; its volatile data may be one a build adds.
                    let (Some((_, Slot::Game(mesh, _))), Some(stream)) =
                        (t.slots.first(), t.stream)
                    else {
                        return Err(Error::InvalidArgument(
                            "a model the project adds needs `template`: the mesh it is built \
                             over (write the sources of the project again)"
                                .into(),
                        ));
                    };
                    plans.push(model::plan(
                        game,
                        ws,
                        index,
                        assets,
                        stream,
                        *mesh,
                        &model,
                        &ImportOptions::default(),
                    )?);
                }
                for plan in plans {
                    notes.extend(plan.summary.warnings.iter().cloned());
                    plan.commit(ws, game)?;
                }
            }
            Kind::Animations => {
                let data = read(&entry.files[0])?;
                let events = match entry.files.get(1) {
                    Some(name) => Some(json(name)?),
                    None => None,
                };
                let data = match events {
                    Some(events) => apply_events(&data, &events)?,
                    None => data,
                };
                for t in &targets {
                    self.put(ws, &label, t, vec![data.clone()])?;
                }
            }
            Kind::Raw => {
                let files: Vec<Vec<u8>> =
                    entry.files.iter().map(|f| read(f)).collect::<Result<_>>()?;
                for t in &targets {
                    self.put(ws, &label, t, files.clone())?;
                }
            }
            Kind::View => {
                return Err(Error::InvalidArgument(
                    "a view is for reading only; edit the raw file of the resource".into(),
                ));
            }
        }
        Ok(notes)
    }

    fn base(&self, entry: &Entry) -> Result<Vec<u8>> {
        let sha = entry.base.as_deref().ok_or_else(|| {
            Error::InvalidArgument(format!(
                "{} entries need `base` (the data the file is applied to)",
                entry.kind.label()
            ))
        })?;
        cache::get_base(self.root, sha)
    }

    /// The data a texture or model the workspace made is built over: the
    /// `n`-th of `template`.
    fn template(&self, entry: &Entry, n: usize) -> Result<Vec<u8>> {
        let sha = entry.template.get(n).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "a {} the project adds needs `template` (the data it is made over: write the \
                 sources of the project again)",
                entry.kind.label()
            ))
        })?;
        cache::get_base(self.root, sha)
    }

    fn models(&mut self) -> (&ResourceIndex, &mut GameAssets) {
        if self.models.is_none() {
            let (index, _) = self.game.build_index(|_| {});
            let assets = GameAssets::new(Arc::new(self.game.clone()), &index);
            self.models = Some((index, assets));
        }
        let (index, assets) = self.models.as_mut().expect("made above");
        (index, assets)
    }
}

/// Writes the events of the clips listed in `document` (as written by the
/// project: `{"clips": [{"index", "clip", "events"}]}`) into the bank.
/// Clips whose events are the same are left alone; the bank is encoded
/// again only when something changed.
pub(super) fn apply_events(data: &[u8], document: &Value) -> Result<Vec<u8>> {
    let bank = AnimBank::parse(data)?;
    let clips = document
        .get("clips")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidArgument("`clips` must be a list".into()))?;
    let mut image: Option<BankImage> = None;
    for item in clips {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::InvalidArgument("a clip without `index`".into()))?
            as usize;
        let clip = bank
            .clips
            .get(index)
            .ok_or_else(|| Error::InvalidArgument(format!("the bank has no clip {index}")))?;
        if let Some(name) = item.get("clip").and_then(Value::as_str)
            && name != clip.display_name()
        {
            return Err(Error::InvalidArgument(format!(
                "clip {index} is `{}`, not `{name}`",
                clip.display_name()
            )));
        }
        let events: Vec<ClipEvent> = serde_json::from_value(
            item.get("events")
                .cloned()
                .unwrap_or(Value::Array(Vec::new())),
        )
        .map_err(|e| Error::InvalidArgument(format!("clip {index}: {e}")))?;
        if events == clip.events {
            continue;
        }
        if image.is_none() {
            image = Some(BankImage::parse(data)?);
        }
        image
            .as_mut()
            .expect("made above")
            .set_events(index, &events)?;
    }
    match image {
        Some(image) => image.encode(),
        None => Ok(data.to_vec()),
    }
}
