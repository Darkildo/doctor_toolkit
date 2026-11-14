//! Preview and export of TG4 textures in the details panel: zoom and pan,
//! channel selection over a checkerboard, mip levels, cube faces and volume
//! slices, the pixel under the cursor, and the original next to an edit.
//!
//! `DOCTOR_TOOLKIT_GUI_TEXTURE=<options>` presets the view for screenshots:
//! comma-separated `zoom=<factor>` (1 = one texel per screen pixel),
//! `channels=<rgba subset>`, `alpha`, `mip=<level>`, `layer=<index>`,
//! `pan=<x>:<y>` (points) and `compare=0`.

use doctor_toolkit::game::StreamFile;
use doctor_toolkit::resource::texture::{self, Texture};
use doctor_toolkit::resource::tg4::{Format, Kind};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, Pos2, Rect, RichText, Sense, Vec2};

/// Which channels are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channels {
    pub rgb: [bool; 3],
    pub alpha: bool,
    /// Alpha as a grey image.
    pub alpha_only: bool,
}

impl Default for Channels {
    fn default() -> Self {
        Self {
            rgb: [true; 3],
            alpha: true,
            alpha_only: false,
        }
    }
}

impl Channels {
    /// Display pixels for raw RGBA: one selected colour channel is shown as grey,
    /// disabled channels are zero, and without alpha the image is opaque.
    pub fn apply(&self, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(rgba.len());
        let single = match self.rgb {
            [true, false, false] => Some(0),
            [false, true, false] => Some(1),
            [false, false, true] => Some(2),
            _ => None,
        };
        for p in rgba.chunks_exact(4) {
            if self.alpha_only {
                out.extend_from_slice(&[p[3], p[3], p[3], 255]);
                continue;
            }
            let a = if self.alpha { p[3] } else { 255 };
            match single {
                Some(c) => out.extend_from_slice(&[p[c], p[c], p[c], a]),
                None => out.extend_from_slice(&[
                    if self.rgb[0] { p[0] } else { 0 },
                    if self.rgb[1] { p[1] } else { 0 },
                    if self.rgb[2] { p[2] } else { 0 },
                    a,
                ]),
            }
        }
        out
    }
}

/// Zoom and pan of the image canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    /// Screen pixels per texel.
    pub scale: f32,
    /// Offset of the image centre from the canvas centre, in points.
    pub pan: Vec2,
    /// Recompute the scale to fit the canvas.
    pub fit: bool,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            scale: 1.0,
            pan: Vec2::ZERO,
            fit: true,
        }
    }
}

impl Zoom {
    /// The image rectangle in a canvas, for an image of `size` texels.
    pub fn image_rect(&self, canvas: Rect, size: Vec2, pixels_per_point: f32) -> Rect {
        Rect::from_center_size(
            canvas.center() + self.pan,
            size * self.scale / pixels_per_point,
        )
    }

    /// Scale that fits `size` texels into the canvas.
    pub fn fit_scale(canvas: Rect, size: Vec2, pixels_per_point: f32) -> f32 {
        let x = canvas.width() * pixels_per_point / size.x.max(1.0);
        let y = canvas.height() * pixels_per_point / size.y.max(1.0);
        x.min(y).max(1e-3)
    }

    /// Multiplies the scale by `factor` keeping the point under `pointer` in place.
    pub fn zoom_about(&mut self, canvas: Rect, pointer: Pos2, factor: f32) {
        let old = self.scale;
        self.scale = (self.scale * factor).clamp(1.0 / 64.0, 64.0);
        let cursor = pointer - canvas.center();
        self.pan = cursor - (cursor - self.pan) * (self.scale / old);
        self.fit = false;
    }
}

/// A decoded surface: the raw pixels for read-out and the displayed image.
struct Decoded {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    handle: egui::TextureHandle,
}

