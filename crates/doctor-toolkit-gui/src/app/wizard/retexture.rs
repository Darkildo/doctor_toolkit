//! Retexture: pick a model (mesh, metamodel, character set or an object of
//! the level view), see its textures with their roles, export them or open
//! them in an image editor, give new images and compare them side by side;
//! every copy of each texture in the game is replaced in one operation.

use std::collections::HashMap;
use std::path::PathBuf;

use doctor_toolkit::character::{Character, MeshLocator};
use doctor_toolkit::export::gltf::AssetSource;
use doctor_toolkit::external::EditorRole;
use doctor_toolkit::resource::mesh::TYPE_MESH;
use doctor_toolkit::resource::partsdb::{PartsDb, Selection};
use doctor_toolkit::resource::texture::{self, Texture};
use doctor_toolkit::retexture::{self as lib, TextureUse};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use super::{Picker, Start, Task, Tx, Watched};
use crate::app::BrowserApp;
use crate::nav::Location;

pub(super) const TYPE_METAMODEL: u32 = doctor_toolkit::hash::sdbm(b"metamodel");
pub(super) const TYPE_PARTSDB: u32 = doctor_toolkit::hash::sdbm(b"partsdb");
/// Meshes of a model, `(stream, resource)`, and the material replacements of a set.
type Meshes = (Vec<(usize, usize)>, HashMap<u32, u32>);

/// Largest side of the preview images.
const PREVIEW: f32 = 256.0;

/// A new image for one texture.
pub(crate) struct NewImage {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub is_png: bool,
    /// Width and height as the game will have them.
    pub size: (u16, u16),
    /// The first mip level as the game will have it, for the preview.
    rgba: (usize, usize, Vec<u8>),
    preview: Option<egui::TextureHandle>,
}

/// One texture of the model and what is done with it.
pub(crate) struct Slot {
    pub texture: TextureUse,
    pub chosen: bool,
    pub new: Option<Result<NewImage, String>>,
    /// The texture as the game has it, decoded for the preview.
    original: Option<Result<egui::TextureHandle, String>>,
    /// Working file open in an image editor.
    pub watched: Option<Watched>,
}

pub(crate) struct Retexture {
    pub picker: Picker,
    pub target: Option<Location>,
    /// Sets of a parts database target, and the one chosen.
    pub sets: Vec<String>,
    pub set: usize,
    pub slots: Vec<Slot>,
    pub message: Option<String>,
    /// Where "Export" wrote the images.
    pub exported: Option<PathBuf>,
}

impl Retexture {
    pub fn new(app: &mut BrowserApp, start: &Start) -> Self {
        let mut r = Self {
            picker: Picker::default(),
            target: None,
            sets: Vec::new(),
            set: 0,
            slots: Vec::new(),
            message: None,
            exported: None,
        };
        let target = start.loc.or_else(|| {
            let id = start.model?;
            find_metamodel(app, id)
        });
        if let Some(loc) = target {
            r.set_target(app, loc);
        }
        r
    }

    pub fn preset(&mut self, key: &str, value: &str, app: &mut BrowserApp) {
        match key {
            "set" => {
                if let Some(i) = self.sets.iter().position(|s| s == value) {
                    self.set = i;
                    self.load_textures(app);
                }
            }
            "image" => {
                let path = PathBuf::from(value);
                for i in 0..self.slots.len() {
                    if self.slots[i].chosen {
                        self.set_image(app, i, &path);
                    }
                }
            }
            "only" => {
                for (i, s) in self.slots.iter_mut().enumerate() {
                    s.chosen = value.split('+').any(|v| v.parse() == Ok(i));
                }
            }
            _ => {}
        }
    }

    /// Picks the model and lists its textures.
    pub fn set_target(&mut self, app: &mut BrowserApp, loc: Location) {
        self.target = Some(loc);
        self.sets.clear();
        self.set = 0;
        if app.type_of(loc) == Some(TYPE_PARTSDB) {
            match app
                .current_data(loc)
                .and_then(|d| PartsDb::parse(&d).map_err(|e| e.to_string()))
            {
                Ok(db) => {
                    self.sets = db.sets.iter().map(|s| s.name.clone()).collect();
                    self.set = db.set_index("_all").unwrap_or(0);
                }
                Err(error) => {
                    self.message = Some(error);
                    self.slots.clear();
                    return;
                }
            }
        }
        self.load_textures(app);
    }

