//! Light, sky and fog of a city by the time of day, the way the game keys
//! them (`docs/analysis/renderer.md` §10).
//!
//! The game keeps no tables of light in its executable: each city's `local`
//! stream holds a `TODSequencer` (one outdoors, one for interiors) and the
//! keys of it — SimGroup objects of the `TODMessage` family, each with a time
//! (`TODMessage` attributes 0–2: hours, minutes, seconds) and the GUID of its
//! sequencer (attribute 4). Between two keys of a kind the game interpolates
//! linearly and wraps around midnight (`0x0082DF70`). The kinds read here:
//!
//! | Class | Attributes | Shader constants |
//! |---|---|---|
//! | `SetFogMessage` | 0 near colour, 1 far colour, 2 start, 3 end (m), 5 amount at the start, 6 at the end | `m_FogColor`, `m_FogEndColor`, `m_FogStartEndParams` |
//! | `SetSkyMessage` (`0x0D0F12E8`) | 2, 3 colours of the sky above the haze band and at the horizon, 6, 7 colours of the glow around the sun above and at the horizon, 1 its strength, 8, 10 its exponents, 12 height of the haze band (rad) | `g_SkyColorLow`, `g_SkyColor`, `g_DiffusionColor*`, `g_DiffusionControl*`, `g_HazeBandElevationAngle` |
//! | `LightingTODMessage` | 0 how dark shadows are, 1 light that wraps around the shaded side | `g_shadowDensity`, `g_diffuseAmbient` |
//! | `GlobalSHTODMessage` | 0–2: 4×4 matrices of the ambient light (R, G, B) | `g_SHMatrixR/G/B` |
//! | `NightDayControlTODMessage` | 0 night (1) or day (0, also when unset) | `g_NightControl`: lit windows and panoramas |
//! | `LUTTODMessage` | 0 a `vfx` of the colour grading, 1 its weight | `gLUTTexture1…6`, `gLUT_params1/2` (stage 70) |
//!
//! The colour grading is a blend of 32³ colour tables (`lutcolor_*` volume
//! textures): between two keys the game weighs the table of the earlier key
//! by `1 − t` and of the later one by `t`, the same `t` as of the other keys
//! (`live peek` of `0x0121930C`, `renderer.md` §10.9).
//!
//! The direction of the sun is not a key of these: the game turns it along
//! a path of its own; [`sun_direction`] gives the path measured in the game.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::game::GameInstance;
use crate::resource::simgroup::{Guid, SimGroup, Value};

/// `TODMessage`: the time and sequencer of a key.
const CLASS_TOD_MESSAGE: u32 = sdbm_lower(b"TODMessage");
const CLASS_TOD_SEQUENCER: u32 = sdbm_lower(b"TODSequencer");
const CLASS_SET_FOG: u32 = sdbm_lower(b"SetFogMessage");
const CLASS_SET_SKY: u32 = sdbm_lower(b"SetSkyMessage");
const CLASS_LIGHTING: u32 = sdbm_lower(b"LightingTODMessage");
const CLASS_GLOBAL_SH: u32 = sdbm_lower(b"GlobalSHTODMessage");
const CLASS_NIGHT_DAY: u32 = sdbm_lower(b"NightDayControlTODMessage");
const CLASS_LUT: u32 = sdbm_lower(b"LUTTODMessage");
const SIMGROUP: u32 = crate::hash::sdbm(b"simgroup");

/// SDBM of a class name in lower case: the hash SimGroups store for a class.
const fn sdbm_lower(name: &[u8]) -> u32 {
    let mut hash = 0u32;
    let mut i = 0;
    while i < name.len() {
        hash = (name[i].to_ascii_lowercase() as u32).wrapping_add(hash.wrapping_mul(65599));
        i += 1;
    }
    hash
}

/// Seconds in a day.
pub const DAY: u32 = 86_400;

/// Fog of the lit shaders: `mix(colour, far colour, f)` weighted by
/// `mix(near amount, far amount, f)`, `f = clamp((d − start) / (end − start))`;
/// no fog at all where `f` is 0. Colours are as stored (gamma-encoded).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub color: [f32; 3],
    pub far_color: [f32; 3],
    pub start: f32,
    pub end: f32,
    pub near_amount: f32,
    pub far_amount: f32,
}

