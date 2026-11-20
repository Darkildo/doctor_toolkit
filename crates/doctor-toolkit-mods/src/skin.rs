//! The look of Open Mod Manager: the menus of The Godfather II itself.
//!
//! Colours are taken from the game's interface textures (`gf2/frontend`):
//! the rust of its popup panels and their oxblood edge, the mustard brush
//! stroke that marks a chosen menu entry, the sepia, olive and steel of its
//! menu backgrounds, the red of its blood splatter, on the warm blacks of
//! the same tiles. The display face stands in for Aachen Std Bold of the
//! game's menus (Podkova ExtraBold), the caps face for Impact LT Std
//! (Oswald Medium); both are under the SIL Open Font License
//! (`fonts/OFL-*.txt`).
//!
//! The pictures — the game's logo in the language of the interface, its
//! sepia menu tile, the brush stroke and the "?" card of the family tree —
//! are read from the player's own game files ([`read_art`]); nothing of the
//! game is shipped with the program. Without them the same places are
//! painted in the palette.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::epaint::{Mesh, PathShape, PathStroke};
use egui::{
    Align2, Color32, ColorImage, CornerRadius, FontFamily, FontId, Pos2, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, TextureHandle, Ui, Visuals, pos2, vec2,
};

use doctor_toolkit::game::GameInstance;

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// Popup panels of the game (`popup_textures`).
pub const RUST: Color32 = rgb(0xb5, 0x3a, 0x00);
/// Their lower edge.
pub const OXBLOOD: Color32 = rgb(0x4c, 0x00, 0x00);
/// Their shading.
pub const EMBER: Color32 = rgb(0x80, 0x1c, 0x00);
/// The brush stroke of a chosen menu entry (`selectorpopup_textures`).
pub const MUSTARD: Color32 = rgb(0xab, 0xa4, 0x08);
/// The menu background tile, light and middle (`shared_textures`).
pub const SEPIA: Color32 = rgb(0xa0, 0x88, 0x5d);
pub const UMBER: Color32 = rgb(0x58, 0x4c, 0x26);
/// The olive tile, lifted to be read as text.
pub const OLIVE: Color32 = rgb(0x8f, 0x9a, 0x5a);
/// The blood splatter (`#870100`), lifted to be read as text.
pub const BLOOD: Color32 = rgb(0xc4, 0x30, 0x1c);
pub const NIGHT: Color32 = rgb(0x0d, 0x0b, 0x07);
pub const COAL: Color32 = rgb(0x15, 0x12, 0x0c);
pub const SOOT: Color32 = rgb(0x1d, 0x19, 0x11);
pub const ASH: Color32 = rgb(0x2a, 0x24, 0x18);
pub const RULE: Color32 = rgb(0x3a, 0x32, 0x22);
pub const BONE: Color32 = rgb(0xe7, 0xdf, 0xcb);
pub const BONE_DIM: Color32 = rgb(0xa9, 0x9d, 0x82);
/// Text on the mustard stroke.
pub const INK: Color32 = rgb(0x1b, 0x16, 0x04);
/// Text on rust.
pub const CREAM: Color32 = rgb(0xff, 0xf6, 0xe8);

const DISPLAY: &str = "gf2-display";
const CAPS: &str = "gf2-caps";

/// Installs the fonts and the visuals (dark only: the game's menus are).
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (name, data) in [
        (
            DISPLAY,
            &include_bytes!("../fonts/Podkova-ExtraBold.ttf")[..],
        ),
        (CAPS, &include_bytes!("../fonts/Oswald-Medium.ttf")[..]),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(egui::FontData::from_static(data)));
        let mut chain = vec![name.to_owned()];
        chain.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(name.into()), chain);
    }
    ctx.set_fonts(fonts);
    ctx.set_visuals_of(egui::Theme::Dark, visuals());
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(ACTIVE), true));
}

const ACTIVE: &str = "gf2-skin-active";

/// Whether [`install`] dressed this context (Open Mod Manager): widgets
/// shared with the editor then take the game's colours.
pub fn active(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::unique(ACTIVE)))
        .unwrap_or(false)
}