    /// The meshes of the target and the material replacements of a set.
    fn meshes(&self, app: &BrowserApp, locator: &MeshLocator) -> Result<Meshes, String> {
        let loc = self.target.ok_or(tr("wizard.no_model"))?;
        let game = app.game.clone().ok_or(tr("edits.no_game"))?;
        if app.type_of(loc) == Some(TYPE_PARTSDB) {
            let data = app.current_data(loc)?;
            let db = PartsDb::parse(&data).map_err(|e| e.to_string())?;
            let character = Character::load(
                &game,
                locator,
                loc.stream,
                &db,
                self.set,
                &Selection::default(),
                0,
            )
            .map_err(|e| e.to_string())?;
            let mut swaps = HashMap::new();
            let mut meshes = Vec::new();
            for part in &character.parts {
                meshes.push(part.location);
                swaps.extend(part.materials.iter().map(|(a, b)| (*a, *b)));
            }
            return Ok((meshes, swaps));
        }
        let meshes = lib::meshes_of(&game, locator, (loc.stream, loc.resource))
            .map_err(|e| e.to_string())?;
        Ok((meshes, HashMap::new()))
    }

    pub fn load_textures(&mut self, app: &mut BrowserApp) {
        self.slots.clear();
        self.message = None;
        let (Some(game), Some(index)) = (app.game.clone(), app.index.clone()) else {
            return;
        };
        let locator = MeshLocator::new(&index);
        let result = self.meshes(app, &locator).and_then(|(meshes, swaps)| {
            if meshes.is_empty() {
                return Err(tr("wizard.retexture.no_meshes").to_owned());
            }
            let assets = app
                .assets
                .get_or_insert_with(|| doctor_toolkit::game::GameAssets::new(game.clone(), &index));
            lib::textures_of(&game, &index, assets, &meshes, &swaps).map_err(|e| e.to_string())
        });
        match result {
            Ok(textures) => {
                if textures.is_empty() {
                    self.message = Some(tr("wizard.retexture.no_textures").to_owned());
                }
                self.slots = textures
                    .into_iter()
                    .map(|texture| Slot {
                        chosen: texture.role
                            == doctor_toolkit::resource::material::TextureRole::Diffuse
                            && !texture.copies.is_empty(),
                        texture,
                        new: None,
                        original: None,
                        watched: None,
                    })
                    .collect();
            }
            Err(error) => self.message = Some(error),
        }
    }

    /// The texture of slot `i` as the game has it.
    fn game_texture(app: &mut BrowserApp, texture: &TextureUse) -> Result<Texture, String> {
        let assets = app.assets.as_mut().ok_or(tr("wizard.no_assets"))?;
        assets.texture(texture.hash).ok_or_else(|| {
            trf(
                "wizard.retexture.missing",
                &[("hash", &format!("{:08x}", texture.hash))],
            )
        })
    }

