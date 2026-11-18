//! A model file placed in a level as a new object, in one step.
//!
//! Putting one's own model into a level used to take a chain of choices in
//! the game's terms: a metamodel to be the template of the import, the
//! stream the new set goes into, a sample object for the class and logic,
//! the id of the new metamodel typed into "Add object". Here the chain is
//! one function. The *sample* — an object of the level — answers all of it:
//! the new object copies its class and logic, its metamodel is the template
//! of the import (materials, the collision part), and its SimGroup and the
//! stream of that SimGroup are where the object and the new model set go, so
//! the game finds the model from the object.
//!
//! [`samples`] lists the objects of a loaded level that can be the sample,
//! the best first; [`plan`] works the import out (nothing is recorded: the
//! plan has the parts, the resources and the warnings to show); [`record`]
//! records the import and the object as one undoable operation.

use crate::game::{GameAssets, GameInstance, ResourceIndex};
use crate::import::model::{self, ImportOptions, ImportPlan};
use crate::resource::hkx::collision::CollisionKind;
use crate::resource::pixels;
use crate::resource::simgroup::Guid;
use crate::scene::{Scene, SceneImage};
use crate::workspace::{ResourceKey, Workspace};
use crate::{Error, Result};

use super::edit::{self, ModelChoice};
use super::{Found, Level, Location, TYPE_METAMODEL};

/// An object of a level a placed model file can copy.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// The SimGroup of the object: the new object goes into it.
    pub group: Location,
    pub object: usize,
    pub class: String,
    pub name: String,
    /// Id of the object's metamodel and where it is: the template of the
    /// import.
    pub model: u32,
    pub template: Location,
    /// The metamodel has collision: the new set can get its own.
    pub collision: bool,
    /// Metres from the place of the new object.
    pub distance: f32,
    pub kind: SampleKind,
}

/// What an object of a class is to a modder who places a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SampleKind {
    /// `Entity`: a model at a place and nothing else.
    Static,
    /// `Destructible`, `CSADestructible`: props; the copy breaks, moves and
    /// is thrown as the sample is set up to.
    Prop,
    /// Any other class (a door, a window, an item): its logic comes along.
    Other,
    /// `ZoneRenderLOD`: the ground and the buildings of a zone.
    Terrain,
}

impl SampleKind {
    pub fn of_class(class: &str) -> Self {
        match class {
            "Entity" => Self::Static,
            "Destructible" | "CSADestructible" => Self::Prop,
            "ZoneRenderLOD" => Self::Terrain,
            _ => Self::Other,
        }
    }
}

/// Most samples [`samples`] returns.
pub const MAX_SAMPLES: usize = 24;