/// The sky dome (`sky_dome` pixel shader). Colours are as stored
/// (the sky shader writes them without conversion).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sky {
    /// At the horizon.
    pub low: [f32; 3],
    /// Above the haze band.
    pub high: [f32; 3],
    /// Glow around the sun (or moon) at the horizon and above the band.
    pub glow_low: [f32; 3],
    pub glow_high: [f32; 3],
    pub glow_strength: f32,
    /// Exponents of the glow (cosine to the sun) above the band and at the horizon.
    pub glow_power_high: f32,
    pub glow_power_low: f32,
    /// Height of the haze band above the horizon, radians.
    pub haze: f32,
}

/// Shadows and wrapped light (`LightingTODMessage`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lighting {
    /// How much of the sun a shadow takes away (0..1).
    pub shadow: f32,
    /// Added to `n·l` before clamping: light wrapping around the shaded side.
    pub wrap: f32,
}

/// Ambient light as three 4×4 matrices: `irradiance = (n, 1)ᵀ · M · (n, 1)`
/// per channel (rows of `g_SHMatrixR/G/B`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ambient(pub [[f32; 16]; 3]);

impl Ambient {
    /// The ambient light from direction `n` (unit, world space, Y up).
    pub fn eval(&self, n: [f32; 3]) -> [f32; 3] {
        let v = [n[0], n[1], n[2], 1.0];
        self.0.map(|m| {
            let mut sum = 0.0;
            for (r, vr) in v.iter().enumerate() {
                for (c, vc) in v.iter().enumerate() {
                    sum += vr * m[r * 4 + c] * vc;
                }
            }
            sum
        })
    }
}

/// A key of the colour grading: the `vfx` resource naming a colour table
/// (`LUTTODMessage` attribute 0) and its weight (attribute 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grading {
    pub vfx: u32,
    pub weight: f32,
}

/// The colour grading of a moment: up to two tables and their weights
/// (`vfx` ids; the same table twice is merged into one).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GradingMix {
    pub tables: [(u32, f32); 2],
    pub count: usize,
}

impl GradingMix {
    pub fn tables(&self) -> &[(u32, f32)] {
        &self.tables[..self.count]
    }
}

/// Everything of one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub fog: Fog,
    pub sky: Sky,
    pub lighting: Lighting,
    pub ambient: Ambient,
    /// Direction the sun (or moon) light travels: from the sky downwards.
    pub sun: [f32; 3],
    /// `g_NightControl`: 0 by day, 1 at night (lit windows, glowing
    /// panoramas, street lights).
    pub night: f32,
    /// The colour grading; empty when the city has no keys of it.
    pub grading: GradingMix,
}

/// Keys of one sequencer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sequence {
    pub name: String,
    pub fog: Vec<(u32, Fog)>,
    pub sky: Vec<(u32, Sky)>,
    pub lighting: Vec<(u32, Lighting)>,
    pub ambient: Vec<(u32, Ambient)>,
    pub night: Vec<(u32, f32)>,
    pub grading: Vec<(u32, Grading)>,
}

/// The two sequencers of a city.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CityLight {
    pub city: String,
    pub outdoor: Sequence,
    pub indoor: Sequence,
}

trait Lerp: Copy {
    fn lerp(&self, other: &Self, t: f32) -> Self;
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| mix(a[i], b[i], t))
}

impl Lerp for Fog {
    fn lerp(&self, o: &Self, t: f32) -> Self {
        Self {
            color: mix3(self.color, o.color, t),
            far_color: mix3(self.far_color, o.far_color, t),
            start: mix(self.start, o.start, t),
            end: mix(self.end, o.end, t),
            near_amount: mix(self.near_amount, o.near_amount, t),
            far_amount: mix(self.far_amount, o.far_amount, t),
        }
    }
}

impl Lerp for Sky {
    fn lerp(&self, o: &Self, t: f32) -> Self {
        Self {
            low: mix3(self.low, o.low, t),
            high: mix3(self.high, o.high, t),
            glow_low: mix3(self.glow_low, o.glow_low, t),
            glow_high: mix3(self.glow_high, o.glow_high, t),
            glow_strength: mix(self.glow_strength, o.glow_strength, t),
            glow_power_high: mix(self.glow_power_high, o.glow_power_high, t),
            glow_power_low: mix(self.glow_power_low, o.glow_power_low, t),
            haze: mix(self.haze, o.haze, t),
        }
    }
}

impl Lerp for Lighting {
    fn lerp(&self, o: &Self, t: f32) -> Self {
        Self {
            shadow: mix(self.shadow, o.shadow, t),
            wrap: mix(self.wrap, o.wrap, t),
        }
    }
}