/// The dark visuals in the palette of the game.
pub fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    let corner = CornerRadius::same(1);
    v.panel_fill = COAL;
    v.window_fill = SOOT;
    v.extreme_bg_color = NIGHT;
    v.faint_bg_color = SOOT;
    v.code_bg_color = ASH;
    v.window_stroke = Stroke::new(1.0, RULE);
    v.window_corner_radius = CornerRadius::same(2);
    v.menu_corner_radius = CornerRadius::same(2);
    v.hyperlink_color = rgb(0xd6, 0xb8, 0x7c);
    v.warn_fg_color = MUSTARD;
    v.error_fg_color = BLOOD;
    v.selection.bg_fill = UMBER;
    v.selection.stroke = Stroke::new(1.0, BONE);
    v.override_text_color = None;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = SOOT;
    w.noninteractive.weak_bg_fill = SOOT;
    w.noninteractive.bg_stroke = Stroke::new(1.0, RULE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, rgb(0xd8, 0xcf, 0xba));
    w.noninteractive.corner_radius = corner;
    w.inactive.bg_fill = ASH;
    w.inactive.weak_bg_fill = ASH;
    w.inactive.bg_stroke = Stroke::new(1.0, RULE);
    w.inactive.fg_stroke = Stroke::new(1.0, BONE);
    w.inactive.corner_radius = corner;
    w.hovered.bg_fill = rgb(0x36, 0x2d, 0x1d);
    w.hovered.weak_bg_fill = rgb(0x36, 0x2d, 0x1d);
    w.hovered.bg_stroke = Stroke::new(1.0, SEPIA);
    w.hovered.fg_stroke = Stroke::new(1.5, CREAM);
    w.hovered.corner_radius = corner;
    w.active.bg_fill = EMBER;
    w.active.weak_bg_fill = EMBER;
    w.active.bg_stroke = Stroke::new(1.0, RUST);
    w.active.fg_stroke = Stroke::new(1.5, CREAM);
    w.active.corner_radius = corner;
    w.open.bg_fill = ASH;
    w.open.weak_bg_fill = ASH;
    w.open.bg_stroke = Stroke::new(1.0, SEPIA);
    w.open.fg_stroke = Stroke::new(1.0, BONE);
    w.open.corner_radius = corner;
    v
}

fn family(ctx: &egui::Context, name: &str) -> FontFamily {
    let wanted = FontFamily::Name(name.into());
    if ctx.fonts(|f| f.definitions().families.contains_key(&wanted)) {
        wanted
    } else {
        FontFamily::Proportional
    }
}

/// The display face (Aachen of the game's menus) at `size`.
pub fn display(ctx: &egui::Context, size: f32) -> FontId {
    FontId::new(size, family(ctx, DISPLAY))
}

/// The narrow caps face (Impact of the game's menus) at `size`.
pub fn caps(ctx: &egui::Context, size: f32) -> FontId {
    FontId::new(size, family(ctx, CAPS))
}

/// A label in narrow caps.
pub fn caps_label(ui: &mut Ui, text: &str, color: Color32) -> Response {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.to_uppercase())
                .font(caps(ui.ctx(), 12.5))
                .color(color)
                .extra_letter_spacing(1.4),
        )
        .selectable(false),
    )
}

// --------------------------------------------------------------- the art

/// Pictures of the game's interface, decoded (see [`read_art`]).
#[derive(Default)]
pub struct ArtImages {
    pub logo: Option<ColorImage>,
    pub sepia: Option<ColorImage>,
    pub brush: Option<ColorImage>,
    pub card: Option<ColorImage>,
}

/// The same pictures as textures.
#[derive(Clone, Default)]
pub struct Art {
    pub logo: Option<TextureHandle>,
    pub sepia: Option<TextureHandle>,
    pub brush: Option<TextureHandle>,
    pub card: Option<TextureHandle>,
}

/// Rows of the logo sheet (`frontend` movie, texture 0, 256 × 2048) that
/// hold the logo of a language.
fn logo_rows(language: &str) -> (usize, usize) {
    match language {
        "ru" => (1140, 1297),
        "es" => (261, 408),
        "de" => (417, 567),
        "it" => (573, 725),
        "pl" => (738, 878),
        "fr" => (1627, 1765),
        "cs" => (6, 250),
        _ => (1305, 1453),
    }
}