/// The objects of `level` a model file placed at `at` can copy, the best
/// first: `selected` (group and object numbers in the level) when it can be
/// one; then objects whose model has collision before those without, plain
/// objects before props, props before other classes, the ground of a zone
/// last, nearer ones first. One object per class and model (the nearest).
///
/// An object can be the sample when it has a place and a metamodel of the
/// game files (also of a stream the workspace creates, once staged): the
/// import reads its template there.
pub fn samples(level: &Level, at: [f32; 3], selected: Option<(usize, usize)>) -> Vec<Sample> {
    let sample = |g: usize, o: usize| -> Option<Sample> {
        let group = level.groups.get(g)?;
        let object = group.objects.get(o)?;
        let matrix = object.transform?;
        let model = object.model?;
        let known = level.models.get(&model)?;
        let Some(Found::Game(template, _)) = known.found else {
            return None;
        };
        let distance = (0..3)
            .map(|i| (matrix[3][i] - at[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        let class = object
            .class_name
            .map_or_else(|| format!("{:08x}", object.class), str::to_owned);
        Some(Sample {
            group: group.location,
            object: o,
            kind: SampleKind::of_class(&class),
            class,
            name: object.name.clone(),
            model,
            template,
            // As the import judges it (`import::collision::attach`): the
            // metamodel refers to a collision resource, in any of its states.
            collision: level.source.metamodel(model).is_some_and(|mm| {
                mm.resources.iter().any(|r| {
                    r.type_id.0 == crate::resource::hkx::TYPE_HKX_REFERENCE
                        || r.type_id.0 == crate::resource::hkx::TYPE_HKX
                })
            }),
            distance,
        })
    };
    let mut found: Vec<Sample> = Vec::new();
    for (g, group) in level.groups.iter().enumerate() {
        for o in 0..group.objects.len() {
            if let Some(s) = sample(g, o) {
                found.push(s);
            }
        }
    }
    found.sort_by(|a, b| {
        (a.kind == SampleKind::Terrain, !a.collision, a.kind)
            .cmp(&(b.kind == SampleKind::Terrain, !b.collision, b.kind))
            .then(a.distance.total_cmp(&b.distance))
    });
    let mut out: Vec<Sample> = Vec::new();
    if let Some((g, o)) = selected
        && let Some(s) = sample(g, o)
    {
        out.push(s);
    }
    for s in found {
        if out.len() >= MAX_SAMPLES {
            break;
        }
        if out
            .iter()
            .all(|x| (&x.class, x.model) != (&s.class, s.model))
        {
            out.push(s);
        }
    }
    out
}

/// A name for the new model set from a file name: lower case letters,
/// digits and `_`, as the game names its models.
pub fn name_from_file(path: &std::path::Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut name = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else if !name.ends_with('_') && !name.is_empty() {
            name.push('_');
        }
    }
    let name = name.trim_end_matches('_');
    if name.is_empty() {
        "new_model".to_owned()
    } else {
        name.to_owned()
    }
}

/// The images of `scene` scaled down (halved until they fit) to at most
/// `side` pixels on the longer side. Returns how many images changed.
/// An image that cannot be decoded is left for the import to report.
pub fn limit_textures(scene: &mut Scene, side: usize) -> usize {
    let side = side.max(1);
    let mut changed = 0;
    let mut limit = |image: &mut Option<SceneImage>| {
        let Some(image) = image else { return };
        let Ok((mut w, mut h, mut rgba)) = image.rgba() else {
            return;
        };
        if w.max(h) <= side {
            return;
        }
        while w.max(h) > side && w.min(h) > 1 {
            (w, h, rgba) = pixels::downsample(w, h, &rgba);
        }
        if let Ok(bytes) = pixels::encode_png(w, h, &rgba) {
            *image = SceneImage::png(image.name.clone(), bytes);
            changed += 1;
        }
    };
    for material in &mut scene.materials {
        limit(&mut material.base_color);
        limit(&mut material.normal);
        limit(&mut material.specular);
    }
    changed
}

/// The largest side of the images of `scene`, in pixels (0 with no images).
pub fn largest_texture(scene: &Scene) -> usize {
    scene
        .materials
        .iter()
        .flat_map(|m| [&m.base_color, &m.normal, &m.specular])
        .flatten()
        .filter_map(|image| image.rgba().ok())
        .map(|(w, h, _)| w.max(h))
        .max()
        .unwrap_or(0)
}

/// Works out the import of `scene` as new model set `name` for an object
/// that copies `sample`: the set is made from the sample's metamodel and
/// goes into the stream of the sample's SimGroup. With `collision` the set
/// gets collision of its own built from the file's geometry (the sample's
/// model must have collision). Nothing is recorded.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &Workspace,
    assets: &mut GameAssets,
    scene: &Scene,
    name: &str,
    sample: &Sample,
    collision: Option<CollisionKind>,
) -> Result<ImportPlan> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::InvalidArgument("the new model needs a name".into()));
    }
    let mut plan = model::plan(
        game,
        ws,
        index,
        assets,
        sample.template.stream,
        sample.template.resource,
        scene,
        &ImportOptions {
            new_name: Some(name.to_owned()),
            collision,
            ..Default::default()
        },
    )?;
    let into = stream_name(game, sample.group)?;
    plan.put_into(game, ws, &into)?;
    Ok(plan)
}

