//! The fonts of the overlay and the atlas they are drawn from.
//!
//! The two fonts are the ones the mod manager shows the game's menus with
//! (`crates/doctor-toolkit-mods/fonts`, SIL Open Font License): Oswald
//! Medium for the items, Podkova ExtraBold for the titles. They ship in the
//! mod unmodified and are rasterised when the overlay first draws, into one
//! atlas with a white corner for the solid rectangles.

use std::collections::HashMap;

/// The fonts of the overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    /// Oswald Medium: the items of a menu, text.
    Text,
    /// Podkova ExtraBold: titles.
    Title,
}

impl Face {
    pub const ALL: [Face; 2] = [Face::Text, Face::Title];

    /// The pixel size the face is rasterised at; drawn at other sizes it is
    /// scaled (linear filtering).
    pub fn raster_px(self) -> f32 {
        match self {
            // Sharp up to about 1080 lines with the sizes of a menu.
            Face::Text => 48.0,
            Face::Title => 64.0,
        }
    }

    fn data(self) -> &'static [u8] {
        match self {
            Face::Text => include_bytes!("../../doctor-toolkit-mods/fonts/Oswald-Medium.ttf"),
            Face::Title => include_bytes!("../../doctor-toolkit-mods/fonts/Podkova-ExtraBold.ttf"),
        }
    }
}

/// The licence texts of the fonts, for the package of a mod.
pub const FONT_LICENCES: [(&str, &str); 2] = [
    (
        "OFL-Oswald.txt",
        include_str!("../../doctor-toolkit-mods/fonts/OFL-Oswald.txt"),
    ),
    (
        "OFL-Podkova.txt",
        include_str!("../../doctor-toolkit-mods/fonts/OFL-Podkova.txt"),
    ),
];

/// The characters in the atlas: Latin, Cyrillic, the typography of menus.
/// Any other character is drawn as `?`.
pub fn charset() -> impl Iterator<Item = char> {
    (' '..='~').chain('А'..='я').chain([
        'Ё', 'ё', '«', '»', '—', '–', '…', '№', '•', '×', '·', '’', '°', '±',
    ])
}

/// A glyph in the atlas, in pixels of the raster size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Glyph {
    /// Its rectangle in the atlas.
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// From the pen to the left edge, and from the baseline up to the top.
    pub left: f32,
    pub top: f32,
    pub advance: f32,
}

/// One face: its glyphs and line metrics, in pixels of the raster size.
pub struct Font {
    pub raster_px: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    glyphs: HashMap<char, Glyph>,
    kern: fontdue::Font,
}

impl Font {
    /// The glyph of a character, `?` for one outside the atlas.
    pub fn glyph(&self, c: char) -> Glyph {
        self.glyphs
            .get(&c)
            .or_else(|| self.glyphs.get(&'?'))
            .copied()
            .unwrap_or_default()
    }

    /// Kerning between two characters, pixels of the raster size.
    pub fn kern(&self, a: char, b: char) -> f32 {
        self.kern
            .horizontal_kern(a, b, self.raster_px)
            .unwrap_or(0.0)
    }

    /// Width of a line of text at `px` pixels.
    pub fn measure(&self, text: &str, px: f32) -> f32 {
        let scale = px / self.raster_px;
        let mut width = 0.0;
        let mut previous = None;
        for c in text.chars() {
            if let Some(p) = previous {
                width += self.kern(p, c);
            }
            width += self.glyph(c).advance;
            previous = Some(c);
        }
        width * scale
    }

    /// Height of a line at `px` pixels.
    pub fn line_height(&self, px: f32) -> f32 {
        (self.ascent - self.descent + self.line_gap) * px / self.raster_px
    }
}

/// The atlas: alpha coverage, 8 bits a pixel, and the two fonts.
pub struct Atlas {
    pub width: u32,
    pub height: u32,
    /// `width * height` bytes; the corner `0..WHITE` × `0..WHITE` is solid.
    pub alpha: Vec<u8>,
    pub fonts: HashMap<Face, Font>,
}

/// The solid corner, for rectangles: sampled at its middle.
pub const WHITE: u32 = 4;
const PAD: u32 = 2;