    /// Reads a new image for slot `i`.
    pub fn set_image(&mut self, app: &mut BrowserApp, i: usize, path: &std::path::Path) {
        let result = std::fs::read(path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .and_then(|bytes| self.image_from(app, i, path.to_owned(), bytes));
        self.slots[i].new = Some(result);
    }

    fn image_from(
        &self,
        app: &mut BrowserApp,
        i: usize,
        path: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<NewImage, String> {
        let original = Self::game_texture(app, &self.slots[i].texture)?;
        let is_png = bytes.starts_with(b"\x89PNG");
        let replacement = if is_png {
            texture::from_png(&original.header, &bytes)
        } else {
            texture::from_dds(&original.header, &bytes)
        }
        .map_err(|e| e.to_string())?;
        let size = (replacement.header.width, replacement.header.height);
        let rgba = replacement.rgba(0).map_err(|e| e.to_string())?;
        Ok(NewImage {
            path,
            bytes,
            is_png,
            size,
            rgba,
            preview: None,
        })
    }

    /// Writes the chosen textures as PNG files into `dir`.
    pub fn export(&mut self, app: &mut BrowserApp, dir: &std::path::Path) -> Result<usize, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut written = 0;
        for slot in self.slots.iter().filter(|s| s.chosen) {
            let texture = Self::game_texture(app, &slot.texture)?;
            let png = texture.to_png(0).map_err(|e| e.to_string())?;
            let path = dir.join(file_name(&slot.texture));
            std::fs::write(&path, png).map_err(|e| format!("{}: {e}", path.display()))?;
            written += 1;
        }
        self.exported = Some(dir.to_owned());
        Ok(written)
    }

    /// Writes slot `i` to a working file, opens it in the image program and
    /// takes every save as the new image.
    fn open_editor(&mut self, app: &mut BrowserApp, i: usize) {
        let result = Self::game_texture(app, &self.slots[i].texture)
            .and_then(|t| t.to_png(0).map_err(|e| e.to_string()))
            .and_then(|png| {
                Watched::write(
                    super::work_dir("retexture").join(file_name(&self.slots[i].texture)),
                    &png,
                )
            });
        match result {
            Ok(watched) => {
                app.wizard_open_editor(EditorRole::Image, &watched.path);
                self.slots[i].watched = Some(watched);
            }
            Err(error) => self.message = Some(error),
        }
    }

    fn model_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.label(tr("wizard.retexture.pick"));
        let mut target = self.target;
        if app.picker_ui(
            ui,
            &mut self.picker,
            &[TYPE_MESH, TYPE_METAMODEL, TYPE_PARTSDB],
            &mut target,
        ) && let Some(loc) = target
        {
            self.set_target(app, loc);
        }
        if !self.sets.is_empty() {
            let mut set = self.set;
            egui::ComboBox::from_label(tr("wizard.retexture.set"))
                .selected_text(self.sets.get(set).cloned().unwrap_or_default())
                .show_ui(ui, |ui| {
                    for (i, name) in self.sets.iter().enumerate() {
                        ui.selectable_value(&mut set, i, name);
                    }
                });
            if set != self.set {
                self.set = set;
                self.load_textures(app);
            }
        }
        if let Some(message) = &self.message {
            ui.colored_label(Color32::YELLOW, message);
        } else if self.target.is_some() {
            ui.label(trn(
                "wizard.retexture.found_n",
                self.slots.len() as u64,
                &[],
            ));
        }
        ui.label(
            RichText::new(tr("wizard.retexture.level_hint"))
                .weak()
                .small(),
        );
    }