/// `rgba` (`w` × `h`) cut to `x0..x1`, `y0..y1`, then to its opaque part.
fn cut(
    w: usize,
    h: usize,
    rgba: &[u8],
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> Option<ColorImage> {
    if x1 > w || y1 > h || x0 >= x1 || y0 >= y1 || rgba.len() != w * h * 4 {
        return None;
    }
    let (mut l, mut t, mut r, mut b) = (x1, y1, x0, y0);
    for y in y0..y1 {
        for x in x0..x1 {
            if rgba[(y * w + x) * 4 + 3] > 8 {
                l = l.min(x);
                t = t.min(y);
                r = r.max(x + 1);
                b = b.max(y + 1);
            }
        }
    }
    if l >= r || t >= b {
        return None;
    }
    let mut out = Vec::with_capacity((r - l) * (b - t) * 4);
    for y in t..b {
        out.extend_from_slice(&rgba[(y * w + l) * 4..(y * w + r) * 4]);
    }
    Some(ColorImage::from_rgba_unmultiplied([r - l, b - t], &out))
}

/// Reads the pictures of the interface from the `gf2/frontend` stream of
/// `game`: the logo in `language`, the sepia menu tile, the brush stroke
/// and the "?" card. What is not found as expected (another edition, a
/// changed file) stays `None`.
pub fn read_art(game: &GameInstance, language: &str) -> ArtImages {
    use doctor_toolkit::resource::apx::AptFile;
    let mut art = ArtImages::default();
    let Some(index) = game.find_stream("gf2/frontend") else {
        return art;
    };
    let Ok(stream) = game.open_stream(index) else {
        return art;
    };
    let apx = doctor_toolkit::hash::string_id("apx");
    for (i, resource) in stream.layout().resources.iter().enumerate() {
        if resource.header.type_id != apx {
            continue;
        }
        let Ok(file) = stream.resource_data(i).and_then(|d| AptFile::parse(&d)) else {
            continue;
        };
        let title = file.title().unwrap_or_default().to_ascii_lowercase();
        let wanted: &[usize] = match title.as_str() {
            "frontend" => &[0],
            "shared" => &[2, 7],
            "selectorpopup" => &[0],
            _ => continue,
        };
        let Ok(textures) = file.textures() else {
            continue;
        };
        for &n in wanted {
            let Some(Ok((w, h, rgba))) = textures.get(n).map(|t| t.rgba(0)) else {
                continue;
            };
            match (title.as_str(), n, w, h) {
                ("frontend", 0, 256, 2048) => {
                    let (y0, y1) = logo_rows(language);
                    art.logo = cut(w, h, &rgba, 0, y0, 256, y1);
                }
                ("shared", 2, 256, 2048) => {
                    art.sepia = cut(w, h, &rgba, 0, 1024, 256, 1280);
                }
                ("shared", 7, 256, 1024) => {
                    art.card = cut(w, h, &rgba, 0, 666, 256, 922);
                }
                ("selectorpopup", 0, 1024, 256) => {
                    art.brush = cut(w, h, &rgba, 0, 0, 500, 256);
                }
                _ => {}
            }
        }
    }
    art
}

/// Reads [`Art`] off the interface thread once per game and language.
#[derive(Default)]
pub struct ArtLoader {
    key: Option<(std::path::PathBuf, String)>,
    pending: Option<Receiver<ArtImages>>,
    art: Art,
}

impl ArtLoader {
    /// The pictures of `game` in `language`; empty until they are read.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        game: Option<&Arc<GameInstance>>,
        language: &str,
    ) -> &Art {
        let key = game.map(|g| (g.root().to_path_buf(), language.to_owned()));
        if key != self.key {
            self.key = key;
            self.art = Art::default();
            self.pending = game.map(|g| {
                let (tx, rx) = channel();
                let (g, lang, ctx) = (Arc::clone(g), language.to_owned(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(read_art(&g, &lang));
                    ctx.request_repaint();
                });
                rx
            });
        }
        if let Some(images) = self.pending.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.pending = None;
            let load = |name: &str, image: Option<ColorImage>| {
                image.map(|i| {
                    ctx.load_texture(format!("gf2-{name}"), i, egui::TextureOptions::LINEAR)
                })
            };
            self.art = Art {
                logo: load("logo", images.logo),
                sepia: load("sepia", images.sepia),
                brush: load("brush", images.brush),
                card: load("card", images.card),
            };
        }
        &self.art
    }
}

// ------------------------------------------------------------ painting

const UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

