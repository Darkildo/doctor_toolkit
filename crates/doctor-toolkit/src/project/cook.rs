//! Building one entry: its files into the workspace, with the importers of
//! the library.

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
use crate::resource::texture::TexturePair;
use crate::scene;
use crate::workspace::{ResourceEdit, StorageChoice, Workspace};
use crate::{Error, Result};

use super::cache;
use super::manifest::{Entry, Kind};
use super::patch::SimGroupPatch;

/// One build: the game and what models need, made when the first model is built.
pub(super) struct Cook<'a> {
    pub game: &'a GameInstance,
    pub root: &'a Path,
    /// Mod id of the project, for the GUIDs of new objects.
    pub project_id: &'a str,
    models: Option<(ResourceIndex, GameAssets)>,
}

/// A resource of the game an entry is built into.
struct Located {
    stream: usize,
    resources: Vec<usize>,
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

    /// The stream and resource indices of each target of `entry`.
    fn locate(&self, entry: &Entry) -> Result<Vec<Located>> {
        entry
            .targets()
            .into_iter()
            .map(|(stream, keys)| {
                let s = self.game.find_stream(&stream).ok_or_else(|| {
                    Error::InvalidArgument(format!("stream {stream} is not in the game"))
                })?;
                let file = self.game.open_stream(s)?;
                let resources = keys
                    .iter()
                    .map(|k| {
                        k.find_in(file.layout()).ok_or_else(|| {
                            Error::InvalidArgument(format!("{k} is not in the game"))
                        })
                    })
                    .collect::<Result<_>>()?;
                Ok(Located {
                    stream: s,
                    resources,
                })
            })
            .collect()
    }

    /// Builds `entry` of folder `dir` (relative to the root) into `ws`.
    /// `patch` is the path of its patch from the root. Each target becomes
    /// one or more operations. Returns notes (warnings of importers).
    pub fn entry(
        &mut self,
        ws: &mut Workspace,
        dir: &str,
        entry: &Entry,
        patch: Option<&str>,
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
                    let &[header, data] = t.resources.as_slice() else {
                        return Err(Error::InvalidArgument(
                            "a texture needs its tg4h and tg4d".into(),
                        ));
                    };
                    ws.import_texture(
                        game,
                        t.stream,
                        TexturePair { header, data },
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
                    ws.set_string_table(game, t.stream, t.resources[0], &table)?;
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
                    ws.set_resource(game, t.stream, t.resources[0], data.clone(), &label)?;
                }
            }
            Kind::Document => {
                let data =
                    super::create::document_bytes(entry.keys[0].type_id, &json(&entry.files[0])?)?;
                for t in &targets {
                    ws.set_resource(game, t.stream, t.resources[0], data.clone(), &label)?;
                }
            }
            Kind::Script => {
                let source = read(&entry.files[0])?;
                for t in &targets {
                    let current = ws.current_data(game, t.stream, t.resources[0])?;
                    let chunk = Chunk::parse(&current)?;
                    let name = chunk.main.source.clone().unwrap_or_default();
                    let compiled = lua::compile(&source, &name, chunk.number_size)
                        .map_err(|e| Error::InvalidArgument(e.to_string()))?;
                    ws.set_resource(game, t.stream, t.resources[0], compiled.to_bytes(), &label)?;
                }
            }
            Kind::Model => {
                let model = scene::read_file(&folder.join(&entry.files[0]))?;
                let (index, assets) = self.models();
                // Every target is planned before anything is recorded.
                let mut plans = Vec::new();
                for t in &targets {
                    plans.push(model::plan(
                        game,
                        ws,
                        index,
                        assets,
                        t.stream,
                        t.resources[0],
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
                    ws.set_resource(game, t.stream, t.resources[0], data.clone(), &label)?;
                }
            }
            Kind::Raw => {
                let files: Vec<Vec<u8>> =
                    entry.files.iter().map(|f| read(f)).collect::<Result<_>>()?;
                for t in &targets {
                    if t.resources.len() != files.len() {
                        return Err(Error::InvalidArgument(format!(
                            "{} file(s) for {} resource(s)",
                            files.len(),
                            t.resources.len()
                        )));
                    }
                    let edits: Vec<ResourceEdit> = t
                        .resources
                        .iter()
                        .zip(&files)
                        .map(|(&index, data)| ResourceEdit {
                            index,
                            data: data.clone(),
                            storage: StorageChoice::Keep,
                        })
                        .collect();
                    ws.set_resources(game, &label, t.stream, &edits)?;
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