    fn textures_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.label(tr("wizard.retexture.choose"));
        let mut open = None;
        egui::Grid::new("retexture-list")
            .num_columns(6)
            .striped(true)
            .show(ui, |ui| {
                for title in [
                    "wizard.retexture.col.change",
                    "wizard.retexture.col.texture",
                    "wizard.retexture.col.role",
                    "wizard.retexture.col.size",
                    "wizard.retexture.col.copies",
                    "wizard.retexture.col.materials",
                ] {
                    ui.strong(tr(title));
                }
                ui.end_row();
                for (i, s) in self.slots.iter_mut().enumerate() {
                    let t = &s.texture;
                    ui.add_enabled(
                        !t.copies.is_empty(),
                        egui::Checkbox::without_text(&mut s.chosen),
                    );
                    let name = if t.name.is_empty() {
                        format!("{:08x}", t.hash)
                    } else {
                        t.name.clone()
                    };
                    ui.horizontal(|ui| {
                        ui.label(name);
                        if !t.copies.is_empty()
                            && ui.small_button(tr("wizard.retexture.edit")).clicked()
                        {
                            open = Some(i);
                        }
                    });
                    ui.label(format!("{} ({})", lib::role_name(t.role), t.sampler));
                    ui.label(format!("{}×{}", t.size.0, t.size.1));
                    ui.label(t.copies.len().to_string());
                    ui.label(RichText::new(t.materials.join(", ")).small());
                    ui.end_row();
                }
            });
        if let Some(i) = open {
            self.slots[i].chosen = true;
            self.open_editor(app, i);
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tr("wizard.retexture.export"))
                .on_hover_text(tr("wizard.retexture.export_hint"))
                .clicked()
            {
                let mut dialog = rfd::FileDialog::new().set_title(tr("wizard.retexture.export"));
                if let Some(dir) = &app.settings.export_dirs.export {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(dir) = dialog.pick_folder() {
                    self.message = Some(match self.export(app, &dir) {
                        Ok(n) => trn(
                            "wizard.retexture.exported_n",
                            n as u64,
                            &[("dir", &dir.display())],
                        ),
                        Err(e) => e,
                    });
                }
            }
            if let Some(dir) = &self.exported
                && ui.button(tr("wizard.retexture.show_folder")).clicked()
            {
                let _ = crate::app::shell::open_external(dir.as_os_str());
            }
        });
        if let Some(message) = &self.message {
            ui.label(RichText::new(message).weak());
        }
    }

    fn images_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        // Saves of images open in an editor.
        let mut saved = Vec::new();
        for (i, s) in self.slots.iter_mut().enumerate() {
            if let Some(bytes) = s.watched.as_mut().and_then(Watched::poll) {
                saved.push((i, bytes));
            }
        }
        for (i, bytes) in saved {
            let path = self.slots[i]
                .watched
                .as_ref()
                .map(|w| w.path.clone())
                .unwrap_or_default();
            let result = self.image_from(app, i, path, bytes);
            self.slots[i].new = Some(result);
        }
        if self.slots.iter().any(|s| s.watched.is_some()) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }
        ui.label(tr("wizard.retexture.images"));
        let mut pick = None;
        let mut open = None;
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical()
            .id_salt("retexture-images")
            .auto_shrink(false)
            .show(ui, |ui| {
                for i in 0..self.slots.len() {
                    if !self.slots[i].chosen {
                        continue;
                    }
                    if self.slots[i].original.is_none() {
                        let result = Self::game_texture(app, &self.slots[i].texture)
                            .and_then(|t| t.rgba(0).map_err(|e| e.to_string()))
                            .map(|(w, h, rgba)| {
                                ctx.load_texture(
                                    format!("wizard-old-{i}"),
                                    egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba),
                                    egui::TextureOptions::LINEAR,
                                )
                            });
                        self.slots[i].original = Some(result);
                    }
                    let slot = &mut self.slots[i];
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(&slot.texture.name);
                            ui.label(RichText::new(lib::role_name(slot.texture.role)).weak());
                            if ui.button(tr("wizard.retexture.choose_file")).clicked() {
                                pick = Some(i);
                            }
                            if ui.button(tr("wizard.retexture.edit")).clicked() {
                                open = Some(i);
                            }
                            if let Some(w) = &slot.watched {
                                ui.label(
                                    RichText::new(w.path.display().to_string()).small().weak(),
                                );
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(tr("wizard.retexture.before")).small());
                                match &slot.original {
                                    Some(Ok(tex)) => {
                                        ui.add(
                                            egui::Image::new(tex)
                                                .max_size(egui::vec2(PREVIEW, PREVIEW)),
                                        );
                                    }
                                    Some(Err(e)) => {
                                        ui.colored_label(Color32::YELLOW, e);
                                    }
                                    None => {}
                                }
                            });
                            ui.vertical(|ui| {
                                ui.label(RichText::new(tr("wizard.retexture.after")).small());
                                match &mut slot.new {
                                    Some(Ok(new)) => {
                                        if new.preview.is_none() {
                                            new.preview = decode_preview(&ctx, i, new);
                                        }
                                        if let Some(tex) = &new.preview {
                                            ui.add(
                                                egui::Image::new(tex)
                                                    .max_size(egui::vec2(PREVIEW, PREVIEW)),
                                            );
                                        }
                                        ui.label(
                                            RichText::new(format!(
                                                "{} · {}×{}",
                                                new.path.display(),
                                                new.size.0,
                                                new.size.1
                                            ))
                                            .small(),
                                        );
                                    }
                                    Some(Err(e)) => {
                                        ui.colored_label(
                                            Color32::from_rgb(230, 90, 80),
                                            e.as_str(),
                                        );
                                    }
                                    None => {
                                        ui.label(
                                            RichText::new(tr("wizard.retexture.no_image")).weak(),
                                        );
                                    }
                                }
                            });
                        });
                    });
                }
            });
        if let Some(i) = pick {
            let mut dialog = rfd::FileDialog::new()
                .add_filter(tr("wizard.retexture.images_filter"), &["png", "dds"]);
            if let Some(dir) = self
                .exported
                .as_ref()
                .or(app.settings.export_dirs.import.as_ref())
            {
                dialog = dialog.set_directory(dir);
            }
            if let Some(path) = dialog.pick_file() {
                if let Some(dir) = path.parent() {
                    app.settings.export_dirs.import = Some(dir.to_owned());
                }
                self.set_image(app, i, &path);
            }
        }
        if let Some(i) = open {
            self.open_editor(app, i);
        }
    }

    fn chosen(&self) -> impl Iterator<Item = &Slot> {
        self.slots.iter().filter(|s| s.chosen)
    }
}

