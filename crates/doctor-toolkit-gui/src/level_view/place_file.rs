//! Model files placed in the view: a file dropped from the file manager, or
//! chosen with "Place a model from a file…".
//!
//! The file goes where the model would stand: the ray under the pointer (or
//! through the middle of the view, for the command) is cast against the
//! collision of the loaded zones, as for a model dragged from the Content
//! Browser ([`super::drop`]). The view only finds the place and returns
//! [`LevelAction::PlaceFile`]; the app reads the file, shows what the import
//! does and records it with the new object as one operation
//! (`app::place_file`, `doctor_toolkit::level::place`).

use std::path::{Path, PathBuf};

use doctor_toolkit::level::place::Sample;
use doctor_toolkit_mods::i18n::tr;
use egui::{Color32, Rect};

use super::drop::ground_point;
use super::walk::physics::V3;
use super::{LevelAction, LevelView};
use crate::render::{View, math};

/// File extensions of the model files that can be placed.
pub const MODEL_EXTENSIONS: &[&str] = &["glb", "gltf", "fbx", "obj", "usd", "usda", "usdc", "usdz"];

/// Whether `path` is a model file by its extension.
pub fn is_model_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MODEL_EXTENSIONS.iter().any(|m| e.eq_ignore_ascii_case(m)))
}

/// How a placed file is imported when nobody is asked (a preset).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlaceOptions {
    /// Name of the new model set; from the file name when `None`.
    pub name: Option<String>,
    /// `mesh`, `hull`, `box` or `none`; a hull when the sample's model has
    /// collision, when `None`.
    pub collision: Option<String>,
    /// Longest side of the textures, pixels.
    pub limit: Option<usize>,
}

/// Where a file waiting for its place goes.
#[derive(Debug, Clone, PartialEq)]
enum Place {
    /// Along this ray (origin, direction).
    Ray(V3, V3),
    /// Along the ray through the middle of the view.
    Centre,
    /// At this point.
    Point(V3),
}

/// A model file that waits for its place (the collision of the level may
/// still be built).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FileDrop {
    path: PathBuf,
    place: Place,
    options: Option<PlaceOptions>,
}

impl LevelView {
    /// Places the model file at `path` where the middle of the view looks
    /// (the edit comes from the next `ui`).
    pub fn request_place_file(&mut self, path: PathBuf) {
        self.file_drop = Some(FileDrop {
            path,
            place: Place::Centre,
            options: None,
        });
    }

    /// The objects of the loaded level a model placed at `at` can copy
    /// (`doctor_toolkit::level::place::samples`): the selected one first.
    pub fn place_samples(&self, at: V3) -> Vec<Sample> {
        let Some(level) = &self.level else {
            return Vec::new();
        };
        let selected = self.filter.selected.map(|r| (r.group, r.object));
        doctor_toolkit::level::place::samples(level, at, selected)
    }

    /// The preset action `file:<path>[:at=<x>/<y>/<z>][:name=<name>]
    /// [:collision=mesh|hull|box|none][:limit=<pixels>]`: the file is placed
    /// without the window, at the point or where the view looks.
    pub(super) fn preset_place_file(&mut self, value: &str) -> Result<(), String> {
        let mut tokens = value.split(':');
        let path = tokens.next().filter(|p| !p.is_empty()).ok_or("no file")?;
        let mut place = Place::Centre;
        let mut options = PlaceOptions::default();
        for token in tokens {
            if let Some(point) = token.strip_prefix("at=") {
                let p: Vec<f32> = point.split('/').filter_map(|v| v.parse().ok()).collect();
                if p.len() != 3 {
                    return Err("at=<x>/<y>/<z> expected".to_owned());
                }
                place = Place::Point([p[0], p[1], p[2]]);
            } else if let Some(name) = token.strip_prefix("name=") {
                options.name = Some(name.to_owned());
            } else if let Some(kind) = token.strip_prefix("collision=") {
                options.collision = Some(kind.to_owned());
            } else if let Some(limit) = token.strip_prefix("limit=") {
                options.limit = Some(
                    limit
                        .parse()
                        .map_err(|_| format!("limit={limit}: a number of pixels expected"))?,
                );
            } else {
                return Err(format!("unknown option `{token}`"));
            }
        }
        self.file_drop = Some(FileDrop {
            path: PathBuf::from(path),
            place,
            options: Some(options),
        });
        Ok(())
    }