pub struct TexturePreview {
    texture: Result<Texture, String>,
    /// Resources of the texture in its stream; `None` for embedded textures.
    pub pair: Option<texture::TexturePair>,
    layer: usize,
    mip: usize,
    /// Show the normal map with X and Y swizzled back and Z rebuilt.
    rebuild_normals: bool,
    channels: Channels,
    zoom: Zoom,
    image: Option<Decoded>,
    decode_error: Option<String>,
    /// The game's original when the shown texture is an edited version.
    original: Option<Texture>,
    original_image: Option<Decoded>,
    /// Show the original next to the edited version.
    compare: bool,
    checker: Option<egui::TextureHandle>,
    probe: Option<Probe>,
    /// Offer Import… for a texture without a pair (embedded in an apx file).
    embedded_import: bool,
}

/// The texel under the pointer and its raw value in the shown and the original image.
type Probe = ([usize; 2], [u8; 4], Option<[u8; 4]>);

impl TexturePreview {
    /// Loads the texture that resource `index` belongs to; `None` if it is no texture.
    pub fn load(stream: &StreamFile, index: usize) -> Option<Self> {
        let pair = texture::pair_of(stream.layout(), index)?;
        let texture =
            Texture::load(stream.data(), stream.layout(), pair).map_err(|e| e.to_string());
        let mut preview = Self::new(texture);
        preview.pair = Some(pair);
        Some(preview)
    }

    /// Shows an edited texture of a stream; `original` is the game's version.
    pub fn edited(pair: texture::TexturePair, texture: Texture, original: Option<Texture>) -> Self {
        let mut preview = Self::new(Ok(texture));
        preview.pair = Some(pair);
        preview.compare = original.is_some();
        preview.original = original;
        preview.apply_preset();
        preview
    }

    /// Wraps a texture that is already loaded, e.g. one embedded in an apx file.
    pub fn from_texture(texture: Texture) -> Self {
        Self::new(Ok(texture))
    }

    /// A texture embedded in an apx file that can be replaced; `original`
    /// is the game's version when `texture` is an edit.
    pub fn embedded(texture: Texture, original: Option<Texture>) -> Self {
        let mut preview = Self::new(Ok(texture));
        preview.embedded_import = true;
        preview.compare = original.is_some();
        preview.original = original;
        preview.apply_preset();
        preview
    }

    fn new(texture: Result<Texture, String>) -> Self {
        let mut preview = Self {
            texture,
            pair: None,
            layer: 0,
            mip: 0,
            rebuild_normals: true,
            channels: Channels::default(),
            zoom: Zoom::default(),
            image: None,
            decode_error: None,
            original: None,
            original_image: None,
            compare: false,
            checker: None,
            probe: None,
            embedded_import: false,
        };
        preview.apply_preset();
        preview
    }

    fn apply_preset(&mut self) {
        let Ok(preset) = std::env::var("DOCTOR_TOOLKIT_GUI_TEXTURE") else {
            return;
        };
        for item in preset.split(',').map(str::trim) {
            let (name, value) = item.split_once('=').unwrap_or((item, ""));
            match name {
                "zoom" => {
                    if let Ok(scale) = value.parse() {
                        self.zoom.scale = scale;
                        self.zoom.fit = false;
                    }
                }
                "channels" => {
                    self.channels.rgb = ['r', 'g', 'b'].map(|c| value.contains(c));
                    self.channels.alpha = value.contains('a');
                }
                "alpha" => self.channels.alpha_only = true,
                "mip" => self.mip = value.parse().unwrap_or(0),
                "layer" => self.layer = value.parse().unwrap_or(0),
                "compare" => self.compare = value != "0",
                "pan" => {
                    if let Some((x, y)) = value.split_once(':')
                        && let (Ok(x), Ok(y)) = (x.parse(), y.parse())
                    {
                        self.zoom.pan = Vec2::new(x, y);
                    }
                }
                _ => {}
            }
        }
    }