impl Atlas {
    /// Rasterises both faces into one atlas, 1024 pixels wide.
    pub fn build() -> Result<Self, String> {
        let width = 1024u32;
        let mut rasters = Vec::new();
        let mut fonts = HashMap::new();
        for face in Face::ALL {
            let font = fontdue::Font::from_bytes(face.data(), fontdue::FontSettings::default())
                .map_err(|e| format!("{face:?}: {e}"))?;
            let px = face.raster_px();
            let line = font
                .horizontal_line_metrics(px)
                .ok_or_else(|| format!("{face:?}: no horizontal metrics"))?;
            for c in charset() {
                let (metrics, bitmap) = font.rasterize(c, px);
                rasters.push((face, c, metrics, bitmap));
            }
            fonts.insert(
                face,
                Font {
                    raster_px: px,
                    ascent: line.ascent,
                    descent: line.descent,
                    line_gap: line.line_gap,
                    glyphs: HashMap::new(),
                    kern: font,
                },
            );
        }
        // Shelves, the tallest first; the white corner opens the first one.
        let mut order: Vec<usize> = (0..rasters.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(rasters[i].2.height));
        let (mut x, mut y, mut shelf) = (WHITE + PAD, 0u32, WHITE);
        let mut places = vec![(0u32, 0u32); rasters.len()];
        for &i in &order {
            let m = &rasters[i].2;
            let (w, h) = (m.width as u32, m.height as u32);
            if x + w + PAD > width {
                x = 0;
                y += shelf + PAD;
                shelf = 0;
            }
            places[i] = (x, y);
            x += w + PAD;
            shelf = shelf.max(h);
        }
        let height = (y + shelf).next_power_of_two().max(64);
        let mut alpha = vec![0u8; (width * height) as usize];
        for row in 0..WHITE {
            alpha[(row * width) as usize..(row * width + WHITE) as usize].fill(255);
        }
        for (i, (face, c, m, bitmap)) in rasters.into_iter().enumerate() {
            let (px, py) = places[i];
            for row in 0..m.height {
                let to = ((py + row as u32) * width + px) as usize;
                alpha[to..to + m.width]
                    .copy_from_slice(&bitmap[row * m.width..(row + 1) * m.width]);
            }
            let glyph = Glyph {
                x: px as u16,
                y: py as u16,
                width: m.width as u16,
                height: m.height as u16,
                left: m.xmin as f32,
                top: m.ymin as f32 + m.height as f32,
                advance: m.advance_width,
            };
            if let Some(font) = fonts.get_mut(&face) {
                font.glyphs.insert(c, glyph);
            }
        }
        Ok(Self {
            width,
            height,
            alpha,
            fonts,
        })
    }

    pub fn font(&self, face: Face) -> &Font {
        &self.fonts[&face]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_atlas_holds_latin_and_cyrillic_without_overlap() {
        let atlas = Atlas::build().unwrap();
        assert_eq!(atlas.width, 1024);
        assert!(
            atlas.height <= 1024,
            "atlas {}x{}",
            atlas.width,
            atlas.height
        );
        let mut rects = Vec::new();
        for face in Face::ALL {
            let font = atlas.font(face);
            for c in ['A', 'z', 'Ж', 'ё', '«', '—'] {
                let g = font.glyph(c);
                assert!(g.width > 0 && g.height > 0, "{face:?} {c}");
                assert!(g.advance > 0.0);
            }
            for c in charset() {
                let g = font.glyph(c);
                if g.width > 0 {
                    rects.push((g.x as u32, g.y as u32, g.width as u32, g.height as u32));
                }
                assert!(u32::from(g.x) + u32::from(g.width) <= atlas.width);
                assert!(u32::from(g.y) + u32::from(g.height) <= atlas.height);
            }
        }
        rects.push((0, 0, WHITE, WHITE));
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let apart =
                    a.0 + a.2 <= b.0 || b.0 + b.2 <= a.0 || a.1 + a.3 <= b.1 || b.1 + b.3 <= a.1;
                assert!(apart, "{a:?} {b:?}");
            }
        }
        assert_eq!(atlas.alpha[0], 255);
        assert_eq!(atlas.alpha[(WHITE - 1) as usize], 255);
    }

    #[test]
    fn text_is_measured_at_any_size() {
        let atlas = Atlas::build().unwrap();
        let font = atlas.font(Face::Text);
        let w = font.measure("Меню разработчика", 34.0);
        assert!(w > 100.0);
        assert!((font.measure("Меню разработчика", 17.0) * 2.0 - w).abs() < 0.01);
        // A character outside the atlas is drawn as `?`.
        assert_eq!(font.glyph('漢'), font.glyph('?'));
        assert!(font.line_height(34.0) > 34.0 * 0.9);
    }
}