    /// Takes a model file dropped on the view from the file manager, shows
    /// that it can be dropped while one hovers, and returns the edit of a
    /// file whose place is known.
    pub(super) fn file_drop_ui(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        view: &View,
    ) -> Option<LevelAction> {
        let pointer = response
            .hover_pos()
            .or(ui.input(|i| i.pointer.latest_pos()));
        // A file manager does not always tell where the pointer is: the
        // view under the pointer takes the file, or the only one in sight.
        let here = pointer.is_none_or(|p| rect.contains(p));
        let (hovering, dropped) = ui.input(|i| {
            let hovering = i
                .raw
                .hovered_files
                .iter()
                .any(|f| f.path.as_deref().is_none_or(is_model_file));
            let dropped = i
                .raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .find(|p| is_model_file(p));
            (hovering, dropped)
        });
        if hovering && here {
            let painter = ui.painter_at(rect);
            painter.rect_stroke(
                rect.shrink(4.0),
                4.0,
                egui::Stroke::new(2.0, crate::app::theme::ACCENT),
                egui::StrokeKind::Inside,
            );
            let text = if self.level.is_some() {
                tr("place.drop_here")
            } else {
                tr("place.load_first")
            };
            let galley = painter.layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(15.0),
                Color32::WHITE,
            );
            let at = rect.center() - galley.size() / 2.0;
            painter.rect_filled(
                Rect::from_min_size(at, galley.size()).expand(8.0),
                4.0,
                Color32::from_black_alpha(180),
            );
            painter.galley(at, galley, Color32::WHITE);
        }
        if let Some(path) = dropped
            && here
            && claim(ui.ctx())
        {
            let place = match pointer.filter(|p| rect.contains(*p)) {
                Some(p) => {
                    let uv = (p - rect.min) / rect.size();
                    let ray = view.ray([uv.x, uv.y]);
                    Place::Ray(ray.origin, ray.dir)
                }
                None => Place::Centre,
            };
            self.file_drop = Some(FileDrop {
                path,
                place,
                options: None,
            });
        }

        let waiting = self.file_drop.clone()?;
        if self.level.is_none() {
            self.file_drop = None;
            self.status = tr("place.load_first").to_owned();
            return None;
        }
        let (origin, dir) = match waiting.place {
            Place::Point(at) => {
                self.file_drop = None;
                return Some(LevelAction::PlaceFile {
                    path: waiting.path,
                    position: at,
                    on_ground: true,
                    options: waiting.options,
                });
            }
            Place::Ray(origin, dir) => (origin, dir),
            Place::Centre => {
                let ray = view.ray([0.5, 0.5]);
                (ray.origin, ray.dir)
            }
        };
        let (level, input) = (self.level.as_ref()?, self.input.as_ref()?);
        let Some(world) = self
            .walk
            .world_for(level, &self.catalog, &input.parents, origin)
        else {
            // The collision is being built: the file waits.
            self.status = tr("drop.waiting").to_owned();
            ui.ctx().request_repaint();
            return None;
        };
        let ground = ground_point(world, origin, dir);
        self.file_drop = None;
        Some(LevelAction::PlaceFile {
            path: waiting.path,
            position: ground.unwrap_or_else(|| math::add(origin, math::scale(dir, 10.0))),
            on_ground: ground.is_some(),
            options: waiting.options,
        })
    }
}

/// Whether this view is the first to take the files dropped in this frame
/// (two level tabs may be in sight).
fn claim(ctx: &egui::Context) -> bool {
    let id = egui::Id::unique("level-file-drop-frame");
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| {
        let taken: Option<u64> = d.get_temp(id);
        if taken == Some(frame) {
            false
        } else {
            d.insert_temp(id, frame);
            true
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_files_are_known_by_extension() {
        assert!(is_model_file(Path::new("/x/Pump.GLB")));
        assert!(is_model_file(Path::new("a.usdz")));
        assert!(!is_model_file(Path::new("a.png")));
        assert!(!is_model_file(Path::new("glb")));
    }
}