    /// Shows the preview. Returns an image file the user picked for import.
    pub fn show(&mut self, ui: &mut egui::Ui, status: &mut String) -> Option<std::path::PathBuf> {
        let mut import = None;
        let tex = match &self.texture {
            Ok(tex) => tex,
            Err(error) => {
                ui.colored_label(
                    Color32::RED,
                    trf("texture.cannot_load", &[("error", error)]),
                );
                return None;
            }
        };
        let h = &tex.header;
        let format = h.format();
        ui.horizontal_wrapped(|ui| {
            ui.strong(&h.name);
            let shape = match h.kind() {
                Kind::Flat => trn("texture.mip_levels", h.mip_count() as u64, &[]),
                Kind::Cube => tr("texture.cube").to_owned(),
                Kind::Volume => trn("texture.volume", h.layer_count() as u64, &[]),
            };
            ui.label(format!(
                "{}×{} {}, {shape}{}",
                h.width,
                h.height,
                format.name(),
                if h.is_srgb() { ", sRGB" } else { "" }
            ));
            ui.label(RichText::new(&h.package).weak());
            if self.original.is_some() {
                ui.label(RichText::new(tr("texture.edited")).color(crate::changes_view::PENDING))
                    .on_hover_text(tr("texture.edited_hint"));
            }
        });

        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            if h.layer_count() > 1 {
                let max = h.layer_count() - 1;
                let label = if h.kind() == Kind::Cube {
                    tr("texture.face")
                } else {
                    tr("texture.slice")
                };
                changed |= ui
                    .add(egui::Slider::new(&mut self.layer, 0..=max).text(label))
                    .changed();
            }
            if h.mip_count() > 1 {
                let max = h.mip_count() - 1;
                changed |= ui
                    .add(egui::Slider::new(&mut self.mip, 0..=max).text("Mip"))
                    .changed();
            }
            if format == Format::Dxt5Nm {
                changed |= ui
                    .checkbox(&mut self.rebuild_normals, tr("texture.rebuild_normals"))
                    .changed();
            }
        });
        ui.horizontal_wrapped(|ui| {
            let before = self.channels;
            let c = &mut self.channels;
            ui.add_enabled_ui(!c.alpha_only, |ui| {
                for (i, name) in ["R", "G", "B"].into_iter().enumerate() {
                    ui.toggle_value(&mut c.rgb[i], name);
                }
                ui.toggle_value(&mut c.alpha, "A")
                    .on_hover_text(tr("texture.alpha_hint"));
            });
            ui.toggle_value(&mut c.alpha_only, tr("texture.alpha_only"))
                .on_hover_text(tr("texture.alpha_only_hint"));
            changed |= self.channels != before;
            ui.separator();
            if ui.button(tr("texture.fit")).clicked() {
                self.zoom = Zoom::default();
            }
            if ui
                .button("1:1")
                .on_hover_text(tr("texture.one_to_one_hint"))
                .clicked()
            {
                self.zoom = Zoom {
                    scale: 1.0,
                    pan: Vec2::ZERO,
                    fit: false,
                };
            }
            ui.label(RichText::new(format!("{:.0}%", self.zoom.scale * 100.0)).weak());
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("texture.export_dds")).clicked() {
                *status = save(&format!("{}.dds", h.name), tex.to_dds());
            }
            if ui.button(tr("texture.export_png")).clicked() {
                *status = save(&format!("{}.png", h.name), tex.to_png(self.layer));
            }
            if (self.pair.is_some() || self.embedded_import)
                && ui
                    .button(tr("texture.import"))
                    .on_hover_text(tr("texture.import_hint"))
                    .clicked()
            {
                import = rfd::FileDialog::new()
                    .add_filter(tr("texture.filter_image"), &["dds", "png"])
                    .pick_file();
            }
            if let Some(original) = &self.original {
                let label = trf(
                    "texture.compare",
                    &[(
                        "size",
                        &format!("{}×{}", original.header.width, original.header.height),
                    )],
                );
                changed |= ui.checkbox(&mut self.compare, label).changed();
            }
        });
        if changed {
            self.image = None;
            self.original_image = None;
            self.decode_error = None;
        }

        let options = (self.layer, self.mip, self.rebuild_normals, self.channels);
        if self.image.is_none() && self.decode_error.is_none() {
            match decode(ui.ctx(), tex, options, &h.name) {
                Ok(image) => self.image = Some(image),
                Err(error) => self.decode_error = Some(error),
            }
        }
        if self.compare
            && self.original_image.is_none()
            && let Some(original) = &self.original
        {
            let name = format!("{} (original)", original.header.name);
            self.original_image = decode(ui.ctx(), original, options, &name).ok();
        }
        if let Some(error) = &self.decode_error {
            ui.colored_label(Color32::RED, error);
        }
        let checker = self
            .checker
            .get_or_insert_with(|| checkerboard(ui.ctx()))
            .id();
        let Some(image) = &self.image else {
            return import;
        };
        let layout = Vec2::new(image.width as f32, image.height as f32);
        let compare = self.compare && self.original_image.is_some();
        let canvas_width = if compare {
            ((ui.available_width() - 8.0) / 2.0).max(64.0)
        } else {
            ui.available_width()
        };
        let height = (canvas_width * layout.y / layout.x)
            .min(ui.available_height() - 40.0)
            .clamp(160.0, 560.0);
        let mut probe = None;
        match &self.original_image {
            Some(original) if self.compare => {
                let width = canvas_width;
                let mut hovered = None;
                ui.horizontal_top(|ui| {
                    for (title, shown) in [
                        ("texture.original", original),
                        ("texture.edited_title", image),
                    ] {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(tr(title)).strong());
                            if let Some(texel) =
                                canvas(ui, shown, checker, &mut self.zoom, [width, height], layout)
                            {
                                hovered = Some([
                                    (texel[0] as f32 + 0.5) / shown.width as f32,
                                    (texel[1] as f32 + 0.5) / shown.height as f32,
                                ]);
                            }
                        });
                    }
                });
                // The images may differ in size: probe both at the same relative spot.
                if let Some(uv) = hovered {
                    let (texel, value) = image_at(image, uv);
                    probe = Some((texel, value, Some(image_at(original, uv).1)));
                }
            }
            _ => {
                let width = ui.available_width();
                if let Some(texel) =
                    canvas(ui, image, checker, &mut self.zoom, [width, height], layout)
                {
                    probe = Some((texel, pixel(image, texel), None));
                }
            }
        }
        if probe.is_some() {
            self.probe = probe;
        }
        let text = match self.probe {
            Some(([x, y], p, original)) => {
                let mut text = format!(
                    "{x}, {y}: R {} G {} B {} A {}  #{:02X}{:02X}{:02X}{:02X}",
                    p[0], p[1], p[2], p[3], p[0], p[1], p[2], p[3]
                );
                if let Some(o) = original {
                    text += &trf(
                        "texture.probe_original",
                        &[(
                            "value",
                            &format!("R {} G {} B {} A {}", o[0], o[1], o[2], o[3]),
                        )],
                    );
                }
                text
            }
            None => tr("texture.probe_hint").to_owned(),
        };
        ui.label(RichText::new(text).monospace().small());
        ui.label(
            RichText::new(trf(
                "texture.mouse_hint",
                &[("size", &format!("{}×{}", image.width, image.height))],
            ))
            .weak()
            .small(),
        );
        import
    }
}