/// A rectangle whose colour goes from `left` to `right`.
pub fn gradient(painter: &egui::Painter, rect: Rect, left: Color32, right: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

/// The game's menu tile under `rect` (or its colour), darkened to the left
/// so text and the logo read on it.
pub fn paint_backdrop(painter: &egui::Painter, rect: Rect, art: &Art) {
    match &art.sepia {
        Some(t) => {
            painter.image(t.id(), rect, UV, Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect, CornerRadius::ZERO, UMBER);
        }
    }
    let (l, m) = (rect.left(), rect.left() + rect.width() * 0.45);
    gradient(
        painter,
        Rect::from_x_y_ranges(l..=m, rect.y_range()),
        Color32::from_black_alpha(228),
        Color32::from_black_alpha(150),
    );
    gradient(
        painter,
        Rect::from_x_y_ranges(m..=rect.right(), rect.y_range()),
        Color32::from_black_alpha(150),
        Color32::from_black_alpha(110),
    );
}

/// A panel of the game's popups: `rect` with its lower corners pulled in
/// by `slant` on the left and the right, and an oxblood edge under it.
pub fn paint_panel(painter: &egui::Painter, rect: Rect, fill: Color32, slant: (f32, f32)) {
    let points = |r: Rect| {
        vec![
            r.left_top(),
            r.right_top(),
            pos2(r.right() - slant.1, r.bottom()),
            pos2(r.left() + slant.0, r.bottom()),
        ]
    };
    painter.add(Shape::convex_polygon(
        points(rect.translate(vec2(0.0, 3.0))),
        OXBLOOD,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(points(rect), fill, Stroke::NONE));
}

/// The mustard brush stroke of a chosen menu entry under `rect`.
pub fn paint_brush(painter: &egui::Painter, rect: Rect, art: &Art) {
    match &art.brush {
        Some(t) => {
            painter.image(t.id(), rect.expand2(vec2(6.0, 4.0)), UV, Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect, CornerRadius::ZERO, MUSTARD);
        }
    }
}

/// The "?" card of the family tree (a mod without a picture) in `rect`.
pub fn paint_card(painter: &egui::Painter, rect: Rect, art: &Art) {
    match &art.card {
        Some(t) => {
            painter.image(t.id(), rect, UV, Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect, CornerRadius::ZERO, rgb(0x8b, 0x72, 0x33));
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "?",
                FontId::proportional(rect.height() * 0.6),
                rgb(0x10, 0x0c, 0x04),
            );
        }
    }
}

/// `texture` cut to fill `rect` (cover).
pub fn paint_cover(painter: &egui::Painter, rect: Rect, texture: &TextureHandle, tint: Color32) {
    let size = texture.size_vec2();
    let (ar, at) = (rect.width() / rect.height(), size.x / size.y);
    let uv = if at > ar {
        let w = ar / at;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = at / ar;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    };
    painter.image(texture.id(), rect, uv, tint);
}

/// Small drawn signs (a 16 × 16 design).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Play,
    Chevron,
    Bolt,
    Shield,
    Warn,
    Check,
    Gear,
    Search,
    Close,
    Box,
}