impl Lerp for f32 {
    fn lerp(&self, o: &Self, t: f32) -> Self {
        mix(*self, *o, t)
    }
}

impl Lerp for Ambient {
    fn lerp(&self, o: &Self, t: f32) -> Self {
        Self(std::array::from_fn(|c| {
            std::array::from_fn(|i| mix(self.0[c][i], o.0[c][i], t))
        }))
    }
}

/// The key before `time` (seconds from midnight), the key after it and how
/// far between them `time` is (0..1), around midnight too (`0x0082DF70`).
fn around<T>(keys: &[(u32, T)], time: u32) -> Option<(&T, &T, f32)> {
    let time = time % DAY;
    let n = keys.len();
    if n == 0 {
        return None;
    }
    // The last key at or before `time`; before the first key, the last of the day.
    let i = keys.iter().rposition(|(t, _)| *t <= time).unwrap_or(n - 1);
    let j = (i + 1) % n;
    let (t0, a) = &keys[i];
    let (t1, b) = &keys[j];
    let span = (t1 + DAY - t0) % DAY;
    if span == 0 {
        return Some((a, a, 0.0));
    }
    let into = (time + DAY - t0) % DAY;
    Some((a, b, into as f32 / span as f32))
}

/// The value at `time` (seconds from midnight) of keys sorted by time: linear
/// between the key before and the key after.
fn at<T: Lerp>(keys: &[(u32, T)], time: u32) -> Option<T> {
    around(keys, time).map(|(a, b, t)| a.lerp(b, t))
}

/// The colour tables at `time`: the earlier key's weighed by `1 − t`, the
/// later one's by `t`.
fn grading_at(keys: &[(u32, Grading)], time: u32) -> GradingMix {
    let mut out = GradingMix::default();
    let Some((a, b, t)) = around(keys, time) else {
        return out;
    };
    for (vfx, w) in [(a.vfx, a.weight * (1.0 - t)), (b.vfx, b.weight * t)] {
        if let Some(slot) = out.tables[..out.count].iter_mut().find(|s| s.0 == vfx) {
            slot.1 += w;
        } else if w > 0.0 {
            out.tables[out.count] = (vfx, w);
            out.count += 1;
        }
    }
    out
}

impl Default for Fog {
    fn default() -> Self {
        // The game's defaults (`0x009CBEE0`): from 500 m to 1000 m, no colour.
        Self {
            color: [0.0; 3],
            far_color: [0.0; 3],
            start: 500.0,
            end: 1000.0,
            near_amount: 0.0,
            far_amount: 0.0,
        }
    }
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            low: [0.75, 0.85, 0.9],
            high: [0.45, 0.65, 0.9],
            glow_low: [0.0; 3],
            glow_high: [0.0; 3],
            glow_strength: 0.0,
            glow_power_high: 1.0,
            glow_power_low: 1.0,
            haze: 0.5,
        }
    }
}

impl Default for Lighting {
    fn default() -> Self {
        // `0x009D3650`.
        Self {
            shadow: 0.8,
            wrap: 0.21,
        }
    }
}

impl Default for Ambient {
    fn default() -> Self {
        // A flat grey ambient: only the constant term.
        let mut m = [0.0; 16];
        m[15] = 0.35;
        Self([m; 3])
    }
}