/// The texel at texture coordinates `uv` (0..1) of an image and its value.
fn image_at(image: &Decoded, uv: [f32; 2]) -> ([usize; 2], [u8; 4]) {
    let texel = [
        ((uv[0] * image.width as f32) as usize).min(image.width - 1),
        ((uv[1] * image.height as f32) as usize).min(image.height - 1),
    ];
    (texel, pixel(image, texel))
}

fn pixel(image: &Decoded, [x, y]: [usize; 2]) -> [u8; 4] {
    let i = (y * image.width + x) * 4;
    image.rgba[i..i + 4].try_into().unwrap_or([0; 4])
}

fn decode(
    ctx: &egui::Context,
    tex: &Texture,
    (layer, mip, rebuild_normals, channels): (usize, usize, bool, Channels),
    name: &str,
) -> Result<Decoded, String> {
    let mip = mip.min(tex.header.mip_count().saturating_sub(1));
    let (width, height, mut rgba) = tex.rgba_level(layer, mip).map_err(|e| e.to_string())?;
    if tex.header.format() == Format::Dxt5Nm && rebuild_normals {
        doctor_toolkit::resource::pixels::rebuild_normal_map(&mut rgba);
    }
    let shown = channels.apply(&rgba);
    let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &shown);
    // Sharp texels when zoomed in, smooth when zoomed out.
    let options = egui::TextureOptions {
        magnification: egui::TextureFilter::Nearest,
        minification: egui::TextureFilter::Linear,
        ..egui::TextureOptions::LINEAR
    };
    Ok(Decoded {
        width,
        height,
        rgba,
        handle: ctx.load_texture(name, image, options),
    })
}

