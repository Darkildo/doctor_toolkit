//! "As in the game" shading: the light, sky and fog of the game's lit pass
//! ported to the scene shader (`docs/analysis/renderer.md` §10).
//!
//! The game draws with a light pre-pass: the sun (`saturate(n·l + wrap)` times
//! its colour) and the ambient light of three 4×4 matrices go into an 8-bit
//! light buffer; the lit pass multiplies the albedo (base texture in linear
//! space) by that buffer and by 2, mixes the fog in and writes
//! `pow(colour, 1/2.2)`. The sky is a dome coloured by height above the
//! horizon with a glow around the sun. The values come from the time-of-day
//! keys of the city ([`doctor_toolkit::time_of_day`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use doctor_toolkit::time_of_day::{self, ColourTable, Look};

use super::math::{self, Vec3};

/// Colour tables of the grading by key (the `vfx` id of the time-of-day
/// key), registered when a city's light is read; the renderer uploads a
/// table the first time a look names it.
static TABLES: Mutex<Option<HashMap<u32, Arc<ColourTable>>>> = Mutex::new(None);

/// Makes colour tables known to every renderer.
pub fn register_tables(tables: impl IntoIterator<Item = (u32, ColourTable)>) {
    let mut lock = TABLES.lock().unwrap_or_else(|e| e.into_inner());
    let map = lock.get_or_insert_with(HashMap::new);
    for (key, table) in tables {
        map.entry(key).or_insert_with(|| Arc::new(table));
    }
}

/// A registered colour table.
pub fn table(key: u32) -> Option<Arc<ColourTable>> {
    TABLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(&key)
        .cloned()
}

/// Light and air of the game at one moment, ready for the renderer.
/// Colours of light are linear; fog and sky colours as the game stores them
/// (gamma-encoded), the shader converts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameLook {
    /// Direction towards the sun (or moon).
    pub sun: Vec3,
    /// Colour of the sun's light in the light buffer (linear, 0..1).
    pub sun_color: [f32; 3],
    /// Added to `n·l` before clamping (`g_diffuseAmbient`).
    pub wrap: f32,
    /// Ambient light: per channel `(n, 1)ᵀ · M · (n, 1)`, row-major.
    pub ambient: [[f32; 16]; 3],
    /// Multiplies the ambient matrices (`g_lightColorSH`).
    pub ambient_scale: f32,
    /// The lit pass's factor on albedo × light (2 in the game).
    pub exposure: f32,
    pub fog: Option<time_of_day::Fog>,
    pub sky: Option<time_of_day::Sky>,
    /// `g_NightControl`: lit windows and panoramas (0 day, 1 night).
    pub night: f32,
    /// Share of the sun left in the shadow of the sun (`1 −` the
    /// `LightingTODMessage` shadow).
    pub shadow: f32,
    /// The colour grading: colour tables ([`register_tables`]) and weights.
    pub grading: [(u32, f32); 2],
    pub grading_count: usize,
}

/// Brightness of the sun's light at noon in the light buffer, and the
/// share of the ambient matrices: not keys of the time of day (the game sets
/// them in code); matched to frames of the game (`renderer.md` §10.6) once
/// world surfaces had their tints and baked occlusion (§10.7). The sun
/// saturates the 8-bit buffer on surfaces facing it, as in the game.
/// Matched again with the colour grading (stage 70, `renderer.md` §10.9).
const SUN_AT_NOON: f32 = 1.1;
const AMBIENT_SCALE: f32 = 0.3;
/// How much the sun dims with the sky by day (1: as much as the sky).
const DAY_EXPONENT: f32 = 0.6;
const SUN_TINT: [f32; 3] = [1.0, 0.85, 0.6];
const MOON_TINT: [f32; 3] = [0.6, 0.85, 1.0];
/// The studio light of the model and character viewers (their surfaces were
/// matched with it before world surfaces were).
const STUDIO_SUN: f32 = 0.42;