impl Sequence {
    /// Light, sky and fog at `time` (seconds from midnight); `city` picks
    /// the measured path of the sun.
    pub fn at(&self, city: &str, time: u32) -> Look {
        Look {
            fog: at(&self.fog, time).unwrap_or_default(),
            sky: at(&self.sky, time).unwrap_or_default(),
            lighting: at(&self.lighting, time).unwrap_or_default(),
            ambient: at(&self.ambient, time).unwrap_or_default(),
            sun: sun_direction(city, time),
            night: at(&self.night, time).unwrap_or(0.0),
            grading: grading_at(&self.grading, time),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fog.is_empty()
            && self.sky.is_empty()
            && self.lighting.is_empty()
            && self.ambient.is_empty()
    }

    fn sort(&mut self) {
        self.fog.sort_by_key(|k| k.0);
        self.sky.sort_by_key(|k| k.0);
        self.lighting.sort_by_key(|k| k.0);
        self.ambient.sort_by_key(|k| k.0);
        self.night.sort_by_key(|k| k.0);
        self.grading.sort_by_key(|k| k.0);
    }
}

/// The city of a stream path: `gf2/cuba/…` → `cuba`.
pub fn city_of(stream: &str) -> Option<&str> {
    let mut parts = stream.split(['/', '\\']);
    if parts.next()? != "gf2" {
        return None;
    }
    let city = parts.next()?;
    let city = city.strip_suffix(".str").unwrap_or(city);
    let city = city.strip_suffix("_global").unwrap_or(city);
    matches!(city, "cuba" | "queens" | "miami").then_some(city)
}

/// A colour attribute as RGB 0..1. Red is the low byte (`0xAABBGGRR`): the
/// game puts `value & 0xFF` into `x` (`0x009D5A00`), and only so the noon sky
/// is blue above and pale at the horizon, as in the game's frames.
fn color(value: Option<&Value>) -> Option<[f32; 3]> {
    let v = match value? {
        Value::Int(v) => *v as u32,
        Value::Hash(h) => h.0,
        _ => return None,
    };
    Some([v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF].map(|c| c as f32 / 255.0))
}

fn float(value: Option<&Value>) -> Option<f32> {
    match value? {
        Value::Float(v) => Some(*v),
        Value::Int(v) => Some(*v as f32),
        _ => None,
    }
}

/// A 4×4 matrix stored as text: 16 numbers separated by commas or spaces.
fn matrix(value: Option<&Value>) -> Option<[f32; 16]> {
    let Value::String(s) = value? else {
        return None;
    };
    let numbers: Vec<f32> = s
        .split([',', ' ', '\t'])
        .filter(|w| !w.is_empty())
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    numbers.try_into().ok()
}

impl CityLight {
    /// Reads the keys of a city from its `gf2/<city>/local` stream.
    pub fn read(game: &GameInstance, city: &str) -> Result<Self> {
        let name = format!("gf2/{city}/local");
        let index = game
            .find_stream(&name)
            .ok_or_else(|| Error::InvalidArgument(format!("no stream `{name}`")))?;
        let file = game.open_stream(index)?;
        let mut groups = Vec::new();
        for (i, entry) in file.layout().resources.iter().enumerate() {
            if entry.header.type_id == SIMGROUP {
                groups.push(SimGroup::parse(&file.resource_data(i)?)?);
            }
        }
        Ok(Self::from_groups(city, &groups))
    }

    /// The keys of the SimGroups of a city's `local` stream.
    pub fn from_groups(city: &str, groups: &[SimGroup]) -> Self {
        let mut names: HashMap<Guid, String> = HashMap::new();
        for g in groups {
            for o in g.objects.iter().filter(|o| o.class == CLASS_TOD_SEQUENCER) {
                let name = match g.attr(o, CLASS_TOD_SEQUENCER, 4) {
                    Some(Value::String(s)) => s.clone(),
                    _ => String::new(),
                };
                names.insert(o.guid, name);
            }
        }
        let mut sequences: HashMap<Guid, Sequence> = HashMap::new();
        for g in groups {
            for o in &g.objects {
                if ![
                    CLASS_SET_FOG,
                    CLASS_SET_SKY,
                    CLASS_LIGHTING,
                    CLASS_GLOBAL_SH,
                    CLASS_NIGHT_DAY,
                    CLASS_LUT,
                ]
                .contains(&o.class)
                {
                    continue;
                }
                let int = |i| match g.attr(o, CLASS_TOD_MESSAGE, i) {
                    Some(Value::Int(v)) => *v as u32,
                    _ => 0,
                };
                let time = (int(0) * 3600 + int(1) * 60 + int(2)) % DAY;
                let Some(Value::Guid(owner)) = g.attr(o, CLASS_TOD_MESSAGE, 4) else {
                    continue;
                };
                let seq = sequences.entry(*owner).or_default();
                let a = |i| g.attr(o, o.class, i);
                match o.class {
                    CLASS_SET_FOG => {
                        let d = Fog::default();
                        seq.fog.push((
                            time,
                            Fog {
                                color: color(a(0)).unwrap_or(d.color),
                                far_color: color(a(1)).unwrap_or(d.far_color),
                                start: float(a(2)).unwrap_or(0.0),
                                end: float(a(3)).unwrap_or(0.0),
                                near_amount: float(a(5)).unwrap_or(0.0),
                                far_amount: float(a(6)).unwrap_or(0.0),
                            },
                        ));
                    }
                    CLASS_SET_SKY => {
                        seq.sky.push((
                            time,
                            Sky {
                                high: color(a(2)).unwrap_or([0.0; 3]),
                                low: color(a(3)).unwrap_or([0.0; 3]),
                                glow_high: color(a(6)).unwrap_or([0.0; 3]),
                                glow_low: color(a(7)).unwrap_or([0.0; 3]),
                                glow_strength: float(a(1)).unwrap_or(0.0),
                                glow_power_high: float(a(8)).unwrap_or(1.0),
                                glow_power_low: float(a(10)).unwrap_or(1.0),
                                haze: float(a(12)).unwrap_or(0.5).max(1e-3),
                            },
                        ));
                    }
                    CLASS_LIGHTING => {
                        let d = Lighting::default();
                        seq.lighting.push((
                            time,
                            Lighting {
                                shadow: float(a(0)).unwrap_or(d.shadow),
                                wrap: float(a(1)).unwrap_or(d.wrap),
                            },
                        ));
                    }
                    CLASS_NIGHT_DAY => {
                        seq.night.push((time, float(a(0)).unwrap_or(0.0)));
                    }
                    CLASS_LUT => {
                        if let Some(Value::Guid(g)) = a(0)
                            && let Some(vfx) = g.resource_id()
                        {
                            seq.grading.push((
                                time,
                                Grading {
                                    vfx,
                                    weight: float(a(1)).unwrap_or(1.0),
                                },
                            ));
                        }
                    }
                    _ => {
                        let m = [matrix(a(0)), matrix(a(1)), matrix(a(2))];
                        if let [Some(r), Some(g), Some(b)] = m {
                            seq.ambient.push((time, Ambient([r, g, b])));
                        }
                    }
                }
            }
        }
        let mut out = Self {
            city: city.to_owned(),
            ..Self::default()
        };
        for (guid, mut seq) in sequences {
            seq.sort();
            seq.name = names.get(&guid).cloned().unwrap_or_default();
            let indoor = seq.name.to_ascii_lowercase().contains("_int");
            let slot = if indoor {
                &mut out.indoor
            } else {
                &mut out.outdoor
            };
            // Two outdoor sequencers would be odd; keep the one with more keys.
            let count = |s: &Sequence| {
                s.fog.len() + s.sky.len() + s.lighting.len() + s.ambient.len() + s.grading.len()
            };
            if count(&seq) > count(slot) {
                *slot = seq;
            }
        }
        out
    }