fn checkerboard(ctx: &egui::Context) -> egui::TextureHandle {
    let (a, b) = (Color32::from_gray(96), Color32::from_gray(160));
    let image = egui::ColorImage::new([2, 2], vec![a, b, b, a]);
    ctx.load_texture("checkerboard", image, egui::TextureOptions::NEAREST_REPEAT)
}

/// Draws the image over a checkerboard with zoom and pan; returns the texel
/// under the pointer. The image is laid out as if it had `layout` texels, so
/// images of different sizes compared side by side stay aligned.
fn canvas(
    ui: &mut egui::Ui,
    image: &Decoded,
    checker: egui::TextureId,
    zoom: &mut Zoom,
    [width, height]: [f32; 2],
    layout: Vec2,
) -> Option<[usize; 2]> {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let ppp = ui.ctx().pixels_per_point();
    let size = layout;
    let texels = Vec2::new(image.width as f32, image.height as f32);
    if zoom.fit {
        zoom.scale = Zoom::fit_scale(rect, size, ppp);
        zoom.pan = Vec2::ZERO;
    }
    if response.dragged() {
        zoom.pan += response.drag_delta();
        zoom.fit = false;
    }
    if response.double_clicked() {
        *zoom = Zoom::default();
        zoom.scale = Zoom::fit_scale(rect, size, ppp);
    }
    if let Some(pointer) = response.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            zoom.zoom_about(rect, pointer, (scroll * 0.0025).exp());
            ui.ctx()
                .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_gray(24));
    let image_rect = zoom.image_rect(rect, size, ppp);
    let visible = image_rect.intersect(rect);
    if visible.is_positive() {
        // 8-point checker cells, anchored to the image.
        let cell = 16.0;
        let uv = Rect::from_min_max(
            ((visible.min - image_rect.min) / cell).to_pos2(),
            ((visible.max - image_rect.min) / cell).to_pos2(),
        );
        painter.image(checker, visible, uv, Color32::WHITE);
    }
    painter.image(
        image.handle.id(),
        image_rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, Color32::from_gray(60)),
        egui::StrokeKind::Inside,
    );
    let pointer = response.hover_pos()?;
    if !image_rect.contains(pointer) {
        return None;
    }
    let t = (pointer - image_rect.min) / image_rect.size();
    let texel = [
        ((t.x * texels.x) as usize).min(image.width - 1),
        ((t.y * texels.y) as usize).min(image.height - 1),
    ];
    // Outline the texel when it is large enough to see.
    let texel_size = image_rect.size() / texels;
    if texel_size.x >= 6.0 {
        let min = image_rect.min + Vec2::new(texel[0] as f32, texel[1] as f32) * texel_size;
        painter.rect_stroke(
            Rect::from_min_size(min, texel_size),
            0.0,
            egui::Stroke::new(1.0, Color32::YELLOW),
            egui::StrokeKind::Outside,
        );
    }
    Some(texel)
}

fn save(default_name: &str, bytes: doctor_toolkit::Result<Vec<u8>>) -> String {
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => return trf("model.cannot_export", &[("error", &error)]),
    };
    let Some(path) = rfd::FileDialog::new()
        .set_file_name(default_name)
        .save_file()
    else {
        return tr("model.export_cancelled").to_owned();
    };
    match std::fs::write(&path, bytes) {
        Ok(()) => trf("model.saved", &[("path", &path.display())]),
        Err(error) => trf(
            "model.cannot_save_path",
            &[("path", &path.display()), ("error", &error)],
        ),
    }
}

