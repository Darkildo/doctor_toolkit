//! The program icons, drawn from a few shapes so that no image file with an
//! unclear licence is needed: a dark rounded square with a cross ("doctor")
//! and a small block in its middle (a resource). The editor's cross is red,
//! the mod manager's green.
//!
//! The same shapes give the window icon ([`icon_data`]), PNG files of every
//! size the desktop wants and an SVG ([`write_icons`], used by
//! `doctor-toolkit-gui --write-icons` and `tools/install-desktop.sh`).

use std::path::{Path, PathBuf};

/// Which program the icon is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Editor,
    Mods,
}

impl Variant {
    pub const ALL: [Variant; 2] = [Variant::Editor, Variant::Mods];

    /// Icon name of the desktop entry and file name of the icon.
    pub fn name(self) -> &'static str {
        match self {
            Self::Editor => "doctor-toolkit",
            Self::Mods => "open-mod-manager",
        }
    }

    fn accent(self) -> [u8; 3] {
        match self {
            Self::Editor => [0xe0, 0x48, 0x48],
            Self::Mods => [0x4c, 0xb8, 0x60],
        }
    }
}

/// Sizes of the PNG icons for the desktop.
pub const SIZES: [u32; 8] = [16, 22, 24, 32, 48, 64, 128, 256];

const TOP: [u8; 3] = [0x2e, 0x3d, 0x4e];
const BOTTOM: [u8; 3] = [0x14, 0x1d, 0x27];

/// A rounded rectangle in units of the icon size: centre, half size, radius.
#[derive(Clone, Copy)]
struct Rounded {
    center: [f32; 2],
    half: [f32; 2],
    radius: f32,
}

impl Rounded {
    const fn new(cx: f32, cy: f32, hw: f32, hh: f32, radius: f32) -> Self {
        Self {
            center: [cx, cy],
            half: [hw, hh],
            radius,
        }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        let qx = (x - self.center[0]).abs() - (self.half[0] - self.radius);
        let qy = (y - self.center[1]).abs() - (self.half[1] - self.radius);
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        outside + qx.max(qy).min(0.0) <= self.radius
    }

    fn svg(&self, size: f32, fill: &str) -> String {
        format!(
            r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{fill}"/>"#,
            (self.center[0] - self.half[0]) * size,
            (self.center[1] - self.half[1]) * size,
            self.half[0] * 2.0 * size,
            self.half[1] * 2.0 * size,
            self.radius * size
        )
    }
}

const BACKGROUND: Rounded = Rounded::new(0.5, 0.5, 0.46, 0.46, 0.2);
const CROSS: [Rounded; 2] = [
    Rounded::new(0.5, 0.5, 0.31, 0.115, 0.04),
    Rounded::new(0.5, 0.5, 0.115, 0.31, 0.04),
];
const BLOCK: Rounded = Rounded::new(0.5, 0.5, 0.055, 0.055, 0.015);

/// Colour and coverage of one point of the icon.
fn shade(variant: Variant, x: f32, y: f32) -> Option<[u8; 3]> {
    if BLOCK.contains(x, y) {
        return Some(BOTTOM);
    }
    if CROSS.iter().any(|c| c.contains(x, y)) {
        return Some(variant.accent());
    }
    if BACKGROUND.contains(x, y) {
        let t = ((y - 0.04) / 0.92).clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        return Some([
            mix(TOP[0], BOTTOM[0]),
            mix(TOP[1], BOTTOM[1]),
            mix(TOP[2], BOTTOM[2]),
        ]);
    }
    None
}

/// The icon as RGBA pixels (straight alpha), `size` × `size`, with 4×4
/// samples per pixel for smooth edges.
pub fn rgba(variant: Variant, size: u32) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut sum = [0u32; 3];
            let mut covered = 0u32;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    if let Some(c) = shade(variant, x, y) {
                        covered += 1;
                        for (s, v) in sum.iter_mut().zip(c) {
                            *s += v as u32;
                        }
                    }
                }
            }
            if covered == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                out.extend(sum.map(|s| (s / covered) as u8));
                out.push((covered * 255 / (SAMPLES * SAMPLES)) as u8);
            }
        }
    }
    out
}

/// The window icon.
pub fn icon_data(variant: Variant) -> egui::IconData {
    const SIZE: u32 = 64;
    egui::IconData {
        rgba: rgba(variant, SIZE),
        width: SIZE,
        height: SIZE,
    }
}

/// The icon as an SVG document.
pub fn svg(variant: Variant) -> String {
    const SIZE: f32 = 256.0;
    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
<defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient></defs>
"#,
        hex(TOP),
        hex(BOTTOM)
    );
    out += &BACKGROUND.svg(SIZE, "url(#bg)");
    out.push('\n');
    for arm in CROSS {
        out += &arm.svg(SIZE, &hex(variant.accent()));
        out.push('\n');
    }
    out += &BLOCK.svg(SIZE, &hex(BOTTOM));
    out += "\n</svg>\n";
    out
}

/// Writes the icons of both programs as an icon theme tree under `dir`:
/// `hicolor/<n>x<n>/apps/<name>.png` and `hicolor/scalable/apps/<name>.svg`.
pub fn write_icons(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for variant in Variant::ALL {
        for size in SIZES {
            let folder = dir
                .join("hicolor")
                .join(format!("{size}x{size}"))
                .join("apps");
            std::fs::create_dir_all(&folder)?;
            let path = folder.join(format!("{}.png", variant.name()));
            let png = doctor_toolkit::resource::pixels::encode_png(
                size as usize,
                size as usize,
                &rgba(variant, size),
            )
            .map_err(std::io::Error::other)?;
            std::fs::write(&path, png)?;
            written.push(path);
        }
        let folder = dir.join("hicolor").join("scalable").join("apps");
        std::fs::create_dir_all(&folder)?;
        let path = folder.join(format!("{}.svg", variant.name()));
        std::fs::write(&path, svg(variant))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_transparent_and_centre_is_drawn() {
        let size = 32;
        let pixels = rgba(Variant::Editor, size);
        assert_eq!(pixels.len(), (size * size * 4) as usize);
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
        };
        assert_eq!(at(0, 0)[3], 0);
        assert_eq!(at(size - 1, size - 1)[3], 0);
        // The block in the middle, the cross around it, the background at the edge.
        assert_eq!(at(16, 16), [BOTTOM[0], BOTTOM[1], BOTTOM[2], 255]);
        let arm = at(16, 9);
        assert_eq!(&arm[..3], &Variant::Editor.accent());
        assert_eq!(arm[3], 255);
        assert_ne!(at(4, 16)[..3], Variant::Editor.accent());
        assert_eq!(at(4, 16)[3], 255);
    }

    #[test]
    fn variants_differ_and_svg_is_complete() {
        assert_ne!(rgba(Variant::Editor, 16), rgba(Variant::Mods, 16));
        let svg = svg(Variant::Mods);
        assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
        assert_eq!(svg.matches("<rect").count(), 4);
    }

    #[test]
    fn writes_an_icon_theme_tree() {
        let dir = std::env::temp_dir().join(format!("dt-icons-{}", std::process::id()));
        let written = write_icons(&dir).unwrap();
        assert_eq!(written.len(), 2 * (SIZES.len() + 1));
        let png = std::fs::read(dir.join("hicolor/48x48/apps/doctor-toolkit.png")).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