    /// Outdoor light at `time`.
    pub fn at(&self, time: u32) -> Look {
        self.outdoor.at(&self.city, time)
    }

    /// The colour tables of the city's outdoor grading by `vfx` id (see
    /// [`colour_table`]); keys whose table is not found are left out.
    pub fn colour_tables(&self, game: &GameInstance) -> HashMap<u32, ColourTable> {
        let wanted: Vec<u32> = self.outdoor.grading.iter().map(|(_, g)| g.vfx).collect();
        let streams = [format!("gf2/{}/local", self.city), GLOBALS.to_owned()];
        let mut names: HashMap<u32, String> = HashMap::new();
        for stream in &streams {
            let Some(file) = game
                .find_stream(stream)
                .and_then(|i| game.open_stream(i).ok())
            else {
                continue;
            };
            for (i, entry) in file.layout().resources.iter().enumerate() {
                let id = entry.header.ids[3];
                if entry.header.type_id != TYPE_VFX
                    || !wanted.contains(&id)
                    || names.contains_key(&id)
                {
                    continue;
                }
                if let Some(tail) = file
                    .resource_data(i)
                    .ok()
                    .as_deref()
                    .and_then(vfx_name_tail)
                {
                    names.insert(id, tail);
                }
            }
        }
        let Ok(tables) = colour_table_textures(game) else {
            return HashMap::new();
        };
        names
            .into_iter()
            .filter_map(|(id, tail)| {
                // The shortest name that ends with the readable tail.
                let table = tables
                    .iter()
                    .filter(|t| t.name.ends_with(&tail))
                    .min_by_key(|t| t.name.len())?;
                Some((id, table.clone()))
            })
            .collect()
    }
}

/// Type id of `vfx` resources (effects; the colour grading is one).
const TYPE_VFX: u32 = crate::hash::sdbm(b"vfx");
const GLOBALS: &str = "gf2/globals_global";

/// A colour table of the grading: `size`³ RGBA texels, red along x, green
/// along y, blue across slices (the game reads it at
/// `c · (size − 1) / size + 1 / (2 · size)`).
#[derive(Debug, Clone, PartialEq)]
pub struct ColourTable {
    /// Texture name, e.g. `lutcolor_miami_day`.
    pub name: String,
    pub size: usize,
    /// `size` slices of `size × size` RGBA texels.
    pub rgba: Vec<u8>,
}

impl ColourTable {
    /// The table at texel `(r, g, b)`.
    pub fn texel(&self, r: usize, g: usize, b: usize) -> [u8; 4] {
        let at = 4 * ((b * self.size + g) * self.size + r);
        self.rgba[at..at + 4].try_into().expect("4 bytes")
    }