// ------------------------------------------------------------ view registry

/// Textures (`tg4h` and `tg4d`), with the workspace version next to the game's.
pub const VIEW: crate::views::ViewEntry = crate::views::ViewEntry {
    id: "texture",
    load: load_view,
};

fn load_view(
    source: &mut crate::views::ViewSource<'_>,
) -> Option<Box<dyn crate::views::ResourceViewer>> {
    let pair = texture::pair_of(source.stream.layout(), source.index)?;
    let edited = [pair.header, pair.data]
        .iter()
        .any(|&i| source.edited(i).is_some());
    if edited && let Some(preview) = edited_texture(source, pair) {
        return Some(Box::new(preview));
    }
    TexturePreview::load(source.stream, source.index)
        .map(|p| Box::new(p) as Box<dyn crate::views::ResourceViewer>)
}

/// The workspace version of a texture with the game's original.
fn edited_texture(
    source: &crate::views::ViewSource<'_>,
    pair: texture::TexturePair,
) -> Option<TexturePreview> {
    let original = |i: usize| {
        source
            .original(i)
            .or_else(|| source.stream.resource_data(i).ok())
    };
    let texture = |header: Vec<u8>, data: Vec<u8>| {
        Some(Texture {
            header: doctor_toolkit::resource::tg4::Tg4Header::parse(&header).ok()?,
            data,
        })
    };
    let edited = texture(source.data(pair.header).ok()?, source.data(pair.data).ok()?)?;
    let original = texture(original(pair.header)?, original(pair.data)?);
    Some(TexturePreview::edited(pair, edited, original))
}

impl crate::views::ResourceViewer for TexturePreview {
    /// The editor fills its tab (stage 56).
    fn tab_body(&self) -> crate::views::TabBody {
        crate::views::TabBody::Fill
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        let path = TexturePreview::show(self, ui, cx.status)?;
        Some(crate::views::ViewAction::ImportTexture {
            pair: self.pair?,
            path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIXELS: [u8; 8] = [10, 20, 30, 40, 200, 100, 50, 0];

    #[test]
    fn channel_selection() {
        assert_eq!(Channels::default().apply(&PIXELS), PIXELS);
        let green = Channels {
            rgb: [false, true, false],
            alpha: false,
            alpha_only: false,
        };
        assert_eq!(green.apply(&PIXELS), [20, 20, 20, 255, 100, 100, 100, 255]);
        let red_blue = Channels {
            rgb: [true, false, true],
            alpha: true,
            alpha_only: false,
        };
        assert_eq!(red_blue.apply(&PIXELS), [10, 0, 30, 40, 200, 0, 50, 0]);
        let alpha = Channels {
            alpha_only: true,
            ..Channels::default()
        };
        assert_eq!(alpha.apply(&PIXELS), [40, 40, 40, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let canvas = Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(400.0, 300.0));
        let size = Vec2::new(256.0, 128.0);
        let mut zoom = Zoom {
            scale: Zoom::fit_scale(canvas, size, 1.0),
            pan: Vec2::ZERO,
            fit: true,
        };
        assert_eq!(zoom.scale, 400.0 / 256.0);
        let pointer = Pos2::new(180.0, 120.0);
        let before = zoom.image_rect(canvas, size, 1.0);
        let t = (pointer - before.min) / before.size();
        zoom.zoom_about(canvas, pointer, 3.0);
        assert!(!zoom.fit);
        let after = zoom.image_rect(canvas, size, 1.0);
        let t2 = (pointer - after.min) / after.size();
        assert!((t - t2).length() < 1e-5, "{t:?} {t2:?}");
        // One texel per physical pixel at scale 1 and 2 points per pixel.
        let one = Zoom {
            scale: 1.0,
            pan: Vec2::ZERO,
            fit: false,
        };
        assert_eq!(one.image_rect(canvas, size, 2.0).size(), size / 2.0);
    }
}