fn luminance(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

impl GameLook {
    /// The look of a moment of a city.
    pub fn from_look(look: &Look) -> Self {
        // The sun follows the brightness of the sky above the haze: full at
        // noon, the dim moon at night.
        let sky = look.sky.high.map(|c| c.powf(2.2));
        let noon_sky = 0.75;
        // By day the sun hardly dims with a paler sky (New York and Miami
        // have paler noon skies than Havana); the moon follows the sky.
        let exponent = DAY_EXPONENT + (1.0 - DAY_EXPONENT) * look.night;
        let strength = (luminance(sky) / noon_sky).clamp(0.08, 1.2).powf(exponent);
        // A warm sun, a cool moon (the night's colour table warms it again).
        let tint: [f32; 3] =
            std::array::from_fn(|i| SUN_TINT[i] + (MOON_TINT[i] - SUN_TINT[i]) * look.night);
        let sun_color = tint.map(|c| c * SUN_AT_NOON * strength);
        Self {
            // Towards the sun. The shadows of characters in the game's
            // frames fall the other way round than the horizontal part of
            // the measured light vector (renderer.md §10.9): up stays up,
            // the bearing turns by half a circle.
            sun: math::normalize_or([look.sun[0], -look.sun[1], look.sun[2]], [0.0, 1.0, 0.0]),
            sun_color,
            wrap: look.lighting.wrap,
            ambient: look.ambient.0,
            ambient_scale: AMBIENT_SCALE,
            exposure: 2.0,
            fog: Some(look.fog),
            sky: Some(look.sky),
            night: look.night,
            shadow: (1.0 - look.lighting.shadow).clamp(0.0, 1.0),
            grading: look.grading.tables,
            grading_count: look.grading.count,
        }
    }

    /// The colour tables and their weights.
    pub fn grading(&self) -> &[(u32, f32)] {
        &self.grading[..self.grading_count]
    }

    /// The light of the viewers of single models and characters: the game's
    /// light at midday without fog or sky.
    pub fn studio() -> Self {
        let mut ambient = [0.0; 16];
        ambient[15] = 0.42;
        // A little more from above than from below.
        // Rows y and w: the y·1 and 1·y terms.
        ambient[4 + 3] = 0.04;
        ambient[3 * 4 + 1] = 0.04;
        Self {
            sun: math::normalize_or([0.35, 0.9, 0.45], [0.0, 1.0, 0.0]),
            sun_color: [STUDIO_SUN, STUDIO_SUN * 0.95, STUDIO_SUN * 0.85],
            wrap: 0.25,
            ambient: [ambient; 3],
            ambient_scale: 1.0,
            exposure: 2.0,
            fog: None,
            sky: None,
            night: 0.0,
            shadow: 1.0,
            grading: [(0, 0.0); 2],
            grading_count: 0,
        }
    }

    /// The colour behind the scene where no sky is drawn: the fog colour at
    /// its end, or the sky colour at the horizon (gamma-encoded RGBA).
    pub fn background(&self) -> Option<[f32; 4]> {
        let c = self.sky.map(|s| s.low).or(self.fog.map(|f| f.far_color))?;
        Some([c[0], c[1], c[2], 1.0])
    }
}

/// The game-look part of the scene uniforms.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct GameUniform {
    /// Inverse of the view-projection: the sky's view rays.
    pub inv_view_proj: [f32; 16],
    /// rgb: sun colour, w: wrap.
    pub sun: [f32; 4],
    /// x: exposure, y: ambient scale, z: 1 when fog is on, w: 1 when the sky is drawn.
    pub params: [f32; 4],
    /// Rows of the ambient matrices: R 0–3, G 4–7, B 8–11.
    pub ambient: [[f32; 4]; 12],
    /// rgb: fog colour at the start (linear), w: amount at the start.
    pub fog_near: [f32; 4],
    /// rgb: fog colour at the end (linear), w: amount at the end.
    pub fog_far: [f32; 4],
    /// x: start, y: 1 / (end − start).
    pub fog_range: [f32; 4],
    pub sky_low: [f32; 4],
    pub sky_high: [f32; 4],
    pub glow_low: [f32; 4],
    pub glow_high: [f32; 4],
    /// x: haze band (rad), y: glow strength, z: glow exponent above the band, w: at the horizon.
    pub sky_params: [f32; 4],
    /// x: night, y: the sun left in shadow, z: shadow cascades, w: lights on.
    pub extra: [f32; 4],
    /// x: tile size in pixels, y: tiles across, z: tiles down, w: lights per tile.
    pub tiles: [f32; 4],
    /// World to each cascade's shadow map.
    pub shadow: [[f32; 16]; 3],
    /// Far end of each cascade; w: depth bias.
    pub splits: [f32; 4],
}