fn stream_name(game: &GameInstance, at: Location) -> Result<String> {
    game.streams()
        .get(at.stream)
        .map(|s| s.stream_name())
        .ok_or_else(|| Error::InvalidArgument(format!("no stream {}", at.stream)))
}

/// Id of the metamodel a plan adds.
pub fn new_model(plan: &ImportPlan) -> Option<u32> {
    plan.summary
        .added
        .iter()
        .filter_map(|k| k.parse::<ResourceKey>().ok())
        .find(|k| k.type_id == TYPE_METAMODEL)
        .map(|k| k.ids[3])
}

/// What [`record`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// The SimGroup and the number of the new object.
    pub group: Location,
    pub object: usize,
    pub guid: Guid,
    /// Id of the new metamodel.
    pub model: u32,
    /// Label of the operation.
    pub label: String,
}

/// Records `plan` (of [`plan`] for `sample`) and a copy of the sample at
/// `position` that shows the new model, as one undoable operation. When the
/// object cannot be added, the import is taken back: nothing stays.
pub fn record(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    plan: &ImportPlan,
    sample: &Sample,
    position: [f32; 3],
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<Placed> {
    let model = new_model(plan).ok_or_else(|| {
        Error::InvalidArgument(
            "the import makes no metamodel: the sample must show a metamodel".into(),
        )
    })?;
    let since = ws.head();
    let done = (|| {
        plan.commit(ws, game)?;
        edit::add(
            game,
            index,
            ws,
            sample.group,
            (sample.group, sample.object),
            ModelChoice::Existing(model),
            Some(position),
            taken,
            true,
        )
    })();
    let added = match done {
        Ok(added) => added,
        Err(error) => {
            ws.rollback(since.min(ws.head()))?;
            return Err(error);
        }
    };
    let label = format!(
        "Place {} as object {} of {}",
        plan.label
            .strip_prefix("Import ")
            .unwrap_or(plan.label.as_str()),
        added.object,
        stream_name(game, sample.group)?
    );
    ws.squash(since, &label)?;
    Ok(Placed {
        group: sample.group,
        object: added.object,
        guid: added.guid,
        model,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_file_names() {
        let name = |s: &str| name_from_file(std::path::Path::new(s));
        assert_eq!(name("/x/Vintage Gas-Pump (01).glb"), "vintage_gas_pump_01");
        assert_eq!(name("gas_pump_01.glb"), "gas_pump_01");
        assert_eq!(name("/x/---.fbx"), "new_model");
        assert_eq!(name(""), "new_model");
    }

    #[test]
    fn classes_are_ranked_for_a_placed_model() {
        assert!(SampleKind::of_class("Entity") < SampleKind::of_class("Destructible"));
        assert!(SampleKind::of_class("CSADestructible") < SampleKind::of_class("Door"));
        assert!(SampleKind::of_class("Door") < SampleKind::of_class("ZoneRenderLOD"));
    }

    #[test]
    fn large_images_are_halved_until_they_fit() {
        let rgba: Vec<u8> = (0..64 * 32).flat_map(|_| [200, 100, 50, 255]).collect();
        let png = pixels::encode_png(64, 32, &rgba).unwrap();
        let mut scene = Scene::default();
        scene.materials.push(crate::scene::SceneMaterial {
            name: "m".into(),
            base_color: Some(SceneImage::png("big", png.clone())),
            normal: Some(SceneImage::png("small", {
                let rgba: Vec<u8> = (0..8 * 8).flat_map(|_| [128, 128, 255, 255]).collect();
                pixels::encode_png(8, 8, &rgba).unwrap()
            })),
            ..Default::default()
        });
        assert_eq!(largest_texture(&scene), 64);
        assert_eq!(limit_textures(&mut scene, 16), 1);
        let image = scene.materials[0].base_color.as_ref().unwrap();
        let (w, h, pixels) = image.rgba().unwrap();
        assert_eq!((w, h), (16, 8));
        assert_eq!(&pixels[..4], &[200, 100, 50, 255]);
        assert_eq!(largest_texture(&scene), 16);
        // Nothing to do the second time.
        assert_eq!(limit_textures(&mut scene, 16), 0);
    }
}