    /// `c` (0..1) graded by the table, trilinear.
    pub fn grade(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size as f32 - 1.0;
        let p = c.map(|v| v.clamp(0.0, 1.0) * n);
        let i = p.map(|v| (v as usize).min(self.size - 2));
        let f: [f32; 3] = std::array::from_fn(|k| p[k] - i[k] as f32);
        let mut out = [0.0; 3];
        for corner in 0..8 {
            let d = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
            let w: f32 = (0..3)
                .map(|k| if d[k] == 1 { f[k] } else { 1.0 - f[k] })
                .product();
            let t = self.texel(i[0] + d[0], i[1] + d[1], i[2] + d[2]);
            for k in 0..3 {
                out[k] += w * t[k] as f32 / 255.0;
            }
        }
        out
    }
}

/// The readable start of the name in the `rcs.` block of a colour grading
/// `vfx` (12 bytes after the tag): the end of its colour table's name
/// (`r_miami_day` for `lutcolor_miami_day`; the 16-byte field is cut at its
/// start).
fn vfx_name_tail(data: &[u8]) -> Option<String> {
    let tag = data.windows(4).position(|w| w == b"rcs.")?;
    let field = data.get(tag + 12..tag + 28)?;
    let len = field
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
        .unwrap_or(16);
    (len > 0).then(|| String::from_utf8_lossy(&field[..len]).into_owned())
}

/// The colour tables of the game (`lutcolor_*` volume textures of
/// `gf2/globals_global`).
pub fn colour_table_textures(game: &GameInstance) -> Result<Vec<ColourTable>> {
    use crate::resource::texture::{Texture, pairs};
    use crate::resource::tg4::{Kind, Tg4Header};
    let index = game
        .find_stream(GLOBALS)
        .ok_or_else(|| Error::InvalidArgument(format!("no stream `{GLOBALS}`")))?;
    let file = game.open_stream(index)?;
    let mut out = Vec::new();
    for pair in pairs(file.layout()) {
        let Ok(header) = Tg4Header::parse(&file.resource_data(pair.header)?) else {
            continue;
        };
        if !header.name.starts_with("lutcolor_") || header.kind() != Kind::Volume {
            continue;
        }
        let texture = Texture::load(file.data(), file.layout(), pair)?;
        let size = header.width as usize;
        if size < 2 || header.height as usize != size || header.layer_count() != size {
            continue;
        }
        let mut rgba = Vec::with_capacity(size * size * size * 4);
        for layer in 0..size {
            let (w, h, pixels) = texture.rgba(layer)?;
            if w != size || h != size {
                return Err(Error::malformed(
                    "tg4d",
                    0,
                    "slices of a colour table differ",
                ));
            }
            rgba.extend_from_slice(&pixels);
        }
        out.push(ColourTable {
            name: header.name.clone(),
            size,
            rgba,
        });
    }
    Ok(out)
}

/// Direction of the sun's light (unit vector, Y up) every hour from
/// midnight, measured in the game for each city (`live peek` of the sun
/// pass, `renderer.md` §10.4). The game holds the sun still from 04:00 to
/// 06:00 and from 18:00 to 21:00 (the moon) and turns it between; each city
/// has a path of its own (New York's sun rises in the other quarter).
const SUN_CUBA: [[f32; 3]; 24] = [
    [0.4741, -0.8745, 0.1022],
    [0.2512, -0.9584, -0.1355],
    [-0.0068, -0.9343, -0.3565],
    [-0.2643, -0.8019, -0.5358],
    [-0.4915, -0.5767, -0.6526],
    [-0.4915, -0.5767, -0.6526],
    [-0.4955, -0.5714, -0.6542],
    [-0.4960, -0.6850, -0.5337],
    [-0.4858, -0.7800, -0.3943],
    [-0.4653, -0.8513, -0.2425],
    [-0.4350, -0.8966, -0.0825],
    [-0.3958, -0.9148, 0.0809],
    [-0.3489, -0.9051, 0.2429],
    [-0.2395, -0.9012, 0.3611],
    [-0.1255, -0.8729, 0.4715],
    [-0.0108, -0.8202, 0.5719],
    [0.1014, -0.7446, 0.6597],
    [0.2081, -0.6481, 0.7325],
    [0.3065, -0.5333, 0.7885],
    [0.3065, -0.5333, 0.7885],
    [0.3065, -0.5333, 0.7885],
    [0.3065, -0.5333, 0.7885],
    [0.3455, -0.7194, 0.6026],
    [0.4030, -0.8397, 0.3639],
];

const SUN_QUEENS: [[f32; 3]; 24] = [
    [-0.4674, -0.8672, 0.1716],
    [-0.2019, -0.9049, 0.3748],
    [0.0928, -0.8409, 0.5331],
    [0.3769, -0.6797, 0.6293],
    [0.6176, -0.4398, 0.6520],
    [0.6176, -0.4398, 0.6520],
    [0.6218, -0.4344, 0.6517],
    [0.6016, -0.5703, 0.5593],
    [0.5672, -0.6918, 0.4470],
    [0.5197, -0.7923, 0.3197],
    [0.4605, -0.8690, 0.1812],
    [0.3915, -0.9195, 0.0355],
    [0.3145, -0.9425, -0.1132],
    [0.1846, -0.9608, -0.2068],
    [0.0512, -0.9535, -0.2969],
    [-0.0815, -0.9204, -0.3825],
    [-0.2097, -0.8622, -0.4610],
    [-0.3302, -0.7807, -0.5306],
    [-0.4397, -0.6779, -0.5892],
    [-0.4397, -0.6779, -0.5892],
    [-0.4397, -0.6779, -0.5892],
    [-0.4397, -0.6779, -0.5892],
    [-0.4391, -0.8209, -0.3651],
    [-0.4479, -0.8884, -0.1003],
];

const SUN_MIAMI: [[f32; 3]; 24] = [
    [0.4617, -0.8868, -0.0211],
    [0.1848, -0.9630, -0.1963],
    [-0.1192, -0.9305, -0.3463],
    [-0.4096, -0.7902, -0.4558],
    [-0.6527, -0.5585, -0.5119],
    [-0.6527, -0.5585, -0.5119],
    [-0.6569, -0.5531, -0.5124],
    [-0.6304, -0.6677, -0.3959],
    [-0.5886, -0.7643, -0.2637],
    [-0.5329, -0.8374, -0.1218],
    [-0.4651, -0.8849, 0.0255],
    [-0.3871, -0.9055, 0.1740],
    [-0.3012, -0.8985, 0.3193],
    [-0.1659, -0.8984, 0.4067],
    [-0.0273, -0.8739, 0.4854],
    [0.1100, -0.8249, 0.5544],
    [0.2423, -0.7529, 0.6118],
    [0.3662, -0.6597, 0.6562],
    [0.4784, -0.5479, 0.6863],
    [0.4784, -0.5479, 0.6863],
    [0.4784, -0.5479, 0.6863],
    [0.4784, -0.5479, 0.6863],
    [0.4653, -0.7331, 0.4960],
    [0.4587, -0.8527, 0.2502],
];

/// Direction of the sun's (or moon's) light at `time` (seconds from
/// midnight): the path measured in the city, interpolated between the hours
/// (Havana's for other places).
pub fn sun_direction(city: &str, time: u32) -> [f32; 3] {
    let path = match city {
        "queens" => &SUN_QUEENS,
        "miami" => &SUN_MIAMI,
        _ => &SUN_CUBA,
    };
    let time = time % DAY;
    let h = (time / 3600) as usize;
    let t = (time % 3600) as f32 / 3600.0;
    let v = mix3(path[h], path[(h + 1) % 24], t);
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    v.map(|c| c / len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fog(start: f32) -> Fog {
        Fog {
            start,
            ..Fog::default()
        }
    }

    #[test]
    fn keys_interpolate_and_wrap_around_midnight() {
        let keys = vec![
            (6 * 3600, fog(10.0)),
            (12 * 3600, fog(70.0)),
            (21 * 3600, fog(40.0)),
        ];
        assert_eq!(at(&keys, 6 * 3600).unwrap().start, 10.0);
        assert_eq!(at(&keys, 9 * 3600).unwrap().start, 40.0);
        // 21:00 → 06:00 is 9 hours: at 01:30 half way.
        assert!((at(&keys, 3600 + 1800).unwrap().start - 25.0).abs() < 1e-4);
        assert!((at(&keys, 23 * 3600).unwrap().start - (40.0 - 30.0 * 2.0 / 9.0)).abs() < 1e-3);
        assert_eq!(at(&keys[..1], 5).unwrap().start, 10.0);
        assert!(at::<Fog>(&[], 5).is_none());
    }

    #[test]
    fn the_sun_is_up_by_day_and_turns_smoothly() {
        for city in ["cuba", "queens", "miami"] {
            let noon = sun_direction(city, 12 * 3600);
            assert!(noon[1] < -0.85, "{city}: light falls from above: {noon:?}");
            let mut prev = sun_direction(city, 0);
            for s in (0..DAY).step_by(600) {
                let d = sun_direction(city, s);
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-4);
                let dot = d[0] * prev[0] + d[1] * prev[1] + d[2] * prev[2];
                assert!(dot > 0.97, "{city}: no jump at {s}: {dot}");
                prev = d;
            }
        }
    }

    #[test]
    fn grading_weighs_the_tables_of_two_keys() {
        let g = |vfx| Grading { vfx, weight: 1.0 };
        // As in the cities: 11:01:34 a, 12:06:34 b, 15:30:34 a again.
        let keys = vec![
            ((11 * 60 + 1) * 60 + 34, g(1)),
            ((12 * 60 + 6) * 60 + 34, g(2)),
            ((15 * 60 + 30) * 60 + 34, g(1)),
        ];
        // At 12:03 the game weighs 0.0549 and 0.9451 (`live peek`).
        let m = grading_at(&keys, 12 * 3600 + 180);
        assert_eq!(m.count, 2);
        assert_eq!(m.tables()[0].0, 1);
        assert!((m.tables()[0].1 - 0.0549).abs() < 1e-3, "{m:?}");
        assert!((m.tables()[1].1 - 0.9451).abs() < 1e-3, "{m:?}");
        // The same table on both sides is one table.
        let m = grading_at(&keys[2..], 3600);
        assert_eq!(m.tables(), &[(1, 1.0)]);
        assert_eq!(grading_at(&[], 5).count, 0);
    }

    #[test]
    fn colour_grading_vfx_names_end_their_tables_names() {
        let mut data = b"r_miami_day\x8c\xf5\x15\xaf_####rcs.\x03\0\0\0\x01\0\0\0".to_vec();
        data.extend_from_slice(b"r_miami_day\x8c\xf5\x15\xaf_");
        data.extend_from_slice(&[0; 16]);
        assert_eq!(vfx_name_tail(&data).as_deref(), Some("r_miami_day"));
        assert_eq!(vfx_name_tail(&[0x8c; 40]), None);
        let table = ColourTable {
            name: "lutcolor_test".into(),
            size: 2,
            rgba: (0..8)
                .flat_map(|i: u8| {
                    let c = |bit: u8| if i & bit != 0 { 255 } else { 0 };
                    [c(1), c(2), c(4), 255]
                })
                .collect(),
        };
        // The identity table gives the colour back.
        let c = table.grade([0.25, 0.5, 0.75]);
        assert!(
            (c[0] - 0.25).abs() < 1e-4 && (c[2] - 0.75).abs() < 1e-4,
            "{c:?}"
        );
    }

    #[test]
    fn ambient_is_a_quadratic_form() {
        let mut m = [0.0; 16];
        m[15] = 0.5; // constant
        m[4 + 3] = 0.1; // y·1
        m[3 * 4 + 1] = 0.1; // 1·y
        let a = Ambient([m; 3]);
        assert!((a.eval([0.0, 1.0, 0.0])[0] - 0.7).abs() < 1e-6);
        assert!((a.eval([0.0, -1.0, 0.0])[0] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn colours_and_matrices_parse() {
        assert_eq!(
            color(Some(&Value::Int(0xFF80_4020u32 as i32))),
            Some([32.0 / 255.0, 64.0 / 255.0, 128.0 / 255.0])
        );
        let text = "1,0,0,0, 0,1,0,0,0,0,1,0,0,0,0,1";
        assert_eq!(matrix(Some(&Value::String(text.into()))).unwrap()[5], 1.0);
        assert!(matrix(Some(&Value::String("1 2 3".into()))).is_none());
        assert_eq!(city_of("gf2/cuba/local/yuri/high/zone4"), Some("cuba"));
        assert_eq!(city_of("gf2\\queens_global.str"), Some("queens"));
        assert_eq!(city_of("mp_mall/mp_mall"), None);
    }
}