fn rgba(c: [f32; 3], w: f32) -> [f32; 4] {
    [c[0], c[1], c[2], w]
}

fn linear(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| v.max(0.0).powf(2.2))
}

impl GameUniform {
    pub fn new(look: Option<&GameLook>, view_proj: &math::Mat4) -> Self {
        let mut out = Self {
            inv_view_proj: math::inverse(view_proj).unwrap_or(math::IDENTITY),
            ..Self::default()
        };
        let Some(look) = look else {
            return out;
        };
        out.sun = rgba(look.sun_color, look.wrap);
        out.params = [
            look.exposure,
            look.ambient_scale,
            look.fog.is_some() as u32 as f32,
            look.sky.is_some() as u32 as f32,
        ];
        for (c, m) in look.ambient.iter().enumerate() {
            for r in 0..4 {
                out.ambient[c * 4 + r] = [m[r * 4], m[r * 4 + 1], m[r * 4 + 2], m[r * 4 + 3]];
            }
        }
        if let Some(f) = look.fog {
            out.fog_near = rgba(linear(f.color), f.near_amount);
            out.fog_far = rgba(linear(f.far_color), f.far_amount);
            out.fog_range = [f.start, 1.0 / (f.end - f.start).max(1e-3), 0.0, 0.0];
        }
        if let Some(s) = look.sky {
            out.sky_low = rgba(s.low, 1.0);
            out.sky_high = rgba(s.high, 1.0);
            out.glow_low = rgba(s.glow_low, 1.0);
            out.glow_high = rgba(s.glow_high, 1.0);
            out.sky_params = [s.haze, s.glow_strength, s.glow_power_high, s.glow_power_low];
        }
        out.extra = [look.night, look.shadow, 0.0, 0.0];
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::time_of_day::{Ambient, Fog, Lighting, Sky};

    fn look(sky_high: [f32; 3]) -> Look {
        Look {
            fog: Fog::default(),
            sky: Sky {
                high: sky_high,
                ..Sky::default()
            },
            lighting: Lighting::default(),
            ambient: Ambient::default(),
            sun: [0.0, -1.0, 0.0],
            night: 0.0,
            grading: Default::default(),
        }
    }

    #[test]
    fn the_sun_dims_with_the_sky() {
        let day = GameLook::from_look(&look([0.7, 0.9, 0.97]));
        let night = GameLook::from_look(&look([0.2, 0.22, 0.22]));
        assert!(day.sun_color[0] > 4.0 * night.sun_color[0]);
        assert_eq!(day.sun, [0.0, 1.0, 0.0], "towards the sun");
        assert!(day.background().is_some());
        assert!(GameLook::studio().background().is_none());
    }

    #[test]
    fn uniform_rows_follow_the_matrices() {
        let mut l = GameLook::studio();
        l.ambient[1][7] = 5.0; // G, row 1, column 3
        let u = GameUniform::new(Some(&l), &math::IDENTITY);
        assert_eq!(u.ambient[5][3], 5.0);
        assert_eq!(u.params[0], 2.0);
        assert_eq!(u.params[2], 0.0, "no fog in the studio");
        assert_eq!(GameUniform::new(None, &math::IDENTITY).params, [0.0; 4]);
    }
}