/// Draws `glyph` into `rect`.
pub fn paint_glyph(painter: &egui::Painter, rect: Rect, glyph: Glyph, color: Color32) {
    let s = rect.width().min(rect.height()) / 16.0;
    let o = rect.center() - vec2(8.0 * s, 8.0 * s);
    let p = |x: f32, y: f32| Pos2::new(o.x + x * s, o.y + y * s);
    let stroke = Stroke::new(1.6 * s.max(0.8), color);
    let line = |pts: Vec<Pos2>| {
        painter.add(Shape::Path(PathShape {
            points: pts,
            closed: false,
            fill: Color32::TRANSPARENT,
            stroke: PathStroke::from(stroke),
        }));
    };
    match glyph {
        Glyph::Play => {
            painter.add(Shape::convex_polygon(
                vec![p(4.5, 3.0), p(13.0, 8.0), p(4.5, 13.0)],
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Chevron => line(vec![p(4.0, 6.0), p(8.0, 10.5), p(12.0, 6.0)]),
        Glyph::Bolt => {
            painter.add(Shape::convex_polygon(
                vec![p(9.5, 1.5), p(3.5, 9.0), p(7.5, 9.0)],
                color,
                Stroke::NONE,
            ));
            painter.add(Shape::convex_polygon(
                vec![p(8.5, 7.0), p(12.5, 7.0), p(6.5, 14.5)],
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Shield => {
            painter.add(Shape::Path(PathShape {
                points: vec![
                    p(8.0, 1.5),
                    p(13.0, 3.5),
                    p(13.0, 7.5),
                    p(11.5, 11.0),
                    p(8.0, 14.5),
                    p(4.5, 11.0),
                    p(3.0, 7.5),
                    p(3.0, 3.5),
                ],
                closed: true,
                fill: Color32::TRANSPARENT,
                stroke: PathStroke::from(stroke),
            }));
            line(vec![p(8.0, 5.0), p(8.0, 8.5)]);
            painter.circle_filled(p(8.0, 11.0), 0.9 * s, color);
        }
        Glyph::Warn => {
            painter.add(Shape::Path(PathShape {
                points: vec![p(8.0, 2.0), p(14.5, 13.5), p(1.5, 13.5)],
                closed: true,
                fill: Color32::TRANSPARENT,
                stroke: PathStroke::from(stroke),
            }));
            line(vec![p(8.0, 6.0), p(8.0, 9.5)]);
            painter.circle_filled(p(8.0, 11.6), 0.9 * s, color);
        }
        Glyph::Check => line(vec![p(3.0, 8.5), p(6.5, 12.0), p(13.0, 4.5)]),
        Glyph::Gear => {
            painter.circle_stroke(rect.center(), 2.6 * s, stroke);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let (sn, cs) = a.sin_cos();
                line(vec![
                    p(8.0 + 4.5 * cs, 8.0 + 4.5 * sn),
                    p(8.0 + 6.5 * cs, 8.0 + 6.5 * sn),
                ]);
            }
        }
        Glyph::Search => {
            painter.circle_stroke(p(7.0, 7.0), 4.2 * s, stroke);
            line(vec![p(10.2, 10.2), p(14.0, 14.0)]);
        }
        Glyph::Close => {
            line(vec![p(4.0, 4.0), p(12.0, 12.0)]);
            line(vec![p(12.0, 4.0), p(4.0, 12.0)]);
        }
        Glyph::Box => {
            painter.add(Shape::Path(PathShape {
                points: vec![
                    p(2.5, 5.0),
                    p(8.0, 2.0),
                    p(13.5, 5.0),
                    p(13.5, 11.0),
                    p(8.0, 14.0),
                    p(2.5, 11.0),
                ],
                closed: true,
                fill: Color32::TRANSPARENT,
                stroke: PathStroke::from(stroke),
            }));
            line(vec![p(2.5, 5.0), p(8.0, 8.0), p(13.5, 5.0)]);
            line(vec![p(8.0, 8.0), p(8.0, 14.0)]);
        }
    }
}

/// A glyph as a widget of `size` points.
pub fn glyph(ui: &mut Ui, glyph: Glyph, color: Color32, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint_glyph(ui.painter(), rect, glyph, color);
    response
}

/// An outlined tag in caps; with `text` empty only the glyph.
pub fn tag(ui: &mut Ui, glyph: Option<Glyph>, text: &str, color: Color32) -> Response {
    let font = caps(ui.ctx(), 11.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_uppercase(), font, color);
    let icon = if glyph.is_some() { 12.0 } else { 0.0 };
    let gap = if glyph.is_some() && !text.is_empty() {
        5.0
    } else {
        0.0
    };
    let size = vec2(icon + gap + galley.size().x + 12.0, 19.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, color),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + 6.0;
    if let Some(g) = glyph {
        paint_glyph(
            painter,
            Rect::from_center_size(pos2(x + 6.0, rect.center().y), vec2(12.0, 12.0)),
            g,
            color,
        );
        x += icon + gap;
    }
    painter.galley(
        pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    response
}

/// The square switch of a mod: rust when on.
pub fn toggle(ui: &mut Ui, on: &mut bool, enabled: bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(36.0, 18.0), Sense::click());
    if enabled && response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let painter = ui.painter();
    let (fill, edge, knob) = if *on {
        (RUST, OXBLOOD, CREAM)
    } else {
        (ASH, RULE, BONE_DIM)
    };
    let alpha = if enabled { 1.0 } else { 0.45 };
    painter.rect(
        rect,
        CornerRadius::ZERO,
        fill.gamma_multiply(alpha),
        Stroke::new(1.0, edge),
        StrokeKind::Inside,
    );
    let x = if *on {
        rect.right() - 16.0
    } else {
        rect.left() + 2.0
    };
    painter.rect_filled(
        Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(12.0, 12.0)),
        CornerRadius::ZERO,
        knob.gamma_multiply(alpha),
    );
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// A button on a rust panel: `primary` fills it, otherwise only its edge.
pub fn button(ui: &mut Ui, text: &str, enabled: bool, primary: bool) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let color = if primary { CREAM } else { BONE };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let size = vec2(galley.size().x + 22.0, 26.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();
    let hover = enabled && response.hovered();
    let alpha = if enabled { 1.0 } else { 0.45 };
    if primary {
        painter.rect_filled(
            rect.translate(vec2(0.0, 2.0)),
            CornerRadius::ZERO,
            OXBLOOD.gamma_multiply(alpha),
        );
        let fill = if hover { rgb(0xc8, 0x46, 0x08) } else { RUST };
        painter.rect_filled(rect, CornerRadius::ZERO, fill.gamma_multiply(alpha));
    } else {
        let fill = if hover { rgb(0x36, 0x2d, 0x1d) } else { SOOT };
        let edge = if hover { SEPIA } else { RULE };
        painter.rect(
            rect,
            CornerRadius::ZERO,
            fill,
            Stroke::new(1.0, edge),
            StrokeKind::Inside,
        );
    }
    painter.galley(
        rect.center() - galley.size() / 2.0,
        galley,
        color.gamma_multiply(alpha),
    );
    response
}

/// What the split Play button reports.
pub struct PlayResponse {
    pub play: Response,
    pub more: Response,
}

/// The big Play button: a rust popup panel of the game with slanted ends
/// and the arrow of its menu. `text` is the label (upper case); `busy`
/// greys it out.
pub fn play_button(ui: &mut Ui, text: &str, enabled: bool, busy: bool) -> PlayResponse {
    let font = display(ui.ctx(), if busy { 18.0 } else { 24.0 });
    let label = if busy {
        text.to_owned()
    } else {
        text.to_uppercase()
    };
    let galley = ui.painter().layout_no_wrap(label, font, CREAM);
    let h = 46.0;
    let glyph_w = if busy { 0.0 } else { 22.0 };
    let main_w = galley.size().x + glyph_w + 50.0;
    let more_w = 40.0;
    let (rect, _) = ui.allocate_exact_size(vec2(main_w + 3.0 + more_w, h + 4.0), Sense::hover());
    let main = Rect::from_min_size(rect.min, vec2(main_w, h));
    let more = Rect::from_min_size(pos2(main.right() + 3.0, rect.top()), vec2(more_w, h));
    let play = ui.interact(
        main,
        ui.scope_id().with("gf2-play"),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let arrow = ui.interact(more, ui.scope_id().with("gf2-play-more"), Sense::click());
    let painter = ui.painter();
    let fill = |r: &Response, base: Color32| {
        if busy {
            ASH
        } else if r.hovered() && r.sense.senses_click() {
            rgb(0xcb, 0x47, 0x08)
        } else {
            base
        }
    };
    paint_panel(painter, main, fill(&play, RUST), (h * 0.18, 0.0));
    paint_panel(painter, more, fill(&arrow, EMBER), (0.0, h * 0.12));
    let color = if busy { BONE } else { CREAM };
    let mut x = main.left() + 22.0;
    if !busy {
        paint_glyph(
            painter,
            Rect::from_center_size(pos2(x + 8.0, main.center().y), vec2(16.0, 16.0)),
            Glyph::Play,
            color,
        );
        x += glyph_w;
    }
    painter.galley(
        pos2(x, main.center().y - galley.size().y / 2.0 + 1.0),
        galley,
        color,
    );
    paint_glyph(
        painter,
        Rect::from_center_size(more.center(), vec2(16.0, 16.0)),
        Glyph::Chevron,
        color,
    );
    if enabled && play.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    PlayResponse { play, more: arrow }
}

/// A dashed frame (where a file can be dropped).
pub fn dashed_frame(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let c = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    painter.extend(Shape::dashed_line(&c, stroke, 7.0, 5.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_cut_to_its_opaque_part() {
        // 4 × 3, one opaque pixel at (2, 1).
        let mut rgba = vec![0u8; 4 * 3 * 4];
        rgba[(4 + 2) * 4..(4 + 2) * 4 + 4].copy_from_slice(&[9, 8, 7, 255]);
        let image = cut(4, 3, &rgba, 0, 0, 4, 3).unwrap();
        assert_eq!(image.size, [1, 1]);
        assert!(cut(4, 3, &rgba, 0, 0, 4, 1).is_none(), "nothing opaque");
        assert!(cut(4, 3, &rgba, 0, 0, 5, 3).is_none(), "out of the picture");
    }

    #[test]
    fn every_language_of_the_interface_has_a_logo() {
        for (code, _) in crate::i18n::languages() {
            let (a, b) = logo_rows(code);
            assert!(a < b && b <= 2048);
        }
    }
}
