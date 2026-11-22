//! Editor of a material resource (stage 103): its shaders and every
//! parameter — numbers and colours in fields, textures by name hash. Apply
//! records the changed parameters as one operation
//! (`doctor_toolkit::resource::material::set_params`, the function behind
//! the `material` command of the command line), in every copy of the
//! material in the game unless "This stream only" is set: the game uses
//! the copy it loads first.
//!
//! The material panel of the model and character views leads here with
//! "Edit material…" ([`ask_edit`]).

use doctor_toolkit::resource::material::{
    self, Material, ParamEdit, ParamEditValue, ParamValue, TYPE_MATERIAL,
};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use crate::views::{ResourceViewer, ViewAction, ViewContext, ViewEntry, ViewSource};

/// A value as it is edited.
#[derive(Debug, Clone, PartialEq)]
enum Field {
    Floats(Vec<f32>),
    /// Name hash of a texture, as typed (hexadecimal).
    Texture(String),
    /// A type the toolkit does not read.
    Unknown(u16),
}

impl Field {
    fn of(value: &ParamValue) -> Self {
        match value {
            ParamValue::Floats(v) => Self::Floats(v.clone()),
            ParamValue::Texture(hash) => Self::Texture(format!("{hash:08x}")),
            ParamValue::Unknown(kind) => Self::Unknown(*kind),
        }
    }

    /// The edit that gives a parameter this value; `None` for a texture
    /// hash that does not read and for unknown types.
    fn edit(&self) -> Option<ParamEditValue> {
        match self {
            Self::Floats(v) => Some(ParamEditValue::Floats(v.clone())),
            Self::Texture(text) => u32::from_str_radix(text.trim().trim_start_matches("0x"), 16)
                .ok()
                .map(ParamEditValue::Texture),
            Self::Unknown(_) => None,
        }
    }
}

pub struct MaterialView {
    material: Result<Material, String>,
    /// The parameters shown, each name once (a material lists a parameter
    /// once per shader pass): index into the material's parameters, and
    /// the number of passes.
    shown: Vec<(usize, usize)>,
    /// The values in the fields, one per shown parameter.
    fields: Vec<Field>,
    /// Stream and resource of the material in the game.
    at: (usize, usize),
    /// Apply changes only this resource, not every copy of the material.
    this_stream_only: bool,
}

pub const VIEW: ViewEntry = ViewEntry {
    id: "material",
    load,
};

fn load(source: &mut ViewSource<'_>) -> Option<Box<dyn ResourceViewer>> {
    if source.type_id != TYPE_MATERIAL {
        return None;
    }
    let material = source
        .data(source.index)
        .and_then(|data| Material::parse(&data).map_err(|e| e.to_string()));
    Some(Box::new(MaterialView::new(
        material,
        (source.stream_index, source.index),
    )))
}

/// Whether a parameter of `count` numbers named `name` is a colour.
fn is_colour(name: &str, count: usize) -> bool {
    let lower = name.to_ascii_lowercase();
    (count == 3 || count == 4)
        && (lower.contains("color") || lower.contains("colour"))
        && !lower.contains("uv")
}

fn request_id() -> egui::Id {
    egui::Id::unique("material-edit-request")
}

/// Asks the app to open the material with name hash `hash` in its editor
/// (the app finds the resource; see `BrowserApp::poll_resource_requests`).
pub fn ask_edit(ctx: &egui::Context, hash: u32) {
    ctx.data_mut(|d| d.insert_temp(request_id(), hash));
}

/// The material asked for with [`ask_edit`], once.
pub fn take_request(ctx: &egui::Context) -> Option<u32> {
    ctx.data_mut(|d| d.remove_temp(request_id()))
}