/// A file name for a texture: its name, or its hash.
fn file_name(t: &TextureUse) -> String {
    let base = doctor_toolkit::external::sanitize(&t.name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{:08x}", t.hash));
    format!("{base}.png")
}

/// The new image as a texture of the window.
fn decode_preview(ctx: &egui::Context, i: usize, new: &NewImage) -> Option<egui::TextureHandle> {
    let (w, h, rgba) = &new.rgba;
    Some(ctx.load_texture(
        format!("wizard-new-{i}"),
        egui::ColorImage::from_rgba_unmultiplied([*w, *h], rgba),
        egui::TextureOptions::LINEAR,
    ))
}

/// The metamodel with id `id`.
pub(super) fn find_metamodel(app: &BrowserApp, id: u32) -> Option<Location> {
    let index = app.index.as_ref()?;
    index
        .entries
        .iter()
        .find(|e| e.header.type_id == TYPE_METAMODEL && e.header.ids[3] == id)
        .map(|e| Location {
            stream: e.stream as usize,
            resource: e.resource as usize,
        })
}

impl Task for Retexture {
    fn steps(&self) -> &'static [&'static str] {
        &[
            "wizard.retexture.step.model",
            "wizard.retexture.step.textures",
            "wizard.retexture.step.images",
        ]
    }

    fn ui(&mut self, step: usize, ui: &mut egui::Ui, app: &mut BrowserApp, _tx: &mut Tx) {
        match step {
            0 => self.model_step(ui, app),
            1 => self.textures_step(ui, app),
            _ => self.images_step(ui, app),
        }
    }

    fn check(&self, step: usize, _app: &BrowserApp) -> Result<(), String> {
        match step {
            0 if self.slots.is_empty() => Err(self
                .message
                .clone()
                .unwrap_or_else(|| tr("wizard.retexture.need_model").to_owned())),
            1 if self.chosen().next().is_none() => {
                Err(tr("wizard.retexture.need_texture").to_owned())
            }
            2 => {
                for s in self.chosen() {
                    match &s.new {
                        Some(Ok(_)) => {}
                        Some(Err(e)) => return Err(format!("{}: {e}", s.texture.name)),
                        None => {
                            return Err(trf(
                                "wizard.retexture.need_image",
                                &[("texture", &s.texture.name)],
                            ));
                        }
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn summary(&self, app: &BrowserApp) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(loc) = self.target {
            out.push(trf(
                "wizard.retexture.summary_model",
                &[("model", &app.location_label(loc))],
            ));
        }
        for s in self.chosen() {
            if let Some(Ok(new)) = &s.new {
                out.push(trf(
                    "wizard.retexture.summary_line",
                    &[
                        ("texture", &s.texture.name),
                        ("role", &lib::role_name(s.texture.role)),
                        ("file", &new.path.display()),
                        ("width", &new.size.0),
                        ("height", &new.size.1),
                        ("copies", &s.texture.copies.len()),
                    ],
                ));
            }
        }
        out
    }

    fn record(&mut self, app: &mut BrowserApp, _tx: &mut Tx) -> Result<String, String> {
        let game = app.game.clone().ok_or(tr("edits.no_game"))?;
        let mut names = Vec::new();
        for s in self.slots.iter().filter(|s| s.chosen) {
            let Some(Ok(new)) = &s.new else {
                return Err(trf(
                    "wizard.retexture.need_image",
                    &[("texture", &s.texture.name)],
                ));
            };
            let pairs = s.texture.pairs(&game).map_err(|e| e.to_string())?;
            for (stream, pair) in pairs {
                app.workspace_mut()?
                    .import_texture(&game, stream, pair, &new.bytes, new.is_png)
                    .map_err(|e| e.to_string())?;
            }
            names.push(s.texture.name.clone());
        }
        Ok(trn(
            "wizard.retexture.label_n",
            names.len() as u64,
            &[("names", &names.join(", "))],
        ))
    }
}