impl MaterialView {
    pub fn new(material: Result<Material, String>, at: (usize, usize)) -> Self {
        let mut shown: Vec<(usize, usize)> = Vec::new();
        if let Ok(m) = &material {
            for (i, p) in m.params.iter().enumerate() {
                match shown
                    .iter_mut()
                    .find(|(first, _)| m.params[*first].name == p.name)
                {
                    Some((_, passes)) => *passes += 1,
                    None => shown.push((i, 1)),
                }
            }
        }
        let fields = material
            .as_ref()
            .map(|m| {
                shown
                    .iter()
                    .map(|&(i, _)| Field::of(&m.params[i].value))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            material,
            shown,
            fields,
            at,
            this_stream_only: false,
        }
    }

    /// The parameters whose field differs from the material, as edits.
    pub fn edits(&self) -> Vec<ParamEdit> {
        let Ok(material) = &self.material else {
            return Vec::new();
        };
        self.shown
            .iter()
            .map(|&(i, _)| &material.params[i])
            .zip(&self.fields)
            .filter(|(p, f)| Field::of(&p.value) != **f)
            .filter_map(|(p, f)| {
                Some(ParamEdit {
                    name: p.name.clone(),
                    value: f.edit()?,
                })
            })
            .collect()
    }

    /// Puts `values` into the field of number parameter `name` (tests); false when there is no such parameter of that size.
    #[cfg(test)]
    pub fn set_floats(&mut self, name: &str, values: &[f32]) -> bool {
        let Ok(material) = &self.material else {
            return false;
        };
        let shown = self.shown.iter().map(|&(i, _)| &material.params[i]);
        for (p, f) in shown.zip(&mut self.fields) {
            if let (true, Field::Floats(v)) = (p.name == name, f)
                && v.len() == values.len()
            {
                v.copy_from_slice(values);
                return true;
            }
        }
        false
    }

    /// The action that records the edits of the fields.
    pub fn apply(&self) -> Option<ViewAction> {
        let edits = self.edits();
        let material = self.material.as_ref().ok()?;
        if edits.is_empty() {
            return None;
        }
        let (stream, resource) = self.at;
        let all_copies = !self.this_stream_only;
        Some(ViewAction::Apply {
            label: trf("material.edit_label", &[("name", &material.name)]),
            run: Box::new(move |game, ws| {
                material::set_params(game, ws, stream, resource, &edits, all_copies).map(|_| ())
            }),
        })
    }

    fn reset(&mut self) {
        if let Ok(m) = &self.material {
            self.fields = self
                .shown
                .iter()
                .map(|&(i, _)| Field::of(&m.params[i].value))
                .collect();
        }
    }
}

impl ResourceViewer for MaterialView {
    fn show(&mut self, ui: &mut egui::Ui, _cx: &mut ViewContext<'_>) -> Option<ViewAction> {
        let material = match &self.material {
            Ok(m) => m,
            Err(error) => {
                ui.colored_label(egui::Color32::RED, error);
                return None;
            }
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(trf("material.title", &[("name", &material.name)]));
            ui.label(
                RichText::new(trf(
                    "material.head",
                    &[
                        ("hash", &format!("{:08x}", material.name_hash)),
                        ("flags", &format!("{:#x}", material.flags)),
                        ("count", &material.params.len()),
                    ],
                ))
                .weak(),
            );
        });
        ui.label(RichText::new(tr("material.shared_hint")).weak().small());
        let changed = self.edits().len();
        let mut action = None;
        let mut reset = false;
        ui.horizontal(|ui| {
            let apply = egui::Button::new(trf("material.apply", &[("count", &changed)]));
            if ui
                .add_enabled(changed > 0, apply)
                .on_hover_text(tr("material.apply_hint"))
                .clicked()
            {
                action = self.apply();
            }
            if ui
                .add_enabled(changed > 0, egui::Button::new(tr("text.discard")))
                .clicked()
            {
                reset = true;
            }
            ui.checkbox(&mut self.this_stream_only, tr("material.this_stream_only"))
                .on_hover_text(tr("material.this_stream_only_hint"));
        });
        let material = self.material.as_ref().ok()?;
        egui::Grid::new(("material editor", self.at))
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                for (i, (&(at, passes), field)) in
                    self.shown.iter().zip(&mut self.fields).enumerate()
                {
                    let p = &material.params[at];
                    let edited = Field::of(&p.value) != *field;
                    let name = RichText::new(&p.name).monospace();
                    ui.label(if edited {
                        name.color(crate::changes_view::PENDING)
                    } else {
                        name
                    });
                    ui.horizontal(|ui| match field {
                        Field::Floats(values) => {
                            if is_colour(&p.name, values.len()) {
                                let mut rgb = [values[0], values[1], values[2]];
                                if ui.color_edit_button_rgb(&mut rgb).changed() {
                                    values[..3].copy_from_slice(&rgb);
                                }
                            }
                            for v in values.iter_mut() {
                                ui.add(egui::DragValue::new(v).speed(0.01).max_decimals(4));
                            }
                        }
                        Field::Texture(text) => {
                            ui.label(tr("material.texture"));
                            ui.add(
                                egui::TextEdit::singleline(text)
                                    .id_salt(("material texture", i))
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(80.0),
                            )
                            .on_hover_text(tr("material.texture_hint"));
                            if field.edit().is_none() {
                                ui.colored_label(egui::Color32::YELLOW, tr("material.bad_hash"));
                            }
                        }
                        Field::Unknown(kind) => {
                            ui.label(
                                RichText::new(trf("material.unknown_type", &[("type", kind)]))
                                    .weak(),
                            );
                        }
                    });
                    let mut note = if p.source == "(null)" {
                        String::new()
                    } else {
                        p.source.clone()
                    };
                    if passes > 1 {
                        note = format!("{} {note}", trf("material.passes", &[("count", &passes)]));
                    }
                    ui.label(RichText::new(note.trim_end()).weak().small());
                    ui.end_row();
                }
            });
        if reset {
            self.reset();
        }
        action
    }

    fn has_unsaved_edits(&self) -> bool {
        !self.edits().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::testing as mini;

    #[test]
    fn fields_become_edits_of_the_changed_parameters() {
        let data = mini::material(mini::MATERIAL_NAME, mini::TEXTURE_HASH);
        let material = Material::parse(&data).unwrap();
        let count = material.floats("g_Color").unwrap().len();
        let mut view = MaterialView::new(Ok(material), (0, mini::MATERIAL));
        assert!(view.edits().is_empty() && view.apply().is_none());
        let new: Vec<f32> = (0..count).map(|i| 0.25 * (i as f32 + 1.0)).collect();
        assert!(view.set_floats("g_Color", &new));
        assert!(!view.set_floats("g_Color", &[1.0; 9]), "the size is kept");
        assert!(!view.set_floats("g_Nothing", &new));
        view.fields[0] = Field::Texture("000000aa".into());
        assert_eq!(
            view.edits(),
            [
                ParamEdit {
                    name: "g_BaseSampler".into(),
                    value: ParamEditValue::Texture(0xaa),
                },
                ParamEdit {
                    name: "g_Color".into(),
                    value: ParamEditValue::Floats(new.clone()),
                },
            ]
        );
        // The edits give the material the fields show.
        let edited = Material::edit(&data, &view.edits()).unwrap();
        let after = Material::parse(&edited).unwrap();
        assert_eq!(after.floats("g_Color").unwrap(), new);
        assert_eq!(after.params[0].value, ParamValue::Texture(0xaa));
        // A hash that does not read is not an edit.
        view.fields[0] = Field::Texture("wall".into());
        assert_eq!(view.edits().len(), 1);
        view.reset();
        assert!(view.edits().is_empty());
        assert!(is_colour("g_TintColor0", 4) && !is_colour("g_TintColor0", 2));
        assert!(!is_colour("g_SpecularPower", 3));
        assert!(!is_colour("g_ColorSpecUVScaleOffset", 4));
    }
}
